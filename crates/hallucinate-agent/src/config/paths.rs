//! Configuration path resolution.
//!
//! Provides platform-specific paths for configuration files, session storage,
//! and other persistent data using the `directories` crate.

use std::path::{Component, Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};

/// Get the project directories for hallucinate.
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("online", "smalls", "hallucinate")
}

/// Get the configuration directory path.
///
/// Returns:
/// - macOS: `~/Library/Application Support/com.hallucinate.hallucinate/`
/// - Linux: `~/.config/hallucinate/`
/// - Windows: `C:\Users\<user>\AppData\Roaming\com\hallucinate\hallucinate\ `
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.config_dir().to_path_buf())
}

/// Get the data directory path.
///
/// Returns:
/// - macOS: `~/Library/Application Support/com.hallucinate.hallucinate/`
/// - Linux: `~/.local/share/hallucinate/`
/// - Windows: `C:\Users\<user>\AppData\Roaming\com\hallucinate\hallucinate\ `
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_dir().to_path_buf())
}

/// Get the cache directory path.
///
/// Returns:
/// - macOS: `~/Library/Caches/com.hallucinate.hallucinate/`
/// - Linux: `~/.cache/hallucinate/`
/// - Windows: `C:\Users\<user>\AppData\Local\com\hallucinate\hallucinate\
///   Cache\`
pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.cache_dir().to_path_buf())
}

/// Get the path to the TOML settings configuration file.
pub fn settings_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("settings.toml"))
}

/// Get the project-local TOML settings override for a working directory.
pub fn project_settings_file(cwd: impl AsRef<std::path::Path>) -> PathBuf {
    cwd.as_ref().join(".hallucinate").join("settings.toml")
}

/// Get the path to the user-level system prompt override.
pub fn system_prompt_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("SYSTEM.md"))
}

/// Get the project-level agent instructions file for a working directory.
pub fn project_agents_file(cwd: impl AsRef<std::path::Path>) -> PathBuf {
    cwd.as_ref().join("AGENTS.md")
}

/// Format a path for display in the context of a project directory.
///
/// Paths inside `project_dir` are shown relative to that directory. Paths
/// elsewhere in the user's home directory are shown with a `~/` prefix. Paths
/// outside the home directory cannot be shortened and remain absolute.
pub fn display_path(
    path: impl AsRef<Path>,
    project_dir: impl AsRef<Path>,
) -> String {
    let project_dir = absolute_normalized(project_dir.as_ref());

    let path = expand_tilde(path.as_ref());
    let path = if path.is_absolute() {
        absolute_normalized(&path)
    } else {
        absolute_normalized(&project_dir.join(path))
    };

    if let Ok(relative) = path.strip_prefix(&project_dir) {
        return relative_path_string(relative);
    }

    if let Some(home_dir) = BaseDirs::new().map(|dirs| absolute_normalized(dirs.home_dir()))
        && let Ok(relative) = path.strip_prefix(&home_dir)
    {
        if relative.as_os_str().is_empty() {
            return "~".to_string();
        }

        let mut shortened = PathBuf::from("~");
        shortened.push(relative);

        return shortened.display().to_string();
    }

    path.display().to_string()
}

/// Expand a path that may contain a tilde (`~`) to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    PathBuf::from(shellexpand::tilde(&path.to_string_lossy()).into_owned())
}

/// Resolve a path to an absolute path and normalize it (removing `.` and `..`
/// components).
fn absolute_normalized(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    let mut normalized = PathBuf::new();

    for component in absolute.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                normalized.pop();
            },
            component => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

/// Convert a path (typically relative to a project dir) to a display string.
fn relative_path_string(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        path.display().to_string()
    }
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

/// Get the user-level script tool definitions directory.
pub fn script_tools_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("script_tools"))
}

/// Get the project-local script tool definitions directory.
pub fn project_script_tools_dir(cwd: impl AsRef<std::path::Path>) -> PathBuf {
    cwd.as_ref().join(".hallucinate").join("script_tools")
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

    if let Some(dir) = script_tools_dir() {
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
        assert_eq!(
            project_settings_file("/tmp/project"),
            PathBuf::from("/tmp/project/.hallucinate/settings.toml")
        );
        assert!(system_prompt_file().is_some());
    }

    #[test]
    fn test_project_agents_file() {
        assert_eq!(
            project_agents_file("/tmp/project"),
            PathBuf::from("/tmp/project/AGENTS.md")
        );
    }

    #[test]
    fn display_path_prefers_project_relative_paths() {
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        let project = home.join("hallucinate-project");

        assert_eq!(
            display_path(project.join("src/main.rs"), &project),
            "src/main.rs"
        );
        assert_eq!(display_path(&project, &project), ".");
        assert_eq!(
            display_path(project.join("../shared/config.toml"), &project),
            "~/shared/config.toml"
        );
    }

    #[test]
    fn display_path_shortens_paths_in_the_home_directory() {
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        let project = home.join("hallucinate-project");

        assert_eq!(
            display_path(home.join("shared/config.toml"), &project),
            "~/shared/config.toml"
        );
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
