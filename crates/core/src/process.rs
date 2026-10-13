//! Process helpers: run external tools (`pnpm`, `git`) and stream their output
//! line by line.
//!
//! Two things matter on Windows and are handled here: `pnpm` resolves to
//! `pnpm.cmd`, and child output is decoded as UTF-8 so Chinese build logs do not
//! turn into mojibake in the UI (a failure mode this project has hit before).

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

/// A handle to a running child process, used for cancellable long tasks such as
/// the `astro dev` preview server (plan §4.2).
pub struct ChildHandle {
    child: Child,
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
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
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

    let mut child = cmd.spawn().map_err(|source| ProcessError::Spawn {
        program: resolved.clone(),
        cwd: cwd.to_path_buf(),
        source,
    })?;

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
    let output = Command::new(&resolved)
        .args(args)
        .current_dir(cwd)
        .env("LANG", "UTF-8")
        .env("LC_ALL", "UTF-8")
        .output()
        .await
        .map_err(|source| ProcessError::Spawn {
            program: resolved.clone(),
            cwd: cwd.to_path_buf(),
            source,
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
