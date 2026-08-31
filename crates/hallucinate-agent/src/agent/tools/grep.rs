//! Grep tool: pattern-based file content search.

use std::{future::Future, path::Path, pin::Pin};

use serde_json::json;

use super::types::Tool;

/// Pattern-based file content search tool.
///
/// Searches for patterns in file contents using grep-like functionality.
#[derive(Debug, Clone)]
pub struct GrepTool {
    /// Maximum number of results to return.
    max_results: usize,
}

impl Default for GrepTool {
    fn default() -> Self {
        Self { max_results: 50 }
    }
}

impl GrepTool {
    /// Create a new GrepTool with default settings.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in file contents. Returns matching lines with file paths and line numbers. Supports regular expressions. Use this instead of running 'grep' or 'rg' commands."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Path to search in (file or directory, default: current directory)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs', '*.py')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 50)"
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

            let file_pattern = arguments
                .get("file_pattern")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.max_results as u64) as usize;

            let regex = regex::Regex::new(pattern)
                .map_err(|e| anyhow::anyhow!("Invalid regex pattern '{}': {}", pattern, e))?;

            let files = collect_files(&search_path, file_pattern.as_deref())?;

            let mut results: Vec<String> = Vec::new();

            for file_path in &files {
                if results.len() >= max_results {
                    break;
                }

                if is_binary_file(file_path) {
                    continue;
                }

                let content = match std::fs::read_to_string(file_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                for (line_num, line) in content.lines().enumerate() {
                    if results.len() >= max_results {
                        break;
                    }

                    if regex.is_match(line) {
                        results.push(format!(
                            "{}:{}: {}",
                            crate::config::paths::display_path(file_path, project_dir),
                            line_num + 1,
                            line.trim()
                        ));
                    }
                }
            }

            if results.is_empty() {
                Ok(super::types::ToolResult::success(
                    "grep",
                    "grep",
                    format!("No matches found for pattern: {}", pattern),
                ))
            } else {
                let result = format!(
                    "Found {} match(es) for '{}':\n\n{}",
                    results.len(),
                    pattern,
                    results.join("\n")
                );

                Ok(super::types::ToolResult::success("grep", "grep", result))
            }
        })
    }
}

fn resolve_path(
    path_str: &str,
    cwd: &str,
) -> std::path::PathBuf {
    let expanded = shellexpand::full(path_str).unwrap_or(std::borrow::Cow::Borrowed(path_str));

    let path = Path::new(expanded.as_ref());
    if path.is_relative() {
        Path::new(cwd).join(path)
    } else {
        path.to_path_buf()
    }
}

fn collect_files(
    search_path: &Path,
    file_pattern: Option<&str>,
) -> Result<Vec<std::path::PathBuf>, anyhow::Error> {
    let path = search_path;

    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if path.is_dir() {
        let pattern = if let Some(fp) = file_pattern {
            search_path
                .join("**")
                .join(fp)
                .to_string_lossy()
                .into_owned()
        } else {
            search_path
                .join("**")
                .join("*")
                .to_string_lossy()
                .into_owned()
        };

        let mut files = Vec::new();
        for entry in glob::glob(&pattern)?.flatten() {
            if entry.is_file() {
                files.push(entry);
            }
        }
        files.sort();

        Ok(files)
    } else {
        Ok(Vec::new())
    }
}

fn is_binary_file(path: &std::path::Path) -> bool {
    if let Ok(content) = std::fs::read(path) {
        content.iter().take(8192).any(|&b| b == 0)
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn test_grep_tool_exists() {
        let tool = GrepTool::new();

        assert_eq!(tool.name(), "grep");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_grep_missing_pattern() {
        let tool = GrepTool::new();
        let args = json!({});
        let result = futures::executor::block_on(tool.execute(&args, None));

        assert!(result.is_err());
    }

    #[test]
    fn test_grep_invalid_regex() {
        let tool = GrepTool::new();
        let args = json!({ "pattern": "[invalid" });
        let result = futures::executor::block_on(tool.execute(&args, None));

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_grep_simple_pattern() {
        let tool = GrepTool::new();
        let test_path = "/tmp/hallucinate_test_grep.txt";

        fs::write(test_path, "hello world\nfoo bar\nhello again").unwrap();

        let args = json!({
            "pattern": "hello",
            "path": test_path
        });

        let result = futures::executor::block_on(tool.execute(&args, None)).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("hello world"));
        assert!(result.content.contains("hello again"));

        fs::remove_file(test_path).ok();
    }
}
