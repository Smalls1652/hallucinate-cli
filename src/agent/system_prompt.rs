//! System prompt assembly.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::config::paths;

/// Default system prompt template.
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are a helpful AI coding assistant. You have access to tools that let you execute shell commands, read/write files, search code, and more.

When given a task:
1. Understand what the user wants
2. Plan your approach
3. Use tools as needed to accomplish the task
4. Explain what you did and show results
5. If something fails, try a different approach

Be concise in your explanations. Focus on solving the user's problem efficiently.

Remember: Always verify your work by reading back files or checking output after making changes. If you make an edit, read the file afterward to confirm it was applied correctly."#;

/// Files which can contribute project or user instructions to the system
/// prompt. The paths are reported independently of whether their contents can
/// be read, so the UI can still tell the user which files were found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemPromptFiles {
    /// The global `SYSTEM.md` override, when it exists.
    pub system_prompt: Option<PathBuf>,
    /// The project-local `AGENTS.md`, when it exists.
    pub agents: Option<PathBuf>,
}

/// Return the system-prompt files found for a project directory.
pub fn prompt_file_paths(cwd: impl AsRef<Path>) -> SystemPromptFiles {
    SystemPromptFiles {
        system_prompt: paths::system_prompt_file().filter(|path| path.is_file()),
        agents: {
            let path = paths::project_agents_file(cwd);
            path.is_file().then_some(path)
        },
    }
}

/// Build the system prompt using the global `SYSTEM.md` override when one is
/// available. This variant is kept for callers that do not have a project
/// working directory; it still applies the global override.
pub fn build_system_prompt(custom_prompt: Option<&str>) -> String {
    let global_prompt = paths::system_prompt_file().and_then(|path| fs::read_to_string(path).ok());
    custom_prompt
        .map(str::to_owned)
        .or(global_prompt)
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
}

/// Build the system prompt for a working directory.
///
/// An explicit prompt (for example, `--system-prompt`) takes precedence over
/// the global file. `AGENTS.md` is always appended when it can be read.
pub fn build_system_prompt_for_cwd(
    custom_prompt: Option<&str>,
    cwd: Option<&Path>,
) -> String {
    let mut prompt = build_system_prompt(custom_prompt);

    if let Some(cwd) = cwd
        && let Some(path) = prompt_file_paths(cwd).agents
        && let Ok(agents_prompt) = fs::read_to_string(path)
    {
        prompt.push_str("\n\n## Project Instructions\n\n");
        prompt.push_str(&agents_prompt);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt(None);
        // A user-installed SYSTEM.md is allowed to customize this result, so
        // this assertion only verifies that the result is non-empty.
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_build_system_prompt_with_custom() {
        let custom = "You are a custom assistant.";
        let prompt = build_system_prompt(Some(custom));
        assert_eq!(prompt, custom);
    }

    #[test]
    fn project_agents_are_appended_to_the_prompt() {
        let directory =
            std::env::temp_dir().join(format!("hallucinate-system-prompt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("AGENTS.md"), "Follow project conventions.").unwrap();

        let prompt = build_system_prompt_for_cwd(Some("Base prompt."), Some(&directory));
        assert!(prompt.starts_with("Base prompt."));
        assert!(prompt.contains("## Project Instructions"));
        assert!(prompt.contains("Follow project conventions."));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn prompt_file_paths_only_reports_existing_files() {
        let directory =
            std::env::temp_dir().join(format!("hallucinate-system-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();

        let files = prompt_file_paths(&directory);
        assert!(files.agents.is_none());

        fs::write(directory.join("AGENTS.md"), "instructions").unwrap();
        let files = prompt_file_paths(&directory);
        assert_eq!(files.agents, Some(directory.join("AGENTS.md")));

        let _ = fs::remove_dir_all(directory);
    }
}
