//! CLI argument parsing.

mod auth;
mod completions;
mod interactive;
mod print;
mod rpc;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use hallucinate_agent::config::settings::Settings;

pub use self::{
    auth::AuthCommand,
    completions::{CompletionsArgs, completions_command},
    interactive::interactive_command,
    print::{PrintArgs, print_command},
    rpc::{RpcArgs, rpc_command},
};

pub const CLI_NAME: &'static str = "hallucinate";

/// Hallucinate CLI - AI coding agent powered by LLMs.
#[derive(Debug, Clone, Parser)]
#[command(name = CLI_NAME, version, about)]
pub struct Cli {
    /// Subcommands for the CLI.
    #[command(subcommand)]
    pub command: Option<CliCommands>,

    /// Model to use as a configured model ID or "provider/model" pair.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// API key for the provider. Overrides environment variables and stored
    /// credentials. Environment fallback also accepts OPENAI_API_KEY.
    #[arg(long = "api-key", env = "HALLUCINATE_API_KEY")]
    pub api_key: Option<String>,

    /// Base URL for the API (for OpenAI-compatible providers).
    #[arg(long = "base-url")]
    pub base_url: Option<String>,

    /// Shell executable for the bash tool. Overrides the settings file and
    /// SHELL environment variable.
    #[arg(long = "shell")]
    pub shell: Option<String>,

    /// Resume this persisted session in interactive mode.
    #[arg(long = "session")]
    pub session: Option<String>,

    /// Maximum number of tokens in the response.
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,

    /// Show model thinking/reasoning in output. Overrides the settings file.
    #[arg(long)]
    pub thinking: bool,

    /// Temperature for sampling (0.0 - 2.0).
    #[arg(long = "temperature")]
    pub temperature: Option<f64>,

    /// System prompt to use.
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Attach an image to a print-mode prompt. May be repeated.
    #[arg(long = "image", action = ArgAction::Append)]
    pub images: Vec<std::path::PathBuf>,

    /// Load a skill Markdown file or a directory of skills. May be repeated.
    #[arg(long = "skill", action = ArgAction::Append)]
    pub skills: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project skills.
    #[arg(long = "no-skills")]
    pub no_skills: bool,

    /// Load a prompt template Markdown file or directory. May be repeated.
    #[arg(long = "prompt-template", action = ArgAction::Append)]
    pub prompt_templates: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project prompt templates.
    #[arg(long = "no-prompt-templates")]
    pub no_prompt_templates: bool,

    /// Write diagnostic logs to the application cache directory.
    #[arg(long = "debug")]
    pub debug: bool,
}

impl From<&Cli> for hallucinate_agent::config::RuntimeOverrides {
    fn from(cli: &Cli) -> Self {
        Self {
            model: cli.model.clone(),
            api_key: cli.api_key.clone(),
            base_url: cli.base_url.clone(),
            shell: cli.shell.clone(),
            max_tokens: cli.max_tokens,
            temperature: cli.temperature,
            system_prompt: cli.system_prompt.clone(),
        }
    }
}

impl Cli {
    /// Execute the CLI command, loading settings and dispatching to the
    /// appropriate run mode.
    pub async fn run(&self) -> Result<()> {
        // All run modes share the same persisted settings and CLI overrides. Do
        // not replace a malformed settings file with defaults while modifying
        // auth; that could discard unrelated configuration.
        let settings = if self.command.is_some() {
            // Authentication commands persist only the global settings. Project
            // overrides are runtime configuration and must not be copied into
            // the user's global settings file.
            Settings::load_global()?
        } else {
            Settings::load()?
        };

        if let Some(command) = &self.command {
            return command.route_subcommand(&settings).await;
        }

        interactive_command(self, &settings).await
    }
}

/// Top-level subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum CliCommands {
    /// Save, remove, or list provider API keys.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },

    /// Send prompt and print the response, then exit.
    Print(PrintArgs),

    /// Run in RPC mode (JSON-RPC over stdin/stdout).
    Rpc(RpcArgs),

    /// Generate shell completions.
    Completions(CompletionsArgs)
}

impl CliCommands {
    /// Route to the appropriate subcommand handler.
    pub async fn route_subcommand(
        &self,
        settings: &Settings,
    ) -> Result<()> {
        match self {
            Self::Auth { command } => command.handle_subcommand(settings),

            Self::Print(args) => print_command(args, settings).await,

            Self::Rpc(args) => rpc_command(args, settings).await,

            Self::Completions(args) => completions_command(args),
        }
    }
}

/// Trait for handling CLI subcommands.
pub trait SubcommandHandler {
    /// Handle the subcommand with the given settings.
    fn handle_subcommand(
        &self,
        settings: &Settings,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parse() {
        let cli = Cli::try_parse_from([
            "hallucinate",
            "print",
            "--model",
            "gpt-4o",
            "--shell",
            "/bin/bash",
            "--image",
            "diagram.png",
            "hello",
        ]);

        let cli = match cli {
            Ok(value) => value,
            Err(error) => panic!("Failed to parse CLI: {}", error),
        };

        if let Some(CliCommands::Print(args)) = &cli.command {
            assert_eq!(args.model, Some("gpt-4o".to_string()));
            assert_eq!(args.shell, Some("/bin/bash".to_string()));
            assert_eq!(args.images, vec![std::path::PathBuf::from("diagram.png")]);
            assert_eq!(args.prompt, "hello".to_string())
        }
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
        ]);

        let cli = match cli {
            Ok(value) => value,
            Err(error) => panic!("Failed to parse CLI: {}", error),
        };

        assert_eq!(cli.skills.len(), 2);
        assert!(cli.no_skills);
        assert_eq!(cli.prompt_templates.len(), 1);
        assert!(cli.no_prompt_templates);
        assert!(cli.debug);
    }
}
