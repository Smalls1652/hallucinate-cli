//! Find tool: bounded deterministic glob-based file search.

use std::{future::Future, pin::Pin};

use serde_json::json;

use super::{
    limits::{MAX_REQUESTED_RESULTS, ToolLimits, parse_bounded_usize, run_blocking},
    paths::resolve_path,
    types::Tool,
    walker::{WalkControl, walk_matching_files},
};

/// Glob-based file search tool.
#[derive(Debug, Clone)]
pub struct FindTool {
    limits: ToolLimits,

    /// Default maximum number of results to return.
    max_results: usize,
}

impl Default for FindTool {
    fn default() -> Self {
        Self {
            limits: ToolLimits::default(),
            max_results: 100,
        }
    }
}

impl FindTool {
    /// Create a new `FindTool` with default settings.
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

impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Search recursively for files matching a glob. Results are deterministic, generated directories are skipped, and traversal stops at configured result and resource limits."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern (for example, '*.rs' or 'src/**/*.ts')" },
                "path": { "type": "string", "description": "Directory to search (default: current directory)" },
                "max_results": { "type": "integer", "description": "Maximum files to return (default: 100, maximum: 1000)" }
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
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: pattern"))?;

            let glob_pattern = glob::Pattern::new(pattern)
                .map_err(|error| anyhow::anyhow!("Invalid glob pattern '{pattern}': {error}"))?;

            let max_results = parse_bounded_usize(
                arguments,
                "max_results",
                self.max_results,
                MAX_REQUESTED_RESULTS,
            )?;

            let project_dir = cwd.unwrap_or(".").to_string();

            let search_path = resolve_path(
                arguments
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("."),
                Some(&project_dir),
            );

            let limits = self.limits.clone();

            let pattern = pattern.to_string();

            run_blocking("find", move || {
                let mut matches = Vec::new();

                let status =
                    walk_matching_files(&search_path, Some(&glob_pattern), &limits, |path| {
                        if matches.len() >= max_results {
                            return WalkControl::Stop;
                        }

                        matches.push(crate::config::paths::display_path(&path, &project_dir));

                        if matches.len() >= max_results {
                            WalkControl::Stop
                        } else {
                            WalkControl::Continue
                        }
                    })?;

                let mut content = if matches.is_empty() {
                    format!("No files matching pattern: {pattern}")
                } else {
                    format!(
                        "Found {} file(s) matching '{pattern}':\n\n{}",
                        matches.len(),
                        matches.join("\n")
                    )
                };

                if status.truncated {
                    content.push_str("\n\n[Search truncated by a result or traversal limit]");
                }

                Ok(super::types::ToolResult::success("find", "find", content))
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn returns_sorted_matches_and_skips_generated_directories() {
        let root = std::env::temp_dir().join(format!("hallucinate-find-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("b.rs"), "").unwrap();
        fs::write(root.join("a.rs"), "").unwrap();
        fs::write(root.join("target/ignored.rs"), "").unwrap();

        let result = FindTool::new()
            .execute(&json!({ "pattern": "*.rs", "path": root }), None)
            .await
            .unwrap();
        assert!(result.content.contains("a.rs"));
        assert!(result.content.contains("b.rs"));
        assert!(!result.content.contains("ignored.rs"));
        assert!(result.content.find("a.rs") < result.content.find("b.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rejects_result_counts_above_the_hard_ceiling() {
        let result = FindTool::new()
            .execute(&json!({ "pattern": "*", "max_results": 1001 }), None)
            .await;
        assert!(result.is_err());
    }
}
