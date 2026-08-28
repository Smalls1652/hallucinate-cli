//! Configuration path resolution.
//!
//! Provides platform-specific paths for configuration files, session storage,
//! and other persistent data using the `directories` crate.

use std::path::PathBuf;

use directories::{BaseDirs, ProjectDirs};

/// Get the project directories for hallucinate-cli.
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("com", "hallucinate", "hallucinate-cli")
}

/// Get the configuration directory path.
///
/// Returns:
/// - macOS: `~/Library/Application Support/com.hallucinate.hallucinate-cli/`
/// - Linux: `~/.config/hallucinate-cli/`
/// - Windows: `C:\Users\<user>\AppData\Roaming\com\hallucinate\hallucinate-cli\
///   `
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.config_dir().to_path_buf())
}

/// Get the data directory path.
///
/// Returns:
/// - macOS: `~/Library/Application Support/com.hallucinate.hallucinate-cli/`
/// - Linux: `~/.local/share/hallucinate-cli/`
/// - Windows: `C:\Users\<user>\AppData\Roaming\com\hallucinate\hallucinate-cli\
///   `
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_dir().to_path_buf())
}

/// Get the cache directory path.
///
/// Returns:
/// - macOS: `~/Library/Caches/com.hallucinate.hallucinate-cli/`
/// - Linux: `~/.cache/hallucinate-cli/`
/// - Windows: `C:\Users\<user>\AppData\Local\com\hallucinate\hallucinate-cli\
///   Cache\`
pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.cache_dir().to_path_buf())
}

/// Get the path to the TOML settings configuration file.
pub fn settings_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("settings.toml"))
}

/// Get the sessions directory path.
pub fn sessions_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("sessions"))
}

/// Get the user-level Agent Skills directory.
pub fn skills_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("skills"))
}

/// Get the user-level skills directory from the shared `.agents` protocol.
pub fn agents_skills_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.home_dir().join(".agents").join("skills"))
}

/// Get the project-local Agent Skills directory for a working directory.
pub fn project_skills_dir(cwd: impl AsRef<std::path::Path>) -> PathBuf {
    cwd.as_ref().join(".hallucinate").join("skills")
}

/// Get the project-local skills directory from the shared `.agents` protocol.
pub fn project_agents_skills_dir(cwd: impl AsRef<std::path::Path>) -> PathBuf {
    cwd.as_ref().join(".agents").join("skills")
}

/// Get the user-level prompt templates directory.
pub fn prompts_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("prompts"))
}

/// Get the project-local prompt templates directory for a working directory.
pub fn project_prompts_dir(cwd: impl AsRef<std::path::Path>) -> PathBuf {
    cwd.as_ref().join(".hallucinate").join("prompts")
}

/// Get the debug log file path.
pub fn debug_log_file() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("logs").join("hallucinate.log"))
}

/// Get the path to a specific session file.
pub fn session_file(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return None;
    }
    sessions_dir().map(|dir| dir.join(format!("{}.jsonl", session_id)))
}

/// Ensure all required directories exist.
pub fn ensure_directories() -> Result<(), std::io::Error> {
    if let Some(dir) = config_dir() {
        std::fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = data_dir() {
        std::fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = cache_dir() {
        std::fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = sessions_dir() {
        std::fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = skills_dir() {
        std::fs::create_dir_all(&dir)?;
    }
    if let Some(dir) = prompts_dir() {
        std::fs::create_dir_all(&dir)?;
    }
    if let Some(path) = debug_log_file()
        && let Some(dir) = path.parent()
    {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_exist() {
        assert!(config_dir().is_some());
        assert!(data_dir().is_some());
        assert!(cache_dir().is_some());
        assert!(sessions_dir().is_some());
        assert!(skills_dir().is_some());
        assert!(agents_skills_dir().is_some());
        assert!(prompts_dir().is_some());
        assert!(debug_log_file().is_some());
        assert!(settings_file().is_some());
    }

    #[test]
    fn test_project_skills_dir() {
        assert_eq!(
            project_skills_dir("/tmp/project"),
            PathBuf::from("/tmp/project/.hallucinate/skills")
        );
    }

    #[test]
    fn test_project_agents_skills_dir() {
        assert_eq!(
            project_agents_skills_dir("/tmp/project"),
            PathBuf::from("/tmp/project/.agents/skills")
        );
    }

    #[test]
    fn test_project_prompts_dir() {
        assert_eq!(
            project_prompts_dir("/tmp/project"),
            PathBuf::from("/tmp/project/.hallucinate/prompts")
        );
    }

    #[test]
    fn test_session_file_path() {
        let path = session_file("test-session-123");
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("test-session-123.jsonl"));
        assert!(session_file("../outside").is_none());
        assert!(session_file("").is_none());
    }
}
