//! Parsing and describing commands entered in the interactive editor.
//!
//! Keeping the small amount of command syntax outside the terminal loop makes
//! it possible to test command dispatch without having to create a terminal.

/// A parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// The command name without the leading slash.
    pub name: String,
    /// The unparsed text following the command name.
    pub arguments: String,
}

/// Parse the first command in an input line. Input which is not a slash
/// command returns `None`; the rest of the line is deliberately left intact
/// so commands can implement their own argument rules.
pub fn parse(input: &str) -> Option<SlashCommand> {
    let input = input.trim_start();
    let input = input.strip_prefix('/')?;

    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    if end == 0 {
        return None;
    }

    let arguments = input[end..].trim().to_string();

    Some(SlashCommand {
        name: input[..end].to_string(),
        arguments,
    })
}

/// A command shown by `/help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

/// Return whether `name` is an exact built-in command name.
pub fn is_builtin_command(name: &str) -> bool {
    BUILTIN_COMMANDS
        .iter()
        .filter(|command| !command.name.starts_with("skill:"))
        .any(|command| command.name.eq_ignore_ascii_case(name))
}

/// Interactive commands provided by hallucinate.
pub const BUILTIN_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        usage: "/help",
        description: "Show available commands",
    },
    CommandInfo {
        name: "auth",
        usage: "/auth <list|set|remove> [provider] [api-key]",
        description: "Save, remove, or list provider API keys",
    },
    CommandInfo {
        name: "new",
        usage: "/new [title]",
        description: "Create and switch to a new session",
    },
    CommandInfo {
        name: "resume",
        usage: "/resume [id]",
        description: "List saved sessions or resume one by ID",
    },
    CommandInfo {
        name: "fork",
        usage: "/fork",
        description: "Fork the current session",
    },
    CommandInfo {
        name: "delete",
        usage: "/delete [id]",
        description: "Delete a saved session",
    },
    CommandInfo {
        name: "name",
        usage: "/name [name]",
        description: "Set or clear the current session name",
    },
    CommandInfo {
        name: "session",
        usage: "/session",
        description: "Show current session information",
    },
    CommandInfo {
        name: "prompts",
        usage: "/prompts",
        description: "List prompt templates",
    },
    CommandInfo {
        name: "skills",
        usage: "/skills",
        description: "List loaded skills",
    },
    CommandInfo {
        name: "model",
        usage: "/model [provider/model]",
        description: "Open the model picker or switch directly",
    },
    CommandInfo {
        name: "thinking",
        usage: "/thinking [on|off]",
        description: "Toggle model thinking/reasoning output",
    },
    CommandInfo {
        name: "skill:<name>",
        usage: "/skill:<name> [arguments]",
        description: "Invoke a loaded skill",
    },
    CommandInfo {
        name: "compact",
        usage: "/compact",
        description: "Compact the current context",
    },
    CommandInfo {
        name: "reload",
        usage: "/reload",
        description: "Reload prompt templates and system prompt files",
    },
    CommandInfo {
        name: "stop",
        usage: "/stop",
        description: "Cancel the running response",
    },
    CommandInfo {
        name: "quit",
        usage: "/quit",
        description: "Exit the interactive session",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_without_destroying_arguments() {
        assert_eq!(
            parse("  /resume   abc-def  "),
            Some(SlashCommand {
                name: "resume".to_string(),
                arguments: "abc-def".to_string()
            })
        );
        assert_eq!(
            parse("/name A useful session title"),
            Some(SlashCommand {
                name: "name".to_string(),
                arguments: "A useful session title".to_string()
            })
        );
    }

    #[test]
    fn ordinary_prompts_and_bare_slashes_are_not_commands() {
        assert_eq!(parse("explain /new"), None);
        assert_eq!(parse("/"), None);
    }

    #[test]
    fn recognizes_built_in_command_names_case_insensitively() {
        assert!(is_builtin_command("quit"));
        assert!(is_builtin_command("QUIT"));
        assert!(!is_builtin_command("q"));
        assert!(!is_builtin_command("unknown"));
    }
}
