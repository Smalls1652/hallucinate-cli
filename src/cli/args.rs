//! CLI argument parsing.

use clap::{ArgAction, Parser, Subcommand};

/// Hallucinate CLI - AI coding agent powered by LLMs.
#[derive(Debug, Clone, Parser)]
#[command(name = "hallucinate", version, about)]
pub struct Cli {
    /// Manage saved provider authentication details.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Model to use (e.g., "gpt-4o", "openai/gpt-4o", or a configured
    /// "provider/model" pair).
    #[arg(short, long)]
    pub model: Option<String>,

    /// API key for the provider. Overrides environment variables and stored
    /// credentials. Environment fallback also accepts OPENAI_API_KEY.
    #[arg(long, env = "HALLUCINATE_API_KEY")]
    pub api_key: Option<String>,

    /// Base URL for the API (for OpenAI-compatible providers).
    #[arg(long)]
    pub base_url: Option<String>,

    /// Shell executable for the bash tool. Overrides the settings file and
    /// SHELL environment variable.
    #[arg(long)]
    pub shell: Option<String>,

    /// Print mode: send prompt and print the response, then exit.
    #[arg(short = 'p', long)]
    pub print: Option<String>,

    /// Run in RPC mode (JSON-RPC over stdin/stdout).
    #[arg(long)]
    pub rpc: bool,

    /// Run in interactive mode (default when no other mode is specified).
    #[arg(long)]
    pub interactive: bool,

    /// Resume this persisted session in interactive mode.
    #[arg(long)]
    pub session: Option<String>,

    /// Maximum number of tokens in the response.
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Show model thinking/reasoning in output. Overrides the settings file.
    #[arg(long, conflicts_with = "no_thinking")]
    pub thinking: bool,

    /// Hide model thinking/reasoning in output. Overrides the settings file.
    #[arg(long, conflicts_with = "thinking")]
    pub no_thinking: bool,

    /// Temperature for sampling (0.0 - 2.0).
    #[arg(long)]
    pub temperature: Option<f64>,

    /// System prompt to use.
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Attach an image to a print-mode prompt. May be repeated.
    #[arg(long = "image", action = ArgAction::Append)]
    pub images: Vec<std::path::PathBuf>,

    /// Load a skill Markdown file or a directory of skills. May be repeated.
    #[arg(long = "skill", action = ArgAction::Append)]
    pub skills: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project skills.
    #[arg(long)]
    pub no_skills: bool,

    /// Load a prompt template Markdown file or directory. May be repeated.
    #[arg(long = "prompt-template", action = ArgAction::Append)]
    pub prompt_templates: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project prompt templates.
    #[arg(long)]
    pub no_prompt_templates: bool,

    /// Write diagnostic logs to the application cache directory.
    #[arg(long)]
    pub debug: bool,
}

/// Top-level subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Save, remove, or list provider API keys.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

/// Operations on the persisted provider credential store.
#[derive(Debug, Clone, Subcommand)]
pub enum AuthCommand {
    /// Save or replace an API key. If omitted, a configured environment
    /// variable is used (for example HALLUCINATE_OPENAI_API_KEY).
    Set {
        /// Provider ID, such as openai or openrouter.
        provider: String,
        /// API key. Prefer an environment variable when possible so the key
        /// is not recorded in shell history.
        #[arg(long, short = 'k')]
        api_key: Option<String>,
    },
    /// Remove a provider's saved API key.
    Remove {
        /// Provider ID, such as openai or openrouter.
        provider: String,
    },
    /// List providers with saved credentials without revealing their keys.
    List,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parse() {
        let cli = Cli::try_parse_from([
            "hallucinate",
            "--model",
            "gpt-4o",
            "--shell",
            "/bin/bash",
            "-p",
            "hello",
            "--image",
            "diagram.png",
            "--session",
            "session-id",
        ])
        .unwrap();
        assert_eq!(cli.model, Some("gpt-4o".to_string()));
        assert_eq!(cli.shell, Some("/bin/bash".to_string()));
        assert_eq!(cli.print, Some("hello".to_string()));
        assert_eq!(cli.images, vec![std::path::PathBuf::from("diagram.png")]);
        assert_eq!(cli.session.as_deref(), Some("session-id"));
        assert!(cli.command.is_none());
        assert!(!cli.thinking);
        assert!(!cli.no_thinking);
    }

    #[test]
    fn test_cli_parses_auth_commands() {
        let cli = Cli::try_parse_from([
            "hallucinate",
            "auth",
            "set",
            "openrouter",
            "--api-key",
            "sk-test",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                command: AuthCommand::Set {
                    provider,
                    api_key: Some(api_key)
                }
            }) if provider == "openrouter" && api_key == "sk-test"
        ));
    }

    #[test]
    fn test_cli_parses_thinking_options() {
        let cli = Cli::try_parse_from(["hallucinate", "--thinking"]).unwrap();
        assert!(cli.thinking);
        assert!(!cli.no_thinking);
        assert!(Cli::try_parse_from(["hallucinate", "--thinking", "--no-thinking"]).is_err());
    }

    #[test]
    fn test_cli_parses_skill_options() {
        let cli = Cli::try_parse_from([
            "hallucinate",
            "--skill",
            "skills/git",
            "--skill=skills/release/SKILL.md",
            "--no-skills",
            "--prompt-template",
            "prompts/review.md",
            "--no-prompt-templates",
            "--debug",
        ])
        .unwrap();
        assert_eq!(cli.skills.len(), 2);
        assert!(cli.no_skills);
        assert_eq!(cli.prompt_templates.len(), 1);
        assert!(cli.no_prompt_templates);
        assert!(cli.debug);
    }
}
