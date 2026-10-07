//! Cancellation-safe cleanup for shell commands and their descendants.

use std::{path::PathBuf, process::Stdio, time::Duration};

#[cfg(unix)]
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
};

use crate::agent::tools::types::ToolOutputSink;

/// Start a shell in its own process group so dropping an in-flight tool can
/// terminate the shell and any commands it spawned, not just the direct child.
pub(super) fn spawn_shell(command: &mut Command) -> Result<ShellChild, std::io::Error> {
    #[cfg(unix)]
    command.process_group(0);

    command.kill_on_drop(true);

    let child = command.spawn()?;

    Ok(ShellChild {
        #[cfg(unix)]
        group_id: child.id().expect("spawned process has an ID"),
        child,
        running: true,
    })
}

/// Wrapper around a Tokio `Child` that tracks process group membership and
/// lifecycle state for cancellation-safe cleanup of shell commands.
pub(super) struct ShellChild {
    /// The underlying Tokio child process handle.
    pub(super) child: Child,

    /// The Unix process group ID for sending signals to descendants.
    #[cfg(unix)]
    group_id: u32,

    /// Whether the child is still running and needs termination on drop.
    running: bool,
}

impl ShellChild {
    /// Kill the entire group before reaping the direct child. This is also
    /// used on timeout; `Drop` covers cancellation while awaiting the child.
    pub(super) fn terminate(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        {
            let group = Pid::from_raw(-(self.group_id as i32));

            match kill(group, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {},
                Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
            }
        }

        #[cfg(not(unix))]
        self.child.start_kill()?;

        Ok(())
    }

    /// Mark the child as no longer running so it won't be terminated on drop.
    pub(super) fn finished(&mut self) {
        self.running = false;
    }
}

impl Drop for ShellChild {
    fn drop(&mut self) {
        if self.running {
            let _ = self.terminate();
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ShellCommandError {
    #[error("{0:?}")]
    ProcessError(#[from] std::io::Error),

    #[error("{0} was explicitly piped")]
    ExplicitlyPipedHandle(String),

    #[error("failed to wait for command: {0}")]
    CommandFailedToWait(std::io::Error),

    #[error("failed to kill timed out command: {0}")]
    FailedToKillTimedoutCommand(std::io::Error),

    #[error("failed to reap timed out command: {0}")]
    FailedToReapTimedoutCommand(std::io::Error),

    #[error("command timed out after {0}s")]
    CommandTimedOut(u64),

    #[error("{0} capture task failed: {1:?}")]
    FailedToCaptureHandle(String, tokio::task::JoinError),

    #[error("{0} capture task failed: {1:?}")]
    CapturedStreamError(String, anyhow::Error),
}

pub struct ShellCommandOutput {
    pub content: String,
    pub success: bool,
}

pub async fn run_shell_command(
    shell: Option<&[String]>,
    command: &str,
    working_directory: &PathBuf,
    timeout: u64,
    stream_limit: usize,
    output: Option<ToolOutputSink>,
) -> Result<ShellCommandOutput, ShellCommandError> {
    let mut process = shell_command(&command, shell.as_deref());
    process
        .current_dir(working_directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = spawn_shell(&mut process)?;

    let stdout = child
        .child
        .stdout
        .take()
        .ok_or(ShellCommandError::ExplicitlyPipedHandle(
            "stdout".to_string(),
        ))?;
    let stderr = child
        .child
        .stderr
        .take()
        .ok_or(ShellCommandError::ExplicitlyPipedHandle(
            "stderr".to_string(),
        ))?;

    // Read both pipes concurrently so a verbose stderr stream cannot block
    // a script whose stdout is being displayed in the transcript.
    let stdout_task = tokio::spawn(capture_pipe(stdout, stream_limit, output));
    let stderr_task = tokio::spawn(capture_pipe(stderr, stream_limit, None));

    let status = match tokio::time::timeout(Duration::from_secs(timeout), child.child.wait()).await
    {
        Ok(Ok(status)) => status,

        Ok(Err(error)) => {
            return Err(ShellCommandError::CommandFailedToWait(error));
        },

        Err(_) => {
            child
                .terminate()
                .map_err(|error| ShellCommandError::FailedToKillTimedoutCommand(error))?;

            child
                .child
                .wait()
                .await
                .map_err(|error| ShellCommandError::FailedToReapTimedoutCommand(error))?;

            let _ = stdout_task.await;
            let _ = stderr_task.await;

            return Err(ShellCommandError::CommandTimedOut(timeout));
        },
    };

    let stdout = stdout_task
        .await
        .map_err(|error| ShellCommandError::FailedToCaptureHandle("stdout".to_string(), error))?
        .map_err(|error| ShellCommandError::CapturedStreamError("stdout".to_string(), error))?;
    let stderr = stderr_task
        .await
        .map_err(|error| ShellCommandError::FailedToCaptureHandle("stderr".to_string(), error))?
        .map_err(|error| ShellCommandError::CapturedStreamError("stderr".to_string(), error))?;

    child.finished();

    let stdout = format_captured_stream("stdout", stdout);
    let stderr = format_captured_stream("stderr", stderr);

    let mut content = String::new();

    match status.success() {
        true => content.push_str(&stdout),
        false => content.push_str(&stderr),
    };

    if content.is_empty() {
        content.push_str("Command exited successfully");
    }

    Ok(ShellCommandOutput {
        content,
        success: status.success(),
    })
}

#[cfg(unix)]
/// Build a `Command` that runs the given shell command string.
fn shell_command(
    command: &str,
    shell: Option<&[String]>,
) -> Command {
    let shell = shell
        .map(|shell| shell.to_vec())
        .unwrap_or_else(|| vec![std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string())]);

    let mut process = Command::new(&shell[0]);
    process.args(&shell[1..]).arg("-c").arg(command);
    process
}

#[cfg(windows)]
/// Build a `Command` that runs the given shell command string.
fn shell_command(
    command: &str,
    shell: Option<&[String]>,
) -> Command {
    let shell = shell.map(|shell| shell.to_vec()).unwrap_or_else(|| {
        vec![std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())]
    });

    let is_cmd = Path::new(&shell[0])
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd"));

    let mut process = Command::new(&shell[0]);
    process
        .args(&shell[1..])
        .arg(if is_cmd { "/C" } else { "-c" })
        .arg(command);
    process
}

#[cfg(not(any(unix, windows)))]
/// Build a `Command` that runs the given shell command string.
fn shell_command(
    command: &str,
    _shell: Option<&[String]>,
) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

/// Captured output from a script stream, including truncation status.
#[derive(Debug)]
struct CapturedStream {
    /// Raw bytes retained from the stream.
    bytes: Vec<u8>,

    /// Whether the stream exceeded its configured output limit.
    truncated: bool,
}

/// Capture a pipe while optionally sending each retained chunk to the TUI.
async fn capture_pipe<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    output: Option<ToolOutputSink>,
) -> Result<CapturedStream, anyhow::Error> {
    let mut bytes = Vec::with_capacity(limit);
    let mut buffer = [0_u8; 8_192];
    let mut truncated = false;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }

        let remaining = limit.saturating_sub(bytes.len());
        let retained = read.min(remaining);
        bytes.extend_from_slice(&buffer[..retained]);

        if let Some(output) = &output {
            output.emit(String::from_utf8_lossy(&buffer[..retained]).to_string());
        }

        truncated |= retained < read;
    }

    Ok(CapturedStream { bytes, truncated })
}

/// Convert captured stream bytes to text and append a truncation notice.
fn format_captured_stream(
    name: &str,
    captured: CapturedStream,
) -> String {
    let mut output = String::from_utf8_lossy(&captured.bytes).to_string();

    if captured.truncated {
        output.push_str(&format!(
            "\n[{name} truncated after {} bytes]",
            captured.bytes.len()
        ));
    }

    output
}
