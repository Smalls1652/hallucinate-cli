//! Bash tool: execute shell commands.

use std::{future::Future, pin::Pin};

use serde_json::json;
use tokio::process::Command;

use super::types::Tool;

/// Shell command execution tool.
///
/// Runs commands in a shell subprocess with configurable timeout and shell.
/// Output is captured and returned as text.
#[derive(Debug, Clone)]
pub struct BashTool {
    /// Maximum execution time in milliseconds.
    timeout_ms: u64,
    /// Explicit shell executable. `None` uses the user's environment default.
    shell: Option<String>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            timeout_ms: 60_000,
            shell: None,
        }
    }
}

impl BashTool {
    /// Create a new BashTool with the given timeout.
    pub fn new() -> Self {
        Self::default()
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
        "Execute a shell command or script and return the output. Commands are interpreted by the user's default shell, so pipes, quoting, &&, ||, redirects, variables, and other shell syntax are supported. Use this for running any shell command, including git, cargo, make, curl, etc."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command or script to execute using the user's default shell"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Maximum execution time in milliseconds (default: 60000)"
                }
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
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: command"))?;

            let timeout_ms = arguments
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.timeout_ms);

            if command.trim().is_empty() {
                return Ok(super::types::ToolResult::error(
                    "bash",
                    "bash",
                    "Empty command",
                ));
            }

            // Pass the complete command to a shell instead of splitting it into
            // whitespace-delimited arguments. The latter loses quoting and
            // makes shell operators (for example `|` and `&&`) ordinary
            // arguments to the first program.
            let mut cmd = shell_command(command, self.shell.as_deref());

            // Set working directory if provided
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }

            // Capture stdout and stderr
            let output =
                tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), cmd.output())
                    .await;

            match output {
                Ok(Ok(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    let mut result = stdout;
                    if !stderr.is_empty() && output.status.success() {
                        // Some tools write warnings to stderr even on success
                        result.push_str("\n[stderr:]\n");
                        result.push_str(&stderr);
                    }

                    if !output.status.success() {
                        let status = output.status;
                        let mut error_msg = format!("Command exited with status: {status}");
                        
                        if !stderr.is_empty() {
                            error_msg.push_str("\n[stderr:]\n");
                            error_msg.push_str(&stderr);
                        }
                        
                        return Ok(super::types::ToolResult::error(
                            "bash",
                            "bash",
                            format!("{}\n\n[stdout:]\n{}", error_msg, result),
                        ));
                    }

                    // Truncate very long output
                    let truncated = truncate_output(&result, 10_000);
                    
                    Ok(super::types::ToolResult::success("bash", "bash", truncated))
                },
                
                Ok(Err(e)) => Ok(super::types::ToolResult::error(
                    "bash",
                    "bash",
                    format!("Failed to execute command: {e}"),
                )),

                Err(_) => Ok(super::types::ToolResult::error(
                    "bash",
                    "bash",
                    format!("Command timed out after {}ms", timeout_ms),
                )),
            }
        })
    }
}

/// Build a command that evaluates the input as a shell script.
///
/// `SHELL` is the conventional Unix environment variable for the user's
/// preferred shell. Falling back to `sh` keeps the tool usable in processes
/// that do not provide it. On Windows, use `SHELL` when available (for
/// example, Git Bash); otherwise use `COMSPEC` and its `/C` switch.
#[cfg(unix)]
fn shell_command(
    command: &str,
    configured_shell: Option<&str>,
) -> Command {
    let shell = configured_shell
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("SHELL"))
        .unwrap_or_else(|| "sh".into());
    
    let mut cmd = Command::new(shell);
    cmd.arg("-c").arg(command);
    
    cmd
}

#[cfg(windows)]
fn shell_command(
    command: &str,
    configured_shell: Option<&str>,
) -> Command {
    if let Some(shell) = configured_shell
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("SHELL"))
    {
        let is_cmd = std::path::Path::new(&shell)
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd"));
        
        let mut cmd = Command::new(shell);
        cmd.arg(if is_cmd { "/C" } else { "-c" }).arg(command);
        
        cmd
    } else {
        let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        
        let mut cmd = Command::new(shell);
        cmd.arg("/C").arg(command);
        
        cmd
    }
}

#[cfg(not(any(unix, windows)))]
fn shell_command(
    command: &str,
    _configured_shell: Option<&str>,
) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    
    cmd
}

/// Truncate output to a maximum length, adding a note if truncated.
fn truncate_output(
    output: &str,
    max_len: usize,
) -> String {
    if output.len() <= max_len {
        output.to_string()
    } else {
        let truncated = &output[..max_len];
        
        format!(
            "{}\n\n[Output truncated: {} total characters]",
            truncated,
            output.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_tool_exists() {
        let tool = BashTool::new();
        
        assert_eq!(tool.name(), "bash");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_echo_command() {
        // Just verify the tool exists with correct name
        let tool = BashTool::new();
        
        assert_eq!(tool.name(), "bash");
        assert!(!tool.description().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_configured_shell_is_used() {
        let tool = BashTool::new().with_shell(Some("/bin/sh".to_string()));
        let arguments = json!({ "command": "printf configured" });

        let result = tool.execute(&arguments, None).await.unwrap();

        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.content, "configured");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_shell_syntax_is_supported() {
        let tool = BashTool::new();
        let arguments = json!({
            "command": "printf '%s\\n' \"hello world\" | tr '[:lower:]' '[:upper:]' && printf 'success\\n' || printf 'failure\\n'"
        });

        let result = tool.execute(&arguments, None).await.unwrap();

        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.content, "HELLO WORLD\nsuccess\n");
    }

    #[test]
    fn test_truncate_output() {
        let short = "hello";
        assert_eq!(truncate_output(short, 100), "hello");

        let long = "a".repeat(200);
        let truncated = truncate_output(&long, 100);
        
        assert!(truncated.len() < long.len());
        assert!(truncated.contains("truncated"));
    }
}
