//! Shared resource limits and blocking-work coordination for built-in tools.
//!
//! Tool calls are model-controlled input. These limits keep a single request
//! from consuming unbounded memory, filesystem work, or Tokio blocking threads.

use anyhow::Context;
use tokio::sync::Semaphore;

/// Hard ceiling accepted for `max_results` and `max_entries` tool arguments.
pub const MAX_REQUESTED_RESULTS: usize = 1_000;

/// Resource limits applied to built-in filesystem and shell tools.
#[derive(Debug, Clone)]
pub struct ToolLimits {
    /// Default and maximum number of bytes read from an ordinary text file.
    pub max_text_file_bytes: usize,
    /// Maximum source-image bytes accepted before decoding.
    pub max_image_source_bytes: usize,
    /// Maximum directory entries inspected during one recursive walk.
    pub max_directory_entries: usize,
    /// Maximum regular files considered during one search.
    pub max_candidate_files: usize,
    /// Maximum bytes scanned from one grep candidate.
    pub max_file_scan_bytes: usize,
    /// Maximum bytes scanned by one grep request.
    pub max_search_bytes: usize,
    /// Maximum bytes captured separately from shell stdout and stderr.
    pub max_shell_stream_bytes: usize,
    /// Maximum bytes included in a returned tool result.
    pub max_tool_result_bytes: usize,
    /// Maximum accepted line length while scanning grep candidates.
    pub max_grep_line_bytes: usize,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            max_text_file_bytes: 1_000_000,
            max_image_source_bytes: 20 * 1024 * 1024,
            max_directory_entries: 20_000,
            max_candidate_files: 10_000,
            max_file_scan_bytes: 1_000_000,
            max_search_bytes: 20 * 1024 * 1024,
            max_shell_stream_bytes: 10_000,
            max_tool_result_bytes: 100_000,
            max_grep_line_bytes: 64 * 1024,
        }
    }
}

static BLOCKING_JOBS: Semaphore = Semaphore::const_new(4);

/// Run synchronous filesystem or CPU work without blocking a Tokio worker.
///
/// A global semaphore bounds concurrent jobs before they reach Tokio's blocking
/// pool, preventing bursts of tool calls from exhausting that pool.
pub async fn run_blocking<T, F>(
    operation: &'static str,
    work: F,
) -> Result<T, anyhow::Error>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, anyhow::Error> + Send + 'static,
{
    let permit = BLOCKING_JOBS
        .acquire()
        .await
        .expect("the global blocking-job semaphore is never closed");
    // Keep the permit inside the blocking closure. If the caller is cancelled,
    // the JoinHandle may be dropped while the filesystem work still runs.
    // Holding it here continues to cap actual concurrent blocking jobs.
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .with_context(|| format!("{operation} blocking task failed"))?;

    result
}

/// Parse a bounded non-negative integer tool argument.
pub fn parse_bounded_usize(
    arguments: &serde_json::Value,
    field: &str,
    default: usize,
    maximum: usize,
) -> Result<usize, anyhow::Error> {
    let Some(value) = arguments.get(field) else {
        return Ok(default);
    };

    let value = value
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("{field} must be a non-negative integer"))?;
    let value = usize::try_from(value)
        .map_err(|_| anyhow::anyhow!("{field} is too large for this platform"))?;

    if value > maximum {
        anyhow::bail!("{field} must not exceed {maximum}");
    }

    Ok(value)
}

/// Truncate text at a valid UTF-8 boundary and append a clear notice.
pub fn truncate_utf8(
    content: &str,
    max_bytes: usize,
    label: &str,
) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }

    let mut end = max_bytes.min(content.len());
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }

    format!(
        "{}\n\n[{label} truncated: {} total bytes]",
        &content[..end],
        content.len()
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_keeps_the_async_runtime_responsive() {
        let blocking_job = run_blocking("slow test job", || {
            std::thread::sleep(Duration::from_millis(50));
            Ok(())
        });
        let timer = tokio::time::sleep(Duration::from_millis(1));

        let (_, ()) = tokio::join!(blocking_job, timer);
    }
}
