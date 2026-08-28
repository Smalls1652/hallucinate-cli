//! Opt-in file logging for diagnosing agent behaviour.

use std::{fs::OpenOptions, path::PathBuf};

use anyhow::Context;
use tracing_subscriber::{EnvFilter, fmt};

use crate::config::paths;

/// Initialize structured logging to the debug log file.
///
/// The `RUST_LOG` environment variable controls the filter. Without it,
/// hallucinate debug events are recorded while dependency logs are omitted.
pub fn init_debug_logging() -> Result<Option<PathBuf>, anyhow::Error> {
    let Some(path) = paths::debug_log_file() else {
        return Ok(None);
    };
    let parent = path
        .parent()
        .context("debug log file must have a parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| {
            format!(
                "failed to create debug log directory {}",
                crate::config::paths::display_path(parent, ".")
            )
        })?;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| {
            format!(
                "failed to open debug log file {}",
                crate::config::paths::display_path(&path, ".")
            )
        })?;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("hallucinate_cli=debug"));
    fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(move || {
            file.try_clone()
                .expect("debug log file remains available while logging is initialized")
        })
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize debug logging: {error}"))?;

    Ok(Some(path))
}
