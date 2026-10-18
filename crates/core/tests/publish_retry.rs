//! Publishing, when the run does not start at the beginning.
//!
//! The pipeline has two recovery paths that are awkward to exercise any other
//! way: re-running from a step that failed (plan §5, step 3 — once the commit
//! has landed, a second "发布" would find a clean tree and refuse), and the
//! pull-and-rebase offered when a push is rejected as non-fast-forward.
//!
//! Both need a real repository, so these tests build one out of thin air:
//! a scratch work tree and a bare repository standing in for the remote, which
//! keeps everything on the local filesystem and offline. The corpus is a single
//! valid post — validation runs on every start and a corpus with broken outline
//! links (like `tests/fixtures`) would stop the pipeline before the step under
//! test.

use astro_publisher_core::publish::{self, PublishError, PublishEvent, PublishOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

struct Scratch {
    root: PathBuf,
    remote: PathBuf,
}

/// Run git, and fail loudly with its own output when it does.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|err| panic!("git {args:?} could not run: {err}"));
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "an-publisher-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("a scratch directory");
    base
}

/// A post with the frontmatter the advisory validator requires, and no outline
/// links for it to chase.
fn write_post(root: &Path, relative: &str, title: &str) {
    let path = root.join("src").join("data").join("blog").join(relative);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the post directory");
    let body = format!(
        "---\ntitle: {title}\ndescription: 测试用文章。\n\
         pubDatetime: 2026-01-01T00:00:00.000+08:00\ntags: [test]\ndraft: false\n---\n\n正文。\n"
    );
    std::fs::write(path, body).expect("the post file");
}

/// A work tree with one commit already pushed to a bare "remote".
///
/// Both repositories get their `HEAD` pinned to `main` by hand. This machine's
/// `init.defaultBranch` is `master`, so a bare repository would otherwise
/// advertise an unborn `master` while the push creates `main` — and `git clone`
/// that resolves `HEAD` to a ref which does not exist checks out *nothing*,
/// leaving a "second machine" with no working tree at all.
fn init(name: &str) -> Scratch {
    let base = scratch(name);
    let root = base.join("repo");
    let remote = base.join("remote.git");

    std::fs::create_dir_all(&remote).expect("the remote directory");
    git(&remote, &["init", "--bare"]);
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    std::fs::create_dir_all(&root).expect("the work tree");
    git(&root, &["init"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "test"]);
    // Signing is a machine-level setting, and a test must not depend on whether
    // this machine happens to have a key configured.
    git(&root, &["config", "commit.gpgsign", "false"]);

    write_post(&root, "solu/one.md", "第一篇");
    // A plain file both sides can rewrite, to force a real conflict later. It
    // sits outside the blog corpus, so it never affects validation.
    std::fs::write(root.join("notes.txt"), "a\nb\nc\n").expect("the shared file");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "first"]);
    git(
        &root,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    git(&root, &["push", "-u", "origin", "main"]);

    Scratch { root, remote }
}

/// A second checkout of the same remote, standing in for another machine.
fn clone_of(scratch: &Scratch) -> PathBuf {
    let dir = scratch
        .root
        .parent()
        .expect("the scratch base")
        .join("other");
    git(
        scratch.root.parent().expect("the scratch base"),
        &[
            "clone",
            &scratch.remote.to_string_lossy(),
            &dir.to_string_lossy(),
        ],
    );
    git(&dir, &["config", "user.email", "other@example.com"]);
    git(&dir, &["config", "user.name", "other"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);
    dir
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", message]);
}

fn options(scratch: &Scratch, message: &str) -> PublishOptions {
    PublishOptions {
        repo: scratch.root.clone(),
        message: message.to_owned(),
        // No `pnpm` in a scratch repository; the build step is not what these
        // tests are about.
        run_build: false,
        dry_run: false,
        git_remote: "origin".to_owned(),
        git_branch: "main".to_owned(),
        stage_paths: None,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
}

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Retrying from the push step must push the commit that exists and leave the
/// rest of the working tree alone.
///
/// This is the whole point of starting late: the commit has already happened, so
/// re-running from the top would stop at "工作区干净" and never push.
#[test]
fn restarting_at_the_push_step_pushes_the_existing_commit() {
    let scratch = init("retry-push");
    write_post(&scratch.root, "solu/two.md", "第二篇");
    commit_all(&scratch.root, "second");
    // Work in progress that a retry must not sweep into a commit.
    write_post(&scratch.root, "solu/three.md", "第三篇");

    let options = options(&scratch, "docs: 补充章节");
    let mut events: Vec<PublishEvent> = Vec::new();
    let mut emit = |event: PublishEvent| events.push(event);
    let summary = runtime()
        .block_on(publish::run_from(
            &options,
            publish::STEP_PUSH,
            cancel(),
            &mut emit,
        ))
        .expect("pushing the commit that is already made succeeds");

    assert!(summary.pushed, "the commit should have reached the remote");
    assert!(
        !summary.committed,
        "a run that started at the push step must not report a commit"
    );

    let status = git(&scratch.root, &["status", "--porcelain"]);
    assert!(
        status.contains("three.md"),
        "the uncommitted file was swept into the commit: {status}"
    );
    let remote_log = git(&scratch.remote, &["log", "--oneline", "main"]);
    assert!(
        remote_log.contains("second"),
        "the pending commit never arrived: {remote_log}"
    );
}

/// A push rejected because the remote moved on is its own error, so the UI can
/// offer the one recovery that fits instead of a generic push failure.
#[test]
fn a_remote_that_moved_on_reads_as_a_non_fast_forward() {
    let scratch = init("diverged");
    let other = clone_of(&scratch);
    write_post(&other, "solu/theirs.md", "别人的改动");
    commit_all(&other, "theirs");
    git(&other, &["push"]);

    // Our copy commits something else without ever seeing theirs.
    write_post(&scratch.root, "solu/mine.md", "我的改动");
    commit_all(&scratch.root, "mine");

    let options = options(&scratch, "docs: 我的改动");
    let mut events: Vec<PublishEvent> = Vec::new();
    let mut emit = |event: PublishEvent| events.push(event);
    let error = runtime()
        .block_on(publish::run_from(
            &options,
            publish::STEP_PUSH,
            cancel(),
            &mut emit,
        ))
        .expect_err("the push cannot succeed against a branch that moved on");

    assert!(
        matches!(error, PublishError::PushNonFastForward { .. }),
        "expected a non-fast-forward, got {error:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, PublishEvent::PushRejected)),
        "the UI is told to offer the rebase retry through an event"
    );
}

/// Pull-and-rebase is the recovery, and when it conflicts it must not leave the
/// repository half-rebased.
///
/// A rebase left in progress would put conflict markers in the working tree, and
/// the next `发布` would stage them. Aborting returns the tree to where it was,
/// with the local commit intact, and says which files clashed.
#[test]
fn a_conflicting_rebase_is_aborted_rather_than_left_in_progress() {
    let scratch = init("conflict");
    let other = clone_of(&scratch);
    std::fs::write(other.join("notes.txt"), "a\nother\nc\n").expect("their edit");
    commit_all(&other, "theirs");
    git(&other, &["push"]);

    std::fs::write(scratch.root.join("notes.txt"), "a\nmine\nc\n").expect("our edit");
    commit_all(&scratch.root, "mine");

    let options = options(&scratch, "docs: 冲突");
    let mut events: Vec<PublishEvent> = Vec::new();
    let mut emit = |event: PublishEvent| events.push(event);
    let error = runtime()
        .block_on(publish::pull_rebase(&options, cancel(), &mut emit))
        .expect_err("the two sides rewrite the same line");

    let PublishError::Rebase { message } = &error else {
        panic!("expected a rebase failure, got {error:?}");
    };
    assert!(
        message.contains("notes.txt"),
        "the clashing file should be named: {message}"
    );
    assert!(
        message.contains("--abort"),
        "the message should say the rebase was undone: {message}"
    );

    let status = git(&scratch.root, &["status", "--porcelain"]);
    assert!(
        !status.contains("UU"),
        "a rebase was left in progress: {status}"
    );
    let log = git(&scratch.root, &["log", "--oneline"]);
    assert!(
        log.contains("mine"),
        "aborting must keep the local commit: {log}"
    );
    assert!(
        !log.contains("theirs"),
        "the rebase did not finish, so theirs is not in yet: {log}"
    );
}
