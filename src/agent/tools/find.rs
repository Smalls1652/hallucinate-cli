//! Find tool: glob-based file search.

use std::{future::Future, path::{Path, PathBuf}, pin::Pin};

use serde_json::json;

use super::types::Tool;

/// Glob-based file search tool.
///
/// Searches for files matching a glob pattern.
#[derive(Debug, Clone)]
pub struct FindTool {
    /// Maximum number of results to return.
    max_results: usize,
}

impl Default for FindTool {
    fn default() -> Self {
        Self { max_results: 100 }
    }
}

impl FindTool {
    /// Create a new FindTool with default settings.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Search for files matching a glob pattern. Supports patterns like '*.rs', 'src/**/*.ts', 'tests/**/test_*'. Returns a list of matching file paths. Use this instead of running 'find' or 'ls' commands."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to search for (e.g., '*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<super::types::ToolResult, anyhow::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let pattern = arguments
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: pattern"))?;

            let project_dir = cwd.unwrap_or(".");
            let search_path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let search_path = resolve_path(search_path, project_dir);

            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.max_results as u64) as usize;

            let full_pattern = search_path.join(pattern);
            let full_pattern_string = full_pattern.to_string_lossy().into_owned();
            let expanded_pattern = shellexpand::full(&full_pattern_string)
                .unwrap_or_else(|_| std::borrow::Cow::Borrowed(&full_pattern_string));

            let matches: Vec<_> = glob::glob(expanded_pattern.as_ref())?
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.is_file())
                .map(|entry| crate::config::paths::display_path(entry, project_dir))
                .take(max_results)
                .collect();

            if matches.is_empty() {
                Ok(super::types::ToolResult::success(
                    "find",
                    "find",
                    format!("No files matching pattern: {}", pattern),
                ))
            } else {
                let result = format!(
                    "Found {} file(s) matching '{}':\n\n{}",
                    matches.len(),
                    pattern,
                    matches.join("\n")
                );
                Ok(super::types::ToolResult::success("find", "find", result))
            }
        })
    }
}

fn resolve_path(
    path_str: &str,
    cwd: &str,
) -> PathBuf {
    let expanded = shellexpand::full(path_str).unwrap_or(std::borrow::Cow::Borrowed(path_str));
    let path = Path::new(expanded.as_ref());
    if path.is_relative() {
        Path::new(cwd).join(path)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_tool_exists() {
        let tool = FindTool::new();
        assert_eq!(tool.name(), "find");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_find_missing_pattern() {
        let tool = FindTool::new();
        let args = json!({});
        let result = futures::executor::block_on(tool.execute(&args, None));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_nonexistent_pattern() {
        let tool = FindTool::new();
        let args = json!({ "pattern": "**/nonexistent_xyz_123.rs" });
        let result = futures::executor::block_on(tool.execute(&args, None)).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("No files"));
    }
}
