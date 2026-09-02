//! Session working directory management.
//!
//! Tracks and persists the working directory for each session,
//! allowing sessions to resume in the correct context.

use std::{fs, path::PathBuf};

use hallucinate_agent::config::paths;

/// Session working directory metadata stored alongside the session file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionCwd {
    /// The working directory path.
    pub cwd: String,
    /// Timestamp when the cwd was last updated.
    pub timestamp: u64,
}

impl SessionCwd {
    /// Get the stored working directory, falling back to the current directory.
    pub fn load(session_id: &str) -> Result<String, anyhow::Error> {
        let cwd_path = cwd_file_path(session_id);

        if cwd_path.exists() {
            let content = fs::read_to_string(&cwd_path)?;
            let cwd: SessionCwd = serde_json::from_str(&content)?;
            return Ok(cwd.cwd);
        }

        // Fall back to current directory
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(Into::into)
    }

    /// Save the current working directory for a session.
    pub fn save(
        session_id: &str,
        cwd: &str,
    ) -> Result<(), anyhow::Error> {
        let cwd_path = cwd_file_path(session_id);

        if let Some(parent) = cwd_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let cwd_data = SessionCwd {
            cwd: cwd.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };

        let content = serde_json::to_string_pretty(&cwd_data)?;
        fs::write(&cwd_path, content)?;

        Ok(())
    }

    /// Delete the stored working directory for a session.
    pub fn delete(session_id: &str) -> Result<(), anyhow::Error> {
        let cwd_path = cwd_file_path(session_id);

        if cwd_path.exists() {
            fs::remove_file(&cwd_path)?;
        }

        Ok(())
    }
}

/// Get the path to a session's cwd metadata file.
fn cwd_file_path(session_id: &str) -> PathBuf {
    let safe_id: String = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();

    if let Some(sessions_dir) = paths::sessions_dir() {
        sessions_dir.join(format!("{}.cwd.json", safe_id))
    } else {
        PathBuf::from(format!("/tmp/session-{}.cwd.json", safe_id))
    }
}
