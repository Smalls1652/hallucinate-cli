//! Edit tool: intelligent text editing with per-edit diff output.

use std::io::Write;

use serde_json::json;

use super::{
    error::{EditsField, InternalToolError},
    limits::run_blocking,
    paths::resolve_path,
    types::{AsyncToolExecutionResult, Tool},
};

/// A single exact-text replacement requested from the edit tool.
#[derive(Debug, Clone)]
struct TextEdit {
    /// Text to find and replace.
    old_text: String,

    /// Replacement text.
    new_text: String,
}

/// Intelligent edit tool for modifying file contents.
///
/// This tool performs one or more targeted replacements in a file. Every
/// replacement is verified against the progressively edited content before the
/// file is written, so a failed edit leaves the original file unchanged.
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
        "Edit a file by applying one or more exact text replacements. Provide a path and an edits array, where each edit has old_text and new_text. Edits are applied in order and are all verified before the file is written. The result shows a diff for every applied edit. Use this for targeted edits rather than rewriting entire files. Each old_text must match exactly, including whitespace and indentation; include enough context to make it unique when needed."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "edits": {
                    "type": "array",
                    "minItems": 1,
                    "description": "Exact text replacements to apply in order",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": {
                                "type": "string",
                                "description": "Exact text to find and replace"
                            },
                            "new_text": {
                                "type": "string",
                                "description": "New text to replace with"
                            }
                        },
                        "required": ["old_text", "new_text"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> AsyncToolExecutionResult<'a> {
        Box::pin(async move {
            let path_str = required_string(arguments, "path")?;
            let edits = parse_edits(arguments)?;

            let path = resolve_path(path_str, cwd);
            let display_path = crate::config::display_path(&path, cwd.unwrap_or("."));

            run_blocking("edit", move || {
                let content = std::fs::read_to_string(&path).map_err(|error| {
                    InternalToolError::FailedToReadFile(display_path.clone(), error)
                })?;

                let mut new_content = content;
                let mut replacement_count = 0;
                let mut edit_diffs = Vec::with_capacity(edits.len());

                for (index, edit) in edits.iter().enumerate() {
                    if !new_content.contains(&edit.old_text) {
                        return Ok(super::types::ToolResult::error(
                            "edit",
                            "edit",
                            format!(
                                "Text for edit {} not found in file: {display_path}\n\nSearched for:\n{}",
                                index + 1,
                                edit.old_text,
                            ),
                        ));
                    }

                    let count = new_content.matches(&edit.old_text).count();
                    replacement_count += count;
                    edit_diffs.push(format_edit_diff(index + 1, count, edit));
                    new_content = new_content.replace(&edit.old_text, &edit.new_text);
                }

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
                        "Successfully applied {} edit(s), replacing {replacement_count} occurrence(s) in {display_path}\n\n{}",
                        edits.len(),
                        edit_diffs.join("\n"),
                    ),
                ))
            })
            .await
        })
    }
}

/// Format the exact replacement made by one edit for the terminal display.
fn format_edit_diff(
    edit_number: usize,
    occurrence_count: usize,
    edit: &TextEdit,
) -> String {
    format!(
        "Edit {edit_number} · {occurrence_count} occurrence(s)\n{}{}",
        diff_lines("  − ", &edit.old_text),
        diff_lines("  + ", &edit.new_text),
    )
}

/// Prefix every line of text with its terminal diff marker.
fn diff_lines(
    marker: &str,
    text: &str,
) -> String {
    if text.is_empty() {
        return format!("{marker}<empty>\n");
    }

    text.lines()
        .map(|line| format!("{marker}{line}\n"))
        .collect()
}

/// Return a required string property from a tool argument object.
fn required_string<'a>(
    arguments: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, InternalToolError> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| InternalToolError::MissingRequiredField(field.to_string()))
}

/// Parse ordered edit requests, accepting the former single-edit shape during
/// the transition to the `edits` array API.
fn parse_edits(arguments: &serde_json::Value) -> Result<Vec<TextEdit>, InternalToolError> {
    let Some(edits_value) = arguments.get("edits") else {
        return Ok(vec![TextEdit {
            old_text: required_string(arguments, "old_text")?.to_string(),
            new_text: required_string(arguments, "new_text")?.to_string(),
        }]);
    };

    let edits = edits_value
        .as_array()
        .ok_or_else(|| InternalToolError::FieldEditsNotAnArray)?;

    if edits.is_empty() {
        return Err(InternalToolError::FieldEditsMissingEdits);
    }

    edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            let old_text = edit
                .get("old_text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    InternalToolError::EditsMissingRequiredField(index, EditsField::OldText)
                })?;
            let new_text = edit
                .get("new_text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    InternalToolError::EditsMissingRequiredField(index, EditsField::NewText)
                })?;

            Ok(TextEdit {
                old_text: old_text.to_string(),
                new_text: new_text.to_string(),
            })
        })
        .collect()
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
        let result = tool.execute(&args, None).await;

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
            "edits": [{ "old_text": "nonexistent", "new_text": "replaced" }]
        });

        let result = tool.execute(&args, None).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
        assert_eq!(fs::read_to_string(&test_path).unwrap(), "hello world");

        fs::remove_file(test_path).ok();
    }

    #[tokio::test]
    async fn test_edit_applies_multiple_edits_in_order() {
        let tool = EditTool::new();
        let test_path = std::env::temp_dir().join(format!(
            "hallucinate_test_edit_{}.txt",
            uuid::Uuid::new_v4()
        ));

        fs::write(&test_path, "hello world").unwrap();

        let args = json!({
            "path": test_path.to_string_lossy(),
            "edits": [
                { "old_text": "world", "new_text": "rust" },
                { "old_text": "hello rust", "new_text": "hello universe" }
            ]
        });

        let result = tool.execute(&args, None).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Successfully applied 2 edit(s)"));
        assert!(result.content.contains("Edit 1 · 1 occurrence(s)"));
        assert!(result.content.contains("  − world\n  + rust"));
        assert!(result.content.contains("Edit 2 · 1 occurrence(s)"));
        assert!(
            result
                .content
                .contains("  − hello rust\n  + hello universe")
        );
        assert_eq!(fs::read_to_string(&test_path).unwrap(), "hello universe");

        fs::remove_file(test_path).ok();
    }

    #[tokio::test]
    async fn test_edit_does_not_write_when_a_later_edit_fails() {
        let tool = EditTool::new();
        let test_path = std::env::temp_dir().join(format!(
            "hallucinate_test_edit_{}.txt",
            uuid::Uuid::new_v4()
        ));

        fs::write(&test_path, "hello world").unwrap();

        let args = json!({
            "path": test_path.to_string_lossy(),
            "edits": [
                { "old_text": "hello", "new_text": "goodbye" },
                { "old_text": "missing", "new_text": "present" }
            ]
        });

        let result = tool.execute(&args, None).await.unwrap();
        assert!(result.is_error);
        assert_eq!(fs::read_to_string(&test_path).unwrap(), "hello world");

        fs::remove_file(test_path).ok();
    }

    #[tokio::test]
    async fn test_edit_accepts_the_legacy_single_edit_shape() {
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

        let result = tool.execute(&args, None).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(fs::read_to_string(&test_path).unwrap(), "hello universe");

        fs::remove_file(test_path).ok();
    }
}
