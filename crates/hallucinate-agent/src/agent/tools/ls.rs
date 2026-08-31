//! Ls tool: directory listing.

use std::{future::Future, path::Path, pin::Pin};

use serde_json::json;

use super::types::Tool;

/// Directory listing tool.
///
/// Lists files and directories with optional details.
#[derive(Debug, Clone)]
pub struct LsTool {
    /// Maximum entries to return.
    max_entries: usize,
}

impl Default for LsTool {
    fn default() -> Self {
        Self { max_entries: 100 }
    }
}

impl LsTool {
    /// Create a new LsTool with default settings.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Resolve a path, handling tilde expansion and relative paths.
pub(crate) fn resolve_path(
    path_str: &str,
    cwd: Option<&str>,
) -> std::path::PathBuf {
    let expanded = shellexpand::full(path_str).unwrap_or(std::borrow::Cow::Borrowed(path_str));

    let path = Path::new(expanded.as_ref());
    if path.is_relative() {
        if let Some(cwd) = cwd {
            return Path::new(cwd).join(path);
        }

        return std::env::current_dir().unwrap_or_default().join(path);
    }

    path.to_path_buf()
}

impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List files and directories. Shows file names, types, and sizes. Supports recursive listing. Use this instead of running 'ls' commands."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to list (default: current directory)"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "List recursively (default: false)"
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum number of entries (default: 100)"
                }
            },
            "required": []
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
                .unwrap_or(".")
                .to_string();

            let recursive = arguments
                .get("recursive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let max_entries = arguments
                .get("max_entries")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.max_entries as u64) as usize;

            let path = resolve_path(&path_str, cwd);
            let display_path = crate::config::paths::display_path(&path, cwd.unwrap_or("."));

            if !path.exists() {
                return Ok(super::types::ToolResult::error(
                    "ls",
                    "ls",
                    format!("Path not found: {display_path}"),
                ));
            }

            let mut entries: Vec<String> = Vec::new();

            if recursive {
                let mut dir_stack = vec![path.clone()];

                while let Some(current) = dir_stack.pop() {
                    if entries.len() >= max_entries {
                        break;
                    }

                    if let Ok(read_dir) = std::fs::read_dir(&current) {
                        let mut child_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
                        child_entries.sort_by_key(|e| e.path());

                        for entry in child_entries {
                            if entries.len() >= max_entries {
                                break;
                            }

                            let entry_path = entry.path();
                            let metadata = entry.metadata().ok();
                            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                            let is_file = metadata.as_ref().map(|m| m.is_file()).unwrap_or(false);
                            let size = metadata.map(|m| m.len()).unwrap_or(0);

                            let entry_type = if is_dir {
                                "dir"
                            } else if is_file {
                                "file"
                            } else {
                                "other"
                            };

                            entries.push(format!(
                                "{:<6} {:>10} {}",
                                entry_type,
                                format_size(size),
                                crate::config::paths::display_path(&entry_path, cwd.unwrap_or("."),)
                            ));

                            if is_dir {
                                dir_stack.push(entry_path);
                            }
                        }
                    }
                }
            } else {
                let read_dir = std::fs::read_dir(&path)?;
                let mut child_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
                child_entries.sort_by_key(|e| e.file_name());

                for entry in child_entries.into_iter().take(max_entries) {
                    let entry_path = entry.path();
                    let metadata = entry.metadata().ok();
                    let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let is_file = metadata.as_ref().map(|m| m.is_file()).unwrap_or(false);
                    let size = metadata.map(|m| m.len()).unwrap_or(0);

                    let entry_type = if is_dir {
                        "dir"
                    } else if is_file {
                        "file"
                    } else {
                        "other"
                    };

                    entries.push(format!(
                        "{:<6} {:>10} {}",
                        entry_type,
                        format_size(size),
                        entry_path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "?".to_string())
                    ));
                }
            }

            if entries.is_empty() {
                Ok(super::types::ToolResult::success(
                    "ls",
                    "ls",
                    format!("Directory is empty: {display_path}"),
                ))
            } else {
                let result = format!("Directory: {display_path}\n\n{}", entries.join("\n"));

                Ok(super::types::ToolResult::success("ls", "ls", result))
            }
        })
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ls_tool_exists() {
        let tool = LsTool::new();

        assert_eq!(tool.name(), "ls");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_ls_nonexistent_path() {
        let tool = LsTool::new();
        let args = json!({ "path": "/nonexistent/directory" });
        let result = futures::executor::block_on(tool.execute(&args, None)).unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_ls_current_dir() {
        let tool = LsTool::new();
        let args = json!({});
        let result = futures::executor::block_on(tool.execute(&args, None)).unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Directory:"));
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(1500), "1.5K");
        assert_eq!(format_size(1_500_000), "1.4M");
        assert_eq!(format_size(1_500_000_000), "1.4G");
    }
}
