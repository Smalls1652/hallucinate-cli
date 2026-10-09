//! Write tool: write file contents atomically.

use std::io::Write;

use serde_json::json;

use super::{
    limits::{ToolLimits, run_blocking, truncate_utf8},
    paths::resolve_path,
    types::{Tool, ToolOutputSink},
};
use crate::agent::tools::{
    error::InternalToolError,
    types::{AsyncToolExecutionResult, ToolExecutionResult},
};

/// File writing tool.
///
/// Writes content to a file atomically using a temp file.
#[derive(Debug, Clone, Default)]
pub struct WriteTool {
    /// Resource limits for output truncation and streaming.
    limits: ToolLimits,
}

impl WriteTool {
    /// Create a new WriteTool with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure resource limits for this tool.
    pub fn with_limits(
        mut self,
        limits: ToolLimits,
    ) -> Self {
        self.limits = limits;
        self
    }
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories if needed. Uses atomic write (temp file + rename) to prevent data corruption."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
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

impl WriteTool {
    async fn execute_inner(
        &self,
        arguments: &serde_json::Value,
        cwd: Option<&str>,
        output: Option<ToolOutputSink>,
    ) -> ToolExecutionResult {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| InternalToolError::MissingRequiredField("path".to_string()))?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| InternalToolError::MissingRequiredField("content".to_string()))?;

        if content.len() > self.limits.max_text_file_bytes {
            return Ok(super::types::ToolResult::error(
                "write",
                "write",
                format!(
                    "Content too large: {} bytes (max: {} bytes)",
                    content.len(),
                    self.limits.max_text_file_bytes
                ),
            ));
        }

        if let Some(output) = output {
            output.emit(truncate_utf8(
                content,
                self.limits.max_tool_result_bytes,
                "Written content",
            ));
        }

        let path = resolve_path(path_str, cwd);
        let content = content.to_string();
        let result_content = truncate_utf8(
            &content,
            self.limits.max_tool_result_bytes,
            "Written content",
        );

        run_blocking("write", move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let temp_path = path.with_extension(format!(".tmp.{}", std::process::id()));
            {
                let mut file = std::fs::File::create(&temp_path)?;
                file.write_all(content.as_bytes())?;
                file.flush()?;
            }
            std::fs::rename(&temp_path, &path)?;

            Ok(super::types::ToolResult {
                tool_call_id: "write".to_string(),
                tool_name: "write".to_string(),
                content: result_content,
                images: Vec::new(),
                is_error: false,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_write_tool_exists() {
        let tool = WriteTool::new();

        assert_eq!(tool.name(), "write");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_write_missing_path() {
        let tool = WriteTool::new();
        let args = json!({ "content": "hello" });
        let result = futures::executor::block_on(tool.execute(&args, None));

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_write_missing_content() {
        let tool = WriteTool::new();
        let args = json!({ "path": "/tmp/test.txt" });
        let result = futures::executor::block_on(tool.execute(&args, None));

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn streams_content_being_written() {
        let path = std::env::temp_dir().join(format!("hallucinate-write-{}", uuid::Uuid::new_v4()));
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let output = ToolOutputSink::new(sender, "call-1", "write");
        let args = json!({ "path": path, "content": "written content" });

        let result = WriteTool::new()
            .execute_with_output(&args, None, Some(output))
            .await
            .unwrap();
        let streamed_output = receiver.recv().await.expect("write should emit content");

        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.content, "written content");
        assert_eq!(streamed_output.tool_call_id, "call-1");
        assert_eq!(streamed_output.tool_name, "write");
        assert_eq!(streamed_output.output, "written content");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "written content");

        std::fs::remove_file(path).unwrap();
    }
}
