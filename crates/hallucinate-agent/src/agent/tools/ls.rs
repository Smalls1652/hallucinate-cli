//! Ls tool: bounded deterministic directory listing.

use serde_json::json;

use super::{
    limits::{MAX_REQUESTED_RESULTS, parse_bounded_usize, run_blocking},
    paths::resolve_path,
    types::{AsyncToolExecutionResult, Tool},
    walker::{WalkControl, walk_entries},
};
use crate::config::LsToolConfig;

/// Directory listing tool.
#[derive(Debug, Clone)]
pub struct LsTool {
    /// Config for the tool.
    config: LsToolConfig,
}

impl Default for LsTool {
    fn default() -> Self {
        Self {
            config: LsToolConfig::default(),
        }
    }
}

impl LsTool {
    /// Create a new [`LsTool`].
    pub fn new(config: Option<LsToolConfig>) -> Self {
        let config = match config {
            Some(value) => value,
            None => LsToolConfig::default(),
        };

        Self { config }
    }
}

impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List files and directories with types and sizes. Recursive lists are deterministic, do not follow directory symlinks, skip generated-directory contents, and are bounded by entry limits."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to list (default: current directory)" },
                "recursive": { "type": "boolean", "description": "List recursively (default: false)" },
                "max_entries": { "type": "integer", "description": "Maximum entries (default: 100, maximum: 1000)" }
            },
            "required": []
        })
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> AsyncToolExecutionResult<'a> {
        Box::pin(async move {
            let path = resolve_path(
                arguments
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("."),
                cwd,
            );

            let display_path = crate::config::display_path(&path, cwd.unwrap_or("."));

            let recursive = arguments
                .get("recursive")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);

            let max_entries = parse_bounded_usize(
                arguments,
                "max_entries",
                self.config.max_results,
                MAX_REQUESTED_RESULTS,
            )?;

            let config = self.config.clone();

            let display_cwd = cwd.unwrap_or(".").to_string();

            run_blocking("ls", move || {
                if !path.exists() {
                    return Ok(super::types::ToolResult::error(
                        "ls",
                        "ls",
                        format!("Path not found: {display_path}"),
                    ));
                }

                if path.is_file() {
                    return Ok(super::types::ToolResult::success(
                        "ls",
                        "ls",
                        format_entry(&path, &display_cwd)?,
                    ));
                }

                let mut entries = Vec::new();

                let status = walk_entries(
                    &path,
                    recursive,
                    &config.max_directory_entries,
                    |entry_path, _| {
                        if entries.len() >= max_entries {
                            return WalkControl::Stop;
                        }

                        if let Ok(entry) = format_entry(&entry_path, &display_cwd) {
                            entries.push(entry);
                        }

                        if entries.len() >= max_entries {
                            WalkControl::Stop
                        } else {
                            WalkControl::Continue
                        }
                    },
                )?;

                let mut content = if entries.is_empty() {
                    format!("Directory is empty: {display_path}")
                } else {
                    format!("Directory: {display_path}\n\n{}", entries.join("\n"))
                };

                if status.truncated {
                    content.push_str("\n\n[Listing truncated by an entry or traversal limit]");
                }

                Ok(super::types::ToolResult::success("ls", "ls", content))
            })
            .await
        })
    }
}

fn format_entry(
    path: &std::path::Path,
    cwd: &str,
) -> Result<String, std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;

    let entry_type = if metadata.is_dir() {
        "dir"
    } else if metadata.is_file() {
        "file"
    } else if metadata.file_type().is_symlink() {
        "link"
    } else {
        "other"
    };

    Ok(format!(
        "{entry_type:<6} {:>10} {}",
        format_size(metadata.len()),
        crate::config::display_path(path, cwd),
    ))
}

/// Format a byte count as a human-readable size string.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
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
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn recursive_list_skips_generated_directory_contents_and_reports_cutoff() {
        let root = std::env::temp_dir().join(format!("hallucinate-ls-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("target/ignored.txt"), "ignored").unwrap();
        let result = LsTool::default()
            .execute(
                &json!({ "path": root, "recursive": true, "max_entries": 1 }),
                None,
            )
            .await
            .unwrap();
        assert!(result.content.contains("truncated"));
        assert!(!result.content.contains("ignored.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(1500), "1.5K");
    }
}
