//! Bash tool: bounded shell command execution.

use std::{future::Future, pin::Pin, process::Stdio, time::Duration};

use serde_json::json;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use super::{
    limits::{ToolLimits, truncate_utf8},
    types::Tool,
};

/// Shell command execution tool.
#[derive(Debug, Clone)]
pub struct BashTool {
    /// Maximum execution time in seconds.
    timeout: u64,
    limits: ToolLimits,
    /// Explicit shell executable. `None` uses the user's environment default.
    shell: Option<String>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            timeout: 60,
            limits: ToolLimits::default(),
            shell: None,
        }
    }
}

impl BashTool {
    /// Create a new `BashTool` with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the timeout (in seconds) for command execution.
    pub fn with_timeout(
        mut self,
        timeout: u64,
    ) -> Self {
        self.timeout = timeout;
        self
    }

    /// Configure resource limits for this tool.
    pub fn with_limits(
        mut self,
        limits: ToolLimits,
    ) -> Self {
        self.limits = limits;
        self
    }

    /// Configure the shell executable used for commands.
    pub fn with_shell(
        mut self,
        shell: Option<String>,
    ) -> Self {
        self.shell = shell;
        self
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command or script and return bounded stdout and stderr. Shell syntax including pipes, redirects, quoting, &&, and || is supported. Timed-out commands are killed before a timeout result is returned."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command or script" },
                "timeout": { "type": "integer", "description": "Maximum execution time in seconds (default: 60)" }
            },
            "required": ["command"]
        })
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<super::types::ToolResult, anyhow::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let command = arguments
                .get("command")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: command"))?;
            if command.trim().is_empty() {
                return Ok(super::types::ToolResult::error(
                    "bash",
                    "bash",
                    "Empty command",
                ));
            }
            let timeout = arguments
                .get("timeout")
                .and_then(|value| value.as_u64())
                .unwrap_or(self.timeout);
            let mut command_process = shell_command(command, self.shell.as_deref());
            if let Some(cwd) = cwd {
                command_process.current_dir(cwd);
            }
            command_process
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            let mut child = command_process
                .spawn()
                .map_err(|error| anyhow::anyhow!("Failed to execute command: {error}"))?;
            let stdout = child.stdout.take().expect("stdout was explicitly piped");
            let stderr = child.stderr.take().expect("stderr was explicitly piped");
            let stream_limit = self.limits.max_shell_stream_bytes;
            let stdout_task = tokio::spawn(capture_pipe(stdout, stream_limit));
            let stderr_task = tokio::spawn(capture_pipe(stderr, stream_limit));

            let status = match tokio::time::timeout(Duration::from_secs(timeout), child.wait())
                .await
            {
                Ok(Ok(status)) => status,
                Ok(Err(error)) => {
                    return Err(anyhow::anyhow!("Failed while waiting for command: {error}"));
                },
                Err(_) => {
                    // `kill_on_drop` is a fallback; await explicit cleanup so a
                    // direct child cannot survive a timeout result.
                    if let Err(error) = child.kill().await {
                        if error.kind() != std::io::ErrorKind::InvalidInput {
                            return Err(anyhow::anyhow!(
                                "Failed to kill timed-out command: {error}"
                            ));
                        }
                    }
                    child.wait().await.map_err(|error| {
                        anyhow::anyhow!("Failed to reap timed-out command: {error}")
                    })?;
                    let _ = stdout_task.await;
                    let _ = stderr_task.await;
                    return Ok(super::types::ToolResult::error(
                        "bash",
                        "bash",
                        format!("Command timed out after {timeout}s"),
                    ));
                },
            };
            let stdout = stdout_task
                .await
                .map_err(|error| anyhow::anyhow!("stdout capture task failed: {error}"))??;
            let stderr = stderr_task
                .await
                .map_err(|error| anyhow::anyhow!("stderr capture task failed: {error}"))??;

            let stdout = format_captured_stream("stdout", stdout);
            let stderr = format_captured_stream("stderr", stderr);
            if !status.success() {
                let mut content = format!("Command exited with status: {status}");
                if !stderr.is_empty() {
                    content.push_str("\n[stderr:]\n");
                    content.push_str(&stderr);
                }
                content.push_str("\n\n[stdout:]\n");
                content.push_str(&stdout);
                return Ok(super::types::ToolResult::error("bash", "bash", content));
            }

            let mut content = stdout;
            if !stderr.is_empty() {
                content.push_str("\n[stderr:]\n");
                content.push_str(&stderr);
            }
            Ok(super::types::ToolResult::success(
                "bash",
                "bash",
                truncate_utf8(&content, self.limits.max_tool_result_bytes, "Output"),
            ))
        })
    }
}

#[derive(Debug)]
struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn capture_pipe<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
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
        truncated |= retained < read;
    }
    Ok(CapturedStream { bytes, truncated })
}

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

#[cfg(unix)]
fn shell_command(
    command: &str,
    configured_shell: Option<&str>,
) -> Command {
    let shell = configured_shell
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("SHELL"))
        .unwrap_or_else(|| "sh".into());
    let mut command_process = Command::new(shell);
    command_process.arg("-c").arg(command);
    command_process
}

#[cfg(windows)]
fn shell_command(
    command: &str,
    configured_shell: Option<&str>,
) -> Command {
    let shell = configured_shell
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("SHELL"))
        .or_else(|| std::env::var_os("COMSPEC"))
        .unwrap_or_else(|| "cmd.exe".into());
    let is_cmd = std::path::Path::new(&shell)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd"));
    let mut command_process = Command::new(shell);
    command_process
        .arg(if is_cmd { "/C" } else { "-c" })
        .arg(command);
    command_process
}

#[cfg(not(any(unix, windows)))]
fn shell_command(
    command: &str,
    _configured_shell: Option<&str>,
) -> Command {
    let mut command_process = Command::new("sh");
    command_process.arg("-c").arg(command);
    command_process
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn captures_both_pipes_without_unbounded_returned_output() {
        let mut limits = ToolLimits::default();
        limits.max_shell_stream_bytes = 32;
        let result = BashTool::new().with_limits(limits).execute(&json!({ "command": "i=0; while [ $i -lt 100 ]; do printf x; printf y >&2; i=$((i+1)); done" }), None).await.unwrap();
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("stdout truncated"));
        assert!(result.content.contains("stderr truncated"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kills_a_timed_out_direct_child() {
        let result = BashTool::new()
            .execute(&json!({ "command": "sleep 2", "timeout": 1 }), None)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("timed out"));
    }

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        assert!(truncate_utf8("aé", 2, "Output").starts_with('a'));
    }
}
