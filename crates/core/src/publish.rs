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
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("校验未通过：{count} 个错误", count = .findings.len())]
    Validation { findings: Vec<Finding> },
    #[error("没有需要提交的改动")]
    NothingToCommit,
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
    let status = process::run_collect("git", &["status", "--porcelain"], &repo).await?;
    if status.stdout_text().trim().is_empty() {
        step(emit, 2, StepState::Failed, "工作区干净".to_owned());
        return Err(PublishError::NothingToCommit);
    }
    if cancelled(&cancel) {
        return Err(PublishError::Cancelled);
    }

    step(emit, 2, StepState::Running, "git add -A".to_owned());
    process::run_collect("git", &["add", "-A"], &repo).await?;

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