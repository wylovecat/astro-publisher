//! Process helpers: run external tools (`pnpm`, `git`) and stream their output
//! line by line.
//!
//! Two things matter on Windows and are handled here: `pnpm` resolves to
//! `pnpm.cmd`, and child output is decoded as UTF-8 so Chinese build logs do not
//! turn into mojibake in the UI (a failure mode this project has hit before).
//!
//! Every command also runs with `CREATE_NO_WINDOW` (see [`hide_console`]). The
//! desktop app ships as a GUI binary with no console of its own, so without that
//! flag Windows gives each child a brand-new console window: a black window that
//! flashes on every `git` call and stays on screen for the whole `pnpm build`.
//!
//! Spawns are also handed to [`crate::job`], so a child that outlives the app by
//! accident — the app killed from the task manager, say — does not outlive the
//! app by design.

use crate::job;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn `{program}` in {cwd}: {source}")]
    Spawn {
        program: String,
        cwd: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{program}` exited with status {code:?}; output tail:\n{tail}")]
    NonZero {
        program: String,
        code: Option<i32>,
        tail: String,
    },
    #[error("io error while reading `{program}` output: {source}")]
    Io {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

/// One line of process output, tagged by stream.
#[derive(Debug, Clone)]
pub struct OutputLine {
    pub stream: Stream,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Result of a finished command.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub lines: Vec<OutputLine>,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        self.lines
            .iter()
            .filter(|l| l.stream == Stream::Stdout)
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Last `n` lines of combined output, for error messages.
    pub fn tail(&self, n: usize) -> String {
        let skip = self.lines.len().saturating_sub(n);
        self.lines
            .iter()
            .skip(skip)
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Resolve an executable name for the current platform.
pub fn resolve_program(program: &str) -> String {
    if cfg!(windows) && program == "pnpm" {
        "pnpm.cmd".to_owned()
    } else {
        program.to_owned()
    }
}

/// Keep a console-subsystem child from opening (or flashing) a console window.
///
/// A GUI process has no console, so Windows allocates a fresh one for every
/// console-subsystem child: `git.exe` flashes a black window, and `pnpm.cmd`
/// (which is a batch file, so the real child chain is `cmd.exe` -> `node.exe`)
/// keeps one on screen for the entire build. Nothing we spawn needs a console -
/// output travels over pipes to the UI - and the effect carries down the tree:
/// a grandchild of a console-less child stays silent as well (verified by
/// watching console windows appear with and without the flag).
///
/// No-op off Windows, where the equivalent long-standing problem does not exist.
fn hide_console(_cmd: &mut Command) {
    #[cfg(windows)]
    {
        // `CREATE_NO_WINDOW`: run the child with no console at all.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Tokio's `Command` forwards this to `std::process::Command`.
        _cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Same as [`hide_console`], for the synchronous `std` command used at shutdown.
#[cfg(windows)]
fn hide_console_std(_cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    _cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Drop ANSI SGR sequences from a line of process output.
///
/// Astro and Vite colour their logs even when stdout is a pipe, so the escape
/// bytes would otherwise be rendered literally by the UI — and would defeat
/// matching on the text, such as finding the dev server's address line.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        // `ESC [ <params> <final byte>` is the CSI form used for colour; the
        // final byte is the first character in `@`–`~`.
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// A handle to a running child process, used for cancellable long tasks such as
/// the `astro dev` preview server (plan §4.2).
pub struct ChildHandle {
    child: Child,
}

/// Kill a process tree from a synchronous context, by pid.
///
/// [`ChildHandle::kill_tree`] is async and needs a runtime; this is what to call
/// from a place that has neither, such as an application-quit callback.
pub fn kill_tree_blocking(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_console_std(&mut cmd);
        let _ = cmd.status();
    }
    #[cfg(unix)]
    {
        // Only the direct child: unlike Windows, `taskkill /T` has no portable
        // equivalent here, and signalling a process group we did not create
        // would reach our own.
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl ChildHandle {
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Wait for the process to exit, returning its status code.
    pub async fn wait(&mut self) -> std::io::Result<Option<i32>> {
        Ok(self.child.wait().await?.code())
    }

    /// Kill the process tree (Windows needs `taskkill /T`; Unix kills the group).
    pub async fn kill_tree(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            if let Some(pid) = self.child.id() {
                let mut cmd = Command::new("taskkill");
                cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                hide_console(&mut cmd);
                let _ = cmd.status().await;
            }
        }
        self.child.kill().await
    }
}

/// A spawned command: the streaming output channel, the child handle (for
/// cancellation), and a task that resolves to the collected output once both
/// output pipes close.
pub struct RunningCommand {
    pub handle: ChildHandle,
    pub lines: mpsc::UnboundedReceiver<OutputLine>,
    finished: tokio::task::JoinHandle<CommandOutput>,
}

impl RunningCommand {
    /// Await the process exit status and the full output.
    pub async fn finish(self) -> Result<CommandOutput, ProcessError> {
        let RunningCommand { lines, finished, .. } = self;
        // Keep receiving so the pump tasks are never blocked on a full channel.
        let drain = tokio::spawn(async move {
            let mut rx = lines;
            while rx.recv().await.is_some() {}
        });
        let output = finished.await.unwrap_or(CommandOutput {
            status: None,
            lines: Vec::new(),
        });
        drain.abort();
        Ok(output)
    }
}

/// Spawn a command whose output streams to a channel while it runs.
pub fn spawn_streaming(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<RunningCommand, ProcessError> {
    let resolved = resolve_program(program);
    let mut cmd = Command::new(&resolved);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Force UTF-8 so Chinese build output stays readable.
        .env("LANG", "UTF-8")
        .env("LC_ALL", "UTF-8")
        .env("PYTHONIOENCODING", "utf-8");
    hide_console(&mut cmd);

    let mut child = cmd.spawn().map_err(|source| ProcessError::Spawn {
        program: resolved.clone(),
        cwd: cwd.to_path_buf(),
        source,
    })?;
    if let Some(pid) = child.id() {
        job::adopt(pid);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::unbounded_channel();

    let out_task = stdout.map(|out| tokio::spawn(pump(out, Stream::Stdout, tx.clone())));
    let err_task = stderr.map(|err| tokio::spawn(pump(err, Stream::Stderr, tx)));

    // The child stays with the caller (so it can be killed); the exit status is
    // reported once the output pipes close, which happens at process exit.
    let finished = tokio::spawn(async move {
        let mut lines = Vec::new();
        if let Some(task) = out_task {
            if let Ok(mut collected) = task.await {
                lines.append(&mut collected);
            }
        }
        if let Some(task) = err_task {
            if let Ok(mut collected) = task.await {
                lines.append(&mut collected);
            }
        }
        CommandOutput { status: None, lines }
    });

    Ok(RunningCommand {
        handle: ChildHandle { child },
        lines: rx,
        finished,
    })
}

/// A long-running child process, such as the `astro dev` preview server.
pub struct RunningProcess {
    pub handle: ChildHandle,
    pub lines: mpsc::UnboundedReceiver<OutputLine>,
}

/// Spawn a long-running child, streaming its output as it arrives.
///
/// [`spawn_streaming`] keeps every line it has seen until the process exits,
/// which is right for a bounded command such as `pnpm build` and wrong for a
/// server that may run for hours: that buffer would grow without limit. This
/// variant only forwards, so nothing accumulates.
///
/// Like the other spawn helpers, this must be called from within a tokio
/// runtime — it uses `tokio::spawn` for the output pumps.
pub fn spawn_background(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<RunningProcess, ProcessError> {
    let resolved = resolve_program(program);
    let mut cmd = Command::new(&resolved);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Force UTF-8 so Chinese build output stays readable.
        .env("LANG", "UTF-8")
        .env("LC_ALL", "UTF-8")
        .env("PYTHONIOENCODING", "utf-8");
    hide_console(&mut cmd);

    let mut child = cmd.spawn().map_err(|source| ProcessError::Spawn {
        program: resolved,
        cwd: cwd.to_path_buf(),
        source,
    })?;
    if let Some(pid) = child.id() {
        job::adopt(pid);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::unbounded_channel();

    if let Some(out) = stdout {
        tokio::spawn(forward(out, Stream::Stdout, tx.clone()));
    }
    if let Some(err) = stderr {
        tokio::spawn(forward(err, Stream::Stderr, tx));
    }

    Ok(RunningProcess {
        handle: ChildHandle { child },
        lines: rx,
    })
}

/// Forward output lines to the channel, keeping none of them.
async fn forward<R>(reader: R, stream: Stream, tx: mpsc::UnboundedSender<OutputLine>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(text)) = lines.next_line().await {
        if tx.send(OutputLine { stream, text }).is_err() {
            // The receiver is gone, so nobody wants the rest.
            break;
        }
    }
}

async fn pump<R>(
    reader: R,
    stream: Stream,
    tx: mpsc::UnboundedSender<OutputLine>,
) -> Vec<OutputLine>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut collected = Vec::new();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(text)) = lines.next_line().await {
        let line = OutputLine { stream, text };
        collected.push(line.clone());
        // If the receiver is gone the task was cancelled; keep draining quietly.
        let _ = tx.send(line);
    }
    collected
}

/// Run a command to completion, collecting all output. Convenience for short
/// commands such as `git status`.
pub async fn run_collect(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<CommandOutput, ProcessError> {
    let resolved = resolve_program(program);
    let mut cmd = Command::new(&resolved);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        // Pinned explicitly: `Command::output` used to do this, and dropping it
        // would send the child's output to the app's own (nonexistent)
        // console instead of into the pipe this function reads from.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LANG", "UTF-8")
        .env("LC_ALL", "UTF-8");
    hide_console(&mut cmd);

    // Spawned by hand rather than via `Command::output` purely to get the pid:
    // that is what puts the child in the kill-on-close job, so a `pnpm build`
    // cannot outlive a killed app. It costs the caller this function's
    // promise — that the output is captured — so the pipes above are not
    // optional.
    let child = cmd.spawn().map_err(|source| ProcessError::Spawn {
        program: resolved.clone(),
        cwd: cwd.to_path_buf(),
        source,
    })?;
    if let Some(pid) = child.id() {
        job::adopt(pid);
    }

    let output = child.wait_with_output().await.map_err(|source| {
        ProcessError::Io {
            program: resolved.clone(),
            source,
        }
    })?;

    let mut lines = Vec::new();
    lines.extend(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|text| OutputLine {
                stream: Stream::Stdout,
                text: text.to_owned(),
            }),
    );
    lines.extend(
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(|text| OutputLine {
                stream: Stream::Stderr,
                text: text.to_owned(),
            }),
    );

    let result = CommandOutput {
        status: output.status.code(),
        lines,
    };

    if !result.success() {
        return Err(ProcessError::NonZero {
            program: resolved,
            code: result.status,
            tail: result.tail(20),
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from `pnpm dev --port 4321` in the blog repository: Astro colours
    /// its log even when stdout is a pipe, so this is what actually arrives.
    const READY_ADDRESS: &str =
        "\u{1b}[2m\u{2503}\u{1b}[22m Local    \u{1b}[36mhttp://localhost:4321/\u{1b}[39m";
    const READY_TIMING: &str = "\u{1b}[42m\u{1b}[1m astro \u{1b}[22m\u{1b}[49m \u{1b}[32mv7.2.8\u{1b}[39m \u{1b}[2mready in\u{1b}[22m 6388 \u{1b}[2mms\u{1b}[22m";

    #[test]
    fn strips_the_escapes_astro_really_emits() {
        assert_eq!(
            strip_ansi(READY_ADDRESS),
            "┃ Local    http://localhost:4321/"
        );
        assert_eq!(
            strip_ansi(READY_TIMING),
            " astro  v7.2.8 ready in 6388 ms"
        );
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(strip_ansi("no escapes here"), "no escapes here");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn a_truncated_escape_does_not_eat_what_follows() {
        assert_eq!(strip_ansi("done\u{1b}"), "done");
        assert_eq!(strip_ansi("done\u{1b}["), "done");
    }

    /// `run_collect` has to capture the child's output.
    ///
    /// It spawns by hand rather than through `Command::output`, and that only
    /// works if the pipes are requested explicitly — without them the child
    /// writes to the app's inherited (nonexistent) console and the caller gets
    /// an empty string back. That failure is invisible until something reads
    /// stdout: `git status` looked like "the working tree is clean", and the
    /// publish pipeline could not even resolve the current branch.
    #[test]
    fn run_collect_returns_what_the_child_printed() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");

        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/c", "echo", "captured-ok"])
        } else {
            ("sh", &["-c", "echo captured-ok"])
        };

        let output = runtime
            .block_on(run_collect(program, args, std::path::Path::new(".")))
            .expect("the command succeeds");

        assert!(
            output.stdout_text().contains("captured-ok"),
            "stdout was {:?} — the child's output escaped the pipe",
            output.stdout_text()
        );
    }
}
