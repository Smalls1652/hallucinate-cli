//! System prompt assembly.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::config::paths;

/// Default system prompt template.
const DEFAULT_SYSTEM_PROMPT: &str = r#"You are an expert assistant operating inside hallucinate, an agent harness. You help users by reading files, executing commands, editing code, and writing new files.

## Guidelines

- Use `bash` for command line operations.
- Use `find` to search for files.
- Use `grep` for pattern-based content searches.
- Use `ls` to list directories and/or files for a directory.
- Use `read` to examine files instead of `cat` or `sed`.
- Use `edit` for precise changes (`edits[].oldText` must match exactly).
- When changing multiple separate locations in one file, use one `edit` call with multiple entries in `edits[]` instead of multiple `edit` calls.
- Each `edits[].oldText` is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.
- Keep `edits[].oldText` as small as possible while still being unique in the file. Do not pad with large unchanged regions.
- Use `write` only for new files or complete rewrites.
- Be concise in your responses.
- Show file paths clearly when working with files.
- Read relevant skills, based off the user's input, that are in the `<available_skills>` with `read`.

## Skills usage

If the user's request matches one of skills in the `<available_skills>` section, read the skill using the `read` tool. This will allow you to gain expanded knowledge on how to perform an operation.

- Skills are specialized prompt-based instruction files that help guide you to perform tasks.
- When necessary, read the skill file to load the instructions.
- They may include scripts within the skill directory that they reside in that can be called with the `bash` tool.
- **Do not attempt to run skills as if they were tools or scripts.**"#;

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
