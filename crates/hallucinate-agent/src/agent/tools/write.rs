//! Write tool: write file contents atomically.

use std::{future::Future, io::Write, pin::Pin};

use serde_json::json;

use super::{
    limits::{ToolLimits, run_blocking},
    paths::resolve_path,
    types::Tool,
};

/// File writing tool.
///
/// Writes content to a file atomically using a temp file.
#[derive(Debug, Clone)]
pub struct WriteTool {
    limits: ToolLimits,
}

impl Default for WriteTool {
    fn default() -> Self {
        Self {
            limits: ToolLimits::default(),
        }
    }
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

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<super::types::ToolResult, anyhow::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let path_str = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: path"))?;

            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: content"))?;

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

            let path = resolve_path(path_str, cwd);
            let display_path = crate::config::paths::display_path(&path, cwd.unwrap_or("."));
            let content = content.to_string();

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

                Ok(super::types::ToolResult::success(
                    "write",
                    "write",
                    format!(
                        "Successfully wrote {} bytes to {}",
                        content.len(),
                        display_path
                    ),
                ))
            })
            .await
        })
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
}
