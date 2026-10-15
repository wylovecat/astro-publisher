//! The publish pipeline, shared by the CLI and the desktop app.
//!
//! One implementation, two front ends: the headless CLI prints the events, the
//! publish panel renders them into a live log. Keeping the sequence here (rather
//! than in either front end) is what makes the GUI's "publish" button and
//! `astro-publisher-core publish` behave identically.
//!
//! The sequence is: validate → local build → commit → push → hosting deploy.
//! Validation errors block; warnings do not. A push failure never triggers an
//! automatic rebase or force push — the pipeline reports it and stops.
//!
//! What gets committed is *not* "whatever the working tree happens to contain".
//! [`PublishOptions::stage_paths`] carries an explicit list, and only those
//! paths are staged (plan §5, step 3): the repo is the blog's own checkout, and
//! a stray `git add -A` would sweep up whatever else was lying around in it.

use crate::process::{self, OutputLine, ProcessError, Stream};
use crate::validate::{self, Finding};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use thiserror::Error;

/// Number of steps in the pipeline (validation, build, commit, push, deploy).
pub const STEP_COUNT: usize = 5;

/// Step titles, in execution order. Index into this is the step number the UI
/// shows, so it must stay aligned with the emission order in [`run`].
pub const STEP_TITLES: [&str; STEP_COUNT] = [
    "校验内容",
    "本地构建",
    "提交改动",
    "推送",
    "托管平台部署",
];

/// Lifecycle of one pipeline step, for the UI's step list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Ok,
    /// Not applicable to this run (for example: build disabled in settings).
    Skipped,
    Failed,
}

impl StepState {
    pub fn label(self) -> &'static str {
        match self {
            StepState::Pending => "待执行",
            StepState::Running => "进行中",
            StepState::Ok => "成功",
            StepState::Skipped => "已跳过",
            StepState::Failed => "失败",
        }
    }
}

/// A progress event from the pipeline.
#[derive(Debug, Clone)]
pub enum PublishEvent {
    /// A step changed state.
    Step {
        index: usize,
        title: &'static str,
        state: StepState,
        /// Human-readable detail (counts, command, branch).
        detail: String,
    },
    /// One line of child-process output.
    Log(OutputLine),
    /// The pipeline finished successfully.
    Finished(PublishSummary),
}

/// What a successful run did.
#[derive(Debug, Clone)]
pub struct PublishSummary {
    pub message: String,
    pub branch: String,
    pub committed: bool,
    pub pushed: bool,
    pub post_count: usize,
    pub warning_count: usize,
    pub dry_run: bool,
}

/// Everything the pipeline needs to run.
#[derive(Debug, Clone)]
pub struct PublishOptions {
    pub repo: PathBuf,
    pub message: String,
    /// Run `pnpm build` before committing (settings toggle / `--no-build`).
    pub run_build: bool,
    /// Validate and build, but neither commit nor push.
    pub dry_run: bool,
    pub git_remote: String,
    /// Fallback branch when the checked-out branch cannot be resolved.
    pub git_branch: String,
    /// Repository-relative paths to stage, in the order shown to the user.
    ///
    /// `Some(list)` stages exactly that list; an empty list is an error rather
    /// than "stage everything", because the way to say "stage everything" is to
    /// say it. `None` stages the whole tree (`git add -A`) and is what the CLI
    /// does without `--stage`, which has no session to draw a list from.
    pub stage_paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("校验未通过：{count} 个错误", count = .findings.len())]
    Validation { findings: Vec<Finding> },
    #[error("没有需要提交的改动")]
    NothingToCommit,
    #[error("没有选中要提交的文件")]
    NothingSelected,
    #[error("构建失败")]
    Build { tail: String },
    #[error("提交失败：{0}")]
    Commit(String),
    #[error("推送失败：{0}")]
    Push(String),
    #[error("已取消")]
    Cancelled,
    #[error("无法确定当前分支")]
    UnknownBranch,
    #[error(transparent)]
    Process(#[from] ProcessError),
}

/// One entry of `git status --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChange {
    /// The two-column XY status code, e.g. `M `, `??`, `A `.
    pub code: String,
    /// Repository-relative path, with forward slashes, unquoted.
    pub path: String,
}

/// The working-tree changes a publish would have to deal with.
pub async fn pending_changes(repo: &Path) -> Result<Vec<StatusChange>, ProcessError> {
    let output = git_status(repo).await?;
    Ok(parse_status_porcelain(&output.stdout_text()))
}

/// `git status --porcelain` with `core.quotePath` off.
///
/// Without that setting git escapes every non-ASCII path into octal (`"\346\226\207.md"`),
/// which is useless in a UI whose whole point is showing the user which files
/// they are about to commit.
async fn git_status(repo: &Path) -> Result<process::CommandOutput, ProcessError> {
    process::run_collect(
        "git",
        &["-c", "core.quotePath=false", "status", "--porcelain"],
        repo,
    )
    .await
}

/// Parse `git status --porcelain` output.
///
/// Two shapes have to survive: the fixed `XY path` prefix, and the rename form
/// `XY old -> new`, where the path that will exist after the commit is the one
/// after the arrow. A renamed pair means the old path is going away and the new
/// one is arriving, and staging the new one is what stages the rename.
pub fn parse_status_porcelain(text: &str) -> Vec<StatusChange> {
    text.lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let code = line[..2].trim().to_owned();
            let rest = line[3..].trim();
            if code.is_empty() {
                return None;
            }
            let path = match rest.split_once(" -> ") {
                Some((_, new_path)) => new_path,
                None => rest,
            };
            let path = path.trim_matches('"').to_owned();
            if path.is_empty() {
                None
            } else {
                Some(StatusChange { code, path })
            }
        })
        .collect()
}

/// Run the pipeline, reporting progress through `emit`.
///
/// The callback is invoked from the same task, so front ends that need to hop
/// threads (the GUI) can push events onto a channel there. Cancellation is
/// cooperative: set the flag and the run stops at the next safe point, killing a
/// running build rather than leaving it behind.
///
/// `emit` is generic rather than `&mut dyn FnMut`: a `dyn` callback is never
/// `Send`, which would make the returned future non-`Send` and rule out callers
/// that must spawn it on a multithreaded runtime (the MCP server).
pub async fn run<F>(
    options: &PublishOptions,
    cancel: Arc<AtomicBool>,
    emit: &mut F,
) -> Result<PublishSummary, PublishError>
where
    F: FnMut(PublishEvent),
{
    let repo = options.repo.clone();
    let blog = repo.join("src").join("data").join("blog");

    // --- 1. Validation ---------------------------------------------------------
    step(
        emit,
        0,
        StepState::Running,
        "检查 frontmatter 与大纲链接…".to_owned(),
    );
    let report = validate::validate_all(&blog);
    if report.has_errors() {
        let findings: Vec<Finding> = report.errors().cloned().collect();
        for finding in &findings {
            emit(PublishEvent::Log(OutputLine {
                stream: Stream::Stderr,
                text: format!("[错误] {} — {}", finding.path, finding.message),
            }));
        }
        let count = findings.len();
        step(
            emit,
            0,
            StepState::Failed,
            format!("{count} 个错误，已中止发布"),
        );
        return Err(PublishError::Validation { findings });
    }
    step(
        emit,
        0,
        StepState::Ok,
        format!(
            "{} 篇文章，{} 个警告",
            report.post_count,
            report.warning_count()
        ),
    );

    if cancelled(&cancel) {
        return Err(PublishError::Cancelled);
    }

    // --- 2. Local build -------------------------------------------------------
    if options.run_build {
        step(
            emit,
            1,
            StepState::Running,
            "pnpm build…".to_owned(),
        );
        let output = stream_command(&repo, &cancel, emit, "pnpm", &["build"]).await?;
        if !output.success() {
            let tail = output.tail(25);
            step(emit, 1, StepState::Failed, "构建未通过".to_owned());
            return Err(PublishError::Build { tail });
        }
        step(emit, 1, StepState::Ok, "构建通过".to_owned());
    } else {
        step(
            emit,
            1,
            StepState::Skipped,
            "已在设置里关闭本地构建".to_owned(),
        );
    }

    if options.dry_run {
        step(emit, 2, StepState::Skipped, "演练模式".to_owned());
        step(emit, 3, StepState::Skipped, "演练模式".to_owned());
        step(emit, 4, StepState::Ok, "演练完成，未改动仓库".to_owned());
        return Ok(PublishSummary {
            message: options.message.clone(),
            branch: options.git_branch.clone(),
            committed: false,
            pushed: false,
            post_count: report.post_count,
            warning_count: report.warning_count(),
            dry_run: true,
        });
    }

    if cancelled(&cancel) {
        return Err(PublishError::Cancelled);
    }

    // --- 3. Commit ------------------------------------------------------------
    step(emit, 2, StepState::Running, "git status".to_owned());
    let status = git_status(&repo).await?;
    let changes = parse_status_porcelain(&status.stdout_text());
    if changes.is_empty() {
        step(emit, 2, StepState::Failed, "工作区干净".to_owned());
        return Err(PublishError::NothingToCommit);
    }
    if cancelled(&cancel) {
        return Err(PublishError::Cancelled);
    }

    match &options.stage_paths {
        // No list to work from: stage the whole tree. This is the CLI's
        // behaviour, and it says so in the log — a silent `git add -A` from a
        // GUI is how unrelated files end up in a content commit.
        None => {
            step(emit, 2, StepState::Running, "git add -A（全部改动）".to_owned());
            process::run_collect("git", &["add", "-A"], &repo).await?;
        }
        Some(paths) => {
            if paths.is_empty() {
                step(
                    emit,
                    2,
                    StepState::Failed,
                    "一个文件都没选中".to_owned(),
                );
                return Err(PublishError::NothingSelected);
            }
            step(
                emit,
                2,
                StepState::Running,
                format!("git add（{} 个文件）", paths.len()),
            );
            for path in paths {
                emit(PublishEvent::Log(OutputLine {
                    stream: Stream::Stdout,
                    text: format!("[暂存] {}", path.display()),
                }));
            }
            // `-A` so a deleted file the user picked is staged as a deletion,
            // and `--` so a path like `-weird.md` is not read as an option.
            let mut args: Vec<String> = vec!["add".to_owned(), "-A".to_owned(), "--".to_owned()];
            args.extend(paths.iter().map(|path| path.to_string_lossy().into_owned()));
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            process::run_collect("git", &borrowed, &repo).await?;

            // Staging is not the same as choosing: an index left over from an
            // earlier `git add` in a terminal is still staged, and `commit`
            // takes all of it. Say so rather than let it ride silently.
            for path in staged_beyond(&repo, paths).await? {
                emit(PublishEvent::Log(OutputLine {
                    stream: Stream::Stderr,
                    text: format!(
                        "[警告] {path} 不在所选清单里，但它此前已被暂存，本次提交会带上它"
                    ),
                }));
            }
        }
    }

    step(
        emit,
        2,
        StepState::Running,
        format!("git commit -m \"{}\"", options.message),
    );
    if let Err(err) = process::run_collect("git", &["commit", "-m", &options.message], &repo).await {
        let message = err.to_string();
        step(emit, 2, StepState::Failed, message.clone());
        return Err(PublishError::Commit(message));
    }
    step(emit, 2, StepState::Ok, "已提交".to_owned());

    if cancelled(&cancel) {
        return Err(PublishError::Cancelled);
    }

    // --- 4. Push --------------------------------------------------------------
    let branch = match current_branch(&repo).await {
        Ok(branch) => branch,
        Err(_) if !options.git_branch.trim().is_empty() => options.git_branch.trim().to_owned(),
        Err(_) => {
            step(emit, 3, StepState::Failed, "无法确定分支".to_owned());
            return Err(PublishError::UnknownBranch);
        }
    };
    let remote = options.git_remote.trim();
    let remote = if remote.is_empty() {
        "origin"
    } else {
        remote
    };

    step(emit, 3, StepState::Running, format!("git push {remote} {branch}"));
    match process::run_collect("git", &["push", remote, &branch], &repo).await {
        Ok(_) => step(emit, 3, StepState::Ok, format!("已推送到 {remote}/{branch}")),
        Err(err) => {
            // Deliberately no rebase, no force: pushing over a diverged branch
            // is the author's call, not the tool's.
            let message = format!(
                "{err}\n未做任何自动 rebase/强推，请手动处理：\
                 git pull --rebase {remote} {branch} 后重试"
            );
            step(emit, 3, StepState::Failed, "推送失败".to_owned());
            return Err(PublishError::Push(message));
        }
    }

    // --- 5. Hosting platform builds from the push ------------------------------
    step(
        emit,
        4,
        StepState::Ok,
        "托管平台会在收到推送后自动构建部署".to_owned(),
    );

    let summary = PublishSummary {
        message: options.message.clone(),
        branch: branch.clone(),
        committed: true,
        pushed: true,
        post_count: report.post_count,
        warning_count: report.warning_count(),
        dry_run: false,
    };
    emit(PublishEvent::Finished(summary.clone()));
    Ok(summary)
}

/// Staged paths that were not in the selection.
///
/// Leftovers matter: `git commit` takes whatever is in the index, so a file
/// staged by an earlier `git add` — in a terminal, or by a previous run of this
/// tool — joins the commit no matter what the current run selected.
async fn staged_beyond(repo: &Path, selected: &[PathBuf]) -> Result<Vec<String>, ProcessError> {
    let output = process::run_collect(
        "git",
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--cached",
            "--name-only",
        ],
        repo,
    )
    .await?;

    let allowed: std::collections::HashSet<String> =
        selected.iter().map(|path| normalise(path)).collect();

    Ok(output
        .stdout_text()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !allowed.contains(&normalise(Path::new(line))))
        .map(str::to_owned)
        .collect())
}

/// Repo-relative, forward-slashed form, which is what git reports and what the
/// selection list holds.
fn normalise(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn step(
    emit: &mut impl FnMut(PublishEvent),
    index: usize,
    state: StepState,
    detail: String,
) {
    emit(PublishEvent::Step {
        index,
        title: STEP_TITLES[index],
        state,
        detail,
    });
}

fn cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Relaxed)
}

/// Run a command, streaming its output line by line into the event callback.
///
/// Cancellation is checked while the command runs so a hung `pnpm build` can be
/// killed instead of blocking the pipeline forever.
async fn stream_command(
    cwd: &Path,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(PublishEvent),
    program: &str,
    args: &[&str],
) -> Result<process::CommandOutput, ProcessError> {
    let mut running = process::spawn_streaming(program, args, cwd)?;

    loop {
        tokio::select! {
            line = running.lines.recv() => match line {
                Some(line) => emit(PublishEvent::Log(line)),
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if cancelled(cancel) {
                    let _ = running.handle.kill_tree().await;
                    let _ = running.finish().await;
                    return Ok(process::CommandOutput {
                        status: Some(-1),
                        lines: Vec::new(),
                    });
                }
            }
        }
    }

    if let Err(source) = running.handle.wait().await {
        return Err(ProcessError::Io {
            program: program.to_owned(),
            source,
        });
    }
    running.finish().await
}

async fn current_branch(repo: &Path) -> Result<String, ProcessError> {
    let out = process::run_collect("git", &["rev-parse", "--abbrev-ref", "HEAD"], repo).await?;
    let branch = out.stdout_text().trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err(ProcessError::Io {
            program: "git".to_owned(),
            source: std::io::Error::other("detached HEAD"),
        });
    }
    Ok(branch)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shapes of `git status --porcelain` (with `core.quotePath=false`
    /// so the Chinese path stays readable, which is the whole reason for the
    /// `-c` flag the pipeline passes).
    const SAMPLE: &str = " M src/data/blog/lgj/lg-B2620.md\n?? src/data/blog/solu/新文章.md\nA  src/data/blog/lgs/lg-P1234.md\n";

    #[test]
    fn reads_code_and_path_from_every_status_shape() {
        let changes = parse_status_porcelain(SAMPLE);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].code, "M");
        assert_eq!(changes[0].path, "src/data/blog/lgj/lg-B2620.md");
        assert_eq!(changes[1].code, "??");
        // Non-ASCII survives instead of arriving as octal escapes.
        assert_eq!(changes[1].path, "src/data/blog/solu/新文章.md");
        assert_eq!(changes[2].code, "A");
    }

    #[test]
    fn a_rename_is_reported_under_the_path_it_moves_to() {
        // `R  old -> new`: staging `new` is what stages the rename, and `old`
        // does not exist afterwards, so the arrow's right side is the path that
        // belongs in the selection.
        let changes = parse_status_porcelain("R  src/data/blog/old.md -> src/data/blog/new.md\n");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].code, "R");
        assert_eq!(changes[0].path, "src/data/blog/new.md");
    }

    #[test]
    fn a_clean_tree_parses_to_nothing() {
        assert!(parse_status_porcelain("").is_empty());
        assert!(parse_status_porcelain("   \n\n").is_empty());
    }

    #[test]
    fn windows_separators_normalise_to_the_form_git_prints() {
        let raw = PathBuf::from(r"src\data\blog\lgj\lg-B2620.md");
        assert_eq!(normalise(&raw), "src/data/blog/lgj/lg-B2620.md");
    }
}
