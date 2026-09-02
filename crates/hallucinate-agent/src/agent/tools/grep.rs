//! Grep tool: bounded, single-pass pattern-based file content search.

use std::{
    fs::File,
    future::Future,
    io::{BufRead, BufReader},
    path::Path,
    pin::Pin,
};

use serde_json::json;

use super::{
    limits::{MAX_REQUESTED_RESULTS, ToolLimits, parse_bounded_usize, run_blocking},
    paths::resolve_path,
    types::Tool,
    walker::{WalkControl, walk_matching_files},
};

/// Pattern-based file content search tool.
#[derive(Debug, Clone)]
pub struct GrepTool {
    limits: ToolLimits,
    /// Default maximum number of matches to return.
    max_results: usize,
}

impl Default for GrepTool {
    fn default() -> Self {
        Self {
            limits: ToolLimits::default(),
            max_results: 50,
        }
    }
}

impl GrepTool {
    /// Create a new `GrepTool` with default settings.
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

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a regular expression. Returns matching paths and line numbers. Search skips generated directories and binary files and stops at bounded result and scan budgets."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression pattern to search for" },
                "path": { "type": "string", "description": "Path to search (file or directory, default: current directory)" },
                "file_pattern": { "type": "string", "description": "Glob pattern to filter files (for example, '*.rs')" },
                "max_results": { "type": "integer", "description": "Maximum matches to return (default: 50, maximum: 1000)" }
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
            let regex = regex::Regex::new(pattern)
                .map_err(|error| anyhow::anyhow!("Invalid regex pattern '{pattern}': {error}"))?;
            let file_pattern = arguments
                .get("file_pattern")
                .and_then(|value| value.as_str())
                .map(glob::Pattern::new)
                .transpose()
                .map_err(|error| anyhow::anyhow!("Invalid file_pattern: {error}"))?;
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

            run_blocking("grep", move || {
                let mut matches = Vec::new();
                let mut scanned_bytes = 0_usize;
                let mut result_bytes = 0_usize;
                let mut scan_truncated = false;
                let walk_status =
                    walk_matching_files(&search_path, file_pattern.as_ref(), &limits, |path| {
                        let scan_status = scan_file(
                            &path,
                            &regex,
                            &project_dir,
                            &limits,
                            &mut scanned_bytes,
                            &mut result_bytes,
                            &mut matches,
                            max_results,
                        );
                        scan_truncated |= scan_status.truncated;

                        if matches.len() >= max_results || scan_status.stop_walk {
                            WalkControl::Stop
                        } else {
                            WalkControl::Continue
                        }
                    })?;

                if matches.is_empty() {
                    let mut content = format!("No matches found for pattern: {pattern}");
                    if walk_status.truncated || scan_truncated {
                        content.push_str(
                            "\n\n[Search truncated by a result, traversal, or scan-byte limit]",
                        );
                    }
                    return Ok(super::types::ToolResult::success("grep", "grep", content));
                }

                let mut content = format!(
                    "Found {} match(es) for '{pattern}':\n\n{}",
                    matches.len(),
                    matches.join("\n")
                );
                if walk_status.truncated || scan_truncated {
                    content.push_str(
                        "\n\n[Search truncated by a result, traversal, or scan-byte limit]",
                    );
                }
                Ok(super::types::ToolResult::success("grep", "grep", content))
            })
            .await
        })
    }
}

#[derive(Debug, Default)]
struct ScanStatus {
    truncated: bool,
    stop_walk: bool,
}

fn scan_file(
    path: &Path,
    regex: &regex::Regex,
    project_dir: &str,
    limits: &ToolLimits,
    scanned_bytes: &mut usize,
    result_bytes: &mut usize,
    matches: &mut Vec<String>,
    max_results: usize,
) -> ScanStatus {
    let Ok(file) = File::open(path) else {
        return ScanStatus::default();
    };
    let mut reader = BufReader::new(file);
    let is_binary = match reader.fill_buf() {
        Ok(prefix) => prefix.iter().take(8_192).any(|byte| *byte == 0),
        Err(_) => return ScanStatus::default(),
    };
    if is_binary {
        return ScanStatus::default();
    }

    let mut status = ScanStatus::default();
    let mut file_bytes = 0_usize;
    let mut line_number = 0_usize;
    loop {
        if matches.len() >= max_results {
            status.stop_walk = true;
            break;
        }
        let remaining_file = limits.max_file_scan_bytes.saturating_sub(file_bytes);
        let remaining_search = limits.max_search_bytes.saturating_sub(*scanned_bytes);
        let remaining = remaining_file.min(remaining_search);
        if remaining == 0 {
            status.truncated = true;
            status.stop_walk = *scanned_bytes >= limits.max_search_bytes;
            break;
        }

        let Some(line) = read_bounded_line(&mut reader, remaining, limits.max_grep_line_bytes)
        else {
            break;
        };
        line_number += 1;
        file_bytes += line.len();
        *scanned_bytes += line.len();
        if line.len() == limits.max_grep_line_bytes && !line.ends_with(b"\n") {
            status.truncated = true;
            break;
        }

        let Ok(line) = std::str::from_utf8(&line) else {
            continue;
        };
        if regex.is_match(line) {
            let matched_line = format!(
                "{}:{line_number}: {}",
                crate::config::paths::display_path(path, project_dir),
                line.trim(),
            );
            let next_size = result_bytes
                .saturating_add(matched_line.len())
                .saturating_add(1);
            if next_size > limits.max_tool_result_bytes {
                status.truncated = true;
                status.stop_walk = true;
                break;
            }
            *result_bytes = next_size;
            matches.push(matched_line);
        }
    }
    status
}

/// Read one line without allocating more than the per-line or scan budget.
fn read_bounded_line(
    reader: &mut BufReader<File>,
    max_bytes: usize,
    max_line_bytes: usize,
) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let limit = max_bytes.min(max_line_bytes);
    while line.len() < limit {
        let buffer = reader.fill_buf().ok()?;
        if buffer.is_empty() {
            return (!line.is_empty()).then_some(line);
        }
        let take = buffer.len().min(limit - line.len());
        let newline = buffer[..take].iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(take, |index| index + 1);
        line.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if newline.is_some() {
            return Some(line);
        }
    }
    (!line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn stops_at_the_result_budget_and_skips_binary_and_generated_files() {
        let root = std::env::temp_dir().join(format!("hallucinate-grep-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("a.txt"), "needle\nneedle\n").unwrap();
        fs::write(root.join("b.txt"), "needle\n").unwrap();
        fs::write(root.join("binary.txt"), b"needle\0").unwrap();
        fs::write(root.join("target/ignored.txt"), "needle\n").unwrap();

        let result = GrepTool::new()
            .execute(
                &json!({ "pattern": "needle", "path": root, "max_results": 1 }),
                None,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.matches("needle").count(), 2); // Pattern heading plus first match.
        assert!(result.content.contains("truncated"));
        assert!(!result.content.contains("ignored"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_an_invalid_regex() {
        let tool = GrepTool::new();
        let arguments = json!({ "pattern": "[invalid" });
        let result = tool.execute(&arguments, None);
        assert!(futures::executor::block_on(result).is_err());
    }
}
