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

/// Every accepted built-in invocation and its canonical command name.
///
/// Keeping aliases here makes abbreviated commands resolve consistently with
/// the autocomplete list and the dispatcher.
pub const BUILTIN_INVOCATIONS: &[(&str, &str)] = &[
    ("help", "help"),
    ("auth", "auth"),
    ("new", "new"),
    ("clear", "new"),
    ("sessions", "sessions"),
    ("list", "sessions"),
    ("resume", "resume"),
    ("fork", "fork"),
    ("delete", "delete"),
    ("name", "name"),
    ("session", "session"),
    ("status", "session"),
    ("prompts", "prompts"),
    ("skills", "skills"),
    ("model", "model"),
    ("models", "model"),
    ("thinking", "thinking"),
    ("compact", "compact"),
    ("reload", "reload"),
    ("stop", "stop"),
    ("cancel", "stop"),
    ("quit", "quit"),
    ("exit", "quit"),
];

/// Resolve a complete command or an unambiguous abbreviation.
///
/// Exact matches win over longer commands. An abbreviation which maps to more
/// than one canonical command is rejected rather than choosing arbitrarily.
pub fn resolve_builtin_command(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();

    if let Some((_, command)) = BUILTIN_INVOCATIONS
        .iter()
        .find(|(invocation, _)| *invocation == name)
    {
        return Some(command);
    }

    let mut resolved = None;

    for (invocation, command) in BUILTIN_INVOCATIONS {
        if !invocation.starts_with(&name) {
            continue;
        }

        if let Some(previous) = resolved
            && previous != *command
        {
            return None;
        }

        resolved = Some(*command);
    }

    resolved
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
        name: "sessions",
        usage: "/sessions",
        description: "List saved sessions",
    },
    CommandInfo {
        name: "resume",
        usage: "/resume <id>",
        description: "Resume a saved session",
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
    fn resolves_only_unambiguous_abbreviations() {
        assert_eq!(resolve_builtin_command("q"), Some("quit"));
        assert_eq!(resolve_builtin_command("qui"), Some("quit"));
        assert_eq!(resolve_builtin_command("n"), None);
        assert_eq!(resolve_builtin_command("new"), Some("new"));
    }
}
