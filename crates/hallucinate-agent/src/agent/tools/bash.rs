//! Bash tool: bounded shell command execution.

use std::path::PathBuf;

use serde_json::json;

use super::{
    limits::truncate_utf8,
    types::{Tool, ToolOutputSink},
};
use crate::agent::tools::{
    BashToolConfig,
    ToolResult,
    error::InternalToolError,
    shell_exec::run_shell_command,
    types::{AsyncToolExecutionResult, ToolExecutionResult},
};

/// Shell command execution tool.
#[derive(Debug, Clone)]
pub struct BashTool {
    /// Config for the tool.
    config: BashToolConfig,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            config: BashToolConfig::default(),
        }
    }
}

impl BashTool {
    /// Create a new [`BashTool`].
    pub fn new(config: Option<BashToolConfig>) -> Self {
        let config = match config {
            Some(value) => value,
            None => BashToolConfig::default(),
        };

        Self { config }
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

    fn execute_with_output<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
        output: Option<ToolOutputSink>,
    ) -> AsyncToolExecutionResult<'a> {
        Box::pin(self.execute_inner(arguments, cwd, output))
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> AsyncToolExecutionResult<'a> {
        Box::pin(self.execute_inner(arguments, cwd, None))
    }
}

impl BashTool {
    /// Execute a shell command, returning the result with captured stdout and
    /// stderr.
    async fn execute_inner(
        &self,
        arguments: &serde_json::Value,
        cwd: Option<&str>,
        output: Option<ToolOutputSink>,
    ) -> ToolExecutionResult {
        let command = arguments
            .get("command")
            .and_then(|value| value.as_str())
            .ok_or_else(|| InternalToolError::MissingRequiredField("command".to_string()))?;

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
            .unwrap_or(self.config.timeout);

        let execution_directory = cwd
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|error| InternalToolError::FailedToGetCurrentDirectory(error))?;

        let command_output = match run_shell_command(
            self.config.shell.as_deref(),
            &command,
            &execution_directory,
            timeout,
            self.config.max_tool_result_bytes,
            output,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Ok(ToolResult::error("bash", "bash", error.to_string())),
        };

        Ok(ToolResult::success(
            "bash",
            "bash",
            truncate_utf8(
                &command_output.content,
                self.config.max_tool_result_bytes,
                "Output",
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn captures_both_pipes_without_unbounded_returned_output() {
        let result = BashTool::new(Some(BashToolConfig { max_tool_result_bytes: 32, ..Default::default() })).execute(&json!({ "command": "i=0; while [ $i -lt 100 ]; do printf x; printf y >&2; i=$((i+1)); done" }), None).await.unwrap();

        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("truncated"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn streams_output_before_the_command_finishes() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let output = ToolOutputSink::new(sender, "call-1", "bash");
        let tool = BashTool::default();
        let command = json!({ "command": "printf first; sleep 1; printf second" });

        let execution = tool.execute_with_output(&command, None, Some(output));
        tokio::pin!(execution);

        let first_output = tokio::select! {
            output = receiver.recv() => output.expect("output channel should remain open"),
            result = &mut execution => panic!("command finished before emitting output: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                panic!("first output should arrive before the command completes");
            },
        };
        assert_eq!(first_output.tool_call_id, "call-1");
        assert_eq!(first_output.output, "first");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut execution)
                .await
                .is_err()
        );

        let result = execution.await.unwrap();
        assert_eq!(result.content, "firstsecond");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kills_a_timed_out_direct_child() {
        let result = BashTool::default()
            .execute(&json!({ "command": "sleep 2", "timeout": 1 }), None)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_spawned_commands() {
        let marker = std::env::temp_dir().join(format!("bash-timeout-{}", uuid::Uuid::new_v4()));

        let command = json!({
            "command": format!("(sleep 2; printf orphan > '{}') & wait", marker.display()),
            "timeout": 1
        });

        let result = BashTool::default().execute(&command, None).await.unwrap();
        assert!(result.content.contains("timed out"));

        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert!(!marker.exists(), "background command survived timeout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_execution_kills_spawned_commands() {
        let marker = std::env::temp_dir().join(format!("bash-cancel-{}", uuid::Uuid::new_v4()));

        let command = json!({ "command": format!(
            "(sleep 1; printf orphan > '{}') & printf ready; wait",
            marker.display()
        ) });

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let output = ToolOutputSink::new(sender, "call-1", "bash");

        let tool = BashTool::default();

        let mut execution = Box::pin(tool.execute_with_output(&command, None, Some(output)));

        tokio::select! {
            output = receiver.recv() => assert_eq!(output.unwrap().output, "ready"),
            result = &mut execution => panic!("command finished early: {result:?}"),
        }

        drop(execution);

        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert!(!marker.exists(), "background command survived cancellation");
    }

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        assert!(truncate_utf8("aé", 2, "Output").starts_with('a'));
    }
}
