//! Edit tool: intelligent text editing with diff verification.

use std::{future::Future, io::Write, pin::Pin};

use serde_json::json;

use super::{paths::resolve_path, types::Tool};

/// Intelligent edit tool for modifying file contents.
///
/// This tool performs targeted text replacements in files with verification.
/// It finds the exact text to replace and replaces it with new content using
/// Rust's `replace` function which replaces all occurrences by default.
///
/// # Usage
///
/// Provide the path to a file, the exact text to find (old_text), and the
/// replacement text. The tool verifies that old_text exists in the file before
/// performing the replacement.
#[derive(Debug, Clone)]
pub struct EditTool;

impl Default for EditTool {
    fn default() -> Self {
        Self
    }
}

impl EditTool {
    /// Create a new EditTool.
    pub fn new() -> Self {
        Self
    }
}

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing text. Finds the exact text to replace and replaces it with new content. Use this for targeted edits rather than rewriting entire files. The old_text must match exactly (including whitespace and indentation). If there are multiple matches, use old_text with enough context to make it unique."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "New text to replace with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    /// Execute the edit tool with given arguments.
    ///
    /// # Parameters
    /// - `arguments`: JSON-serialized tool arguments containing:
    ///   - `path`: Path to the file to edit (string)
    ///   - `old_text`: Exact text to find and replace (string)
    ///   - `new_text`: New text to replace with (string)
    /// - `cwd`: Optional explicit working directory for path resolution
    ///
    /// # Returns
    /// - `ToolResult::success` if the replacement was successful, including the
    ///   message
    /// - `ToolResult::error` if:
    ///   - Required arguments are missing
    ///   - The text to replace is not found in the file
    ///   - The file cannot be read or written
    ///
    /// # Errors
    /// Returns an error if any step fails (reading file, creating directories,
    /// writing file).
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

            let old_text = arguments
                .get("old_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: old_text"))?;

            let new_text = arguments
                .get("new_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: new_text"))?;

            let path = resolve_path(path_str, cwd);
            let display_path = crate::config::paths::display_path(&path, cwd.unwrap_or("."));

            let content = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("Failed to read file {display_path}: {e}"))?;

            if !content.contains(old_text) {
                return Ok(super::types::ToolResult::error(
                    "edit",
                    "edit",
                    format!(
                        "Text not found in file.\n\nFile: {display_path}\n\nSearched for:\n{old_text}\n\nFile content:\n{content}"
                    ),
                ));
            }

            let count = content.matches(old_text).count();
            let new_content = content.replace(old_text, new_text);

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let temp_path = path.with_extension(format!(".tmp.{}", std::process::id()));
            {
                let mut file = std::fs::File::create(&temp_path)?;
                file.write_all(new_content.as_bytes())?;
                file.flush()?;
            }
            std::fs::rename(&temp_path, &path)?;

            Ok(super::types::ToolResult::success(
                "edit",
                "edit",
                format!(
                    "Successfully replaced {} occurrence(s) in {}",
                    count, display_path
                ),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn test_edit_tool_exists() {
        let tool = EditTool::new();

        assert_eq!(tool.name(), "edit");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_edit_missing_fields() {
        let tool = EditTool::new();
        let args = json!({ "path": "/tmp/test.txt" });
        let result = futures::executor::block_on(tool.execute(&args, None));

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_edit_text_not_found() {
        let tool = EditTool::new();
        let test_path = std::env::temp_dir().join(format!(
            "hallucinate_test_edit_{}.txt",
            uuid::Uuid::new_v4()
        ));

        fs::write(&test_path, "hello world").unwrap();

        let args = json!({
            "path": test_path.to_string_lossy(),
            "old_text": "nonexistent",
            "new_text": "replaced"
        });

        let result = futures::executor::block_on(tool.execute(&args, None)).unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));

        assert_eq!(fs::read_to_string(&test_path).unwrap(), "hello world");

        fs::remove_file(test_path).ok();
    }

    #[tokio::test]
    async fn test_edit_replace_text() {
        let tool = EditTool::new();
        let test_path = std::env::temp_dir().join(format!(
            "hallucinate_test_edit_{}.txt",
            uuid::Uuid::new_v4()
        ));

        fs::write(&test_path, "hello world").unwrap();

        let args = json!({
            "path": test_path.to_string_lossy(),
            "old_text": "world",
            "new_text": "universe"
        });

        let result = futures::executor::block_on(tool.execute(&args, None)).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Successfully replaced"));

        assert_eq!(fs::read_to_string(&test_path).unwrap(), "hello universe");

        fs::remove_file(test_path).ok();
    }
}
