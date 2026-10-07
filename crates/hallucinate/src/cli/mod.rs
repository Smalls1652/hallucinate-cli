//! CLI argument parsing.

mod auth;
mod completions;
mod interactive;
mod print;
mod rpc;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use hallucinate_agent::ai::types::ReasoningEffort;

pub use self::{
    auth::AuthCommand,
    completions::{CompletionsArgs, completions_command},
    interactive::interactive_command,
    print::{PrintArgs, print_command},
    rpc::{RpcArgs, rpc_command},
};

pub const CLI_NAME: &str = "hallucinate";

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

    /// Reasoning effort for models that support it.
    #[arg(long = "reasoning-effort", value_parser = parse_reasoning_effort)]
    pub reasoning_effort: Option<ReasoningEffort>,

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
        let shell = match cli.shell {
            Some(ref value) => Some(
                value
                    .clone()
                    .split(" ")
                    .map(|item| item.to_string())
                    .collect::<Vec<String>>(),
            ),
            None => None,
        };

        Self {
            model: cli.model.clone(),
            api_key: cli.api_key.clone(),
            base_url: cli.base_url.clone(),
            shell: shell,
            max_tokens: cli.max_tokens,
            temperature: cli.temperature,
            reasoning_effort: cli.reasoning_effort,
            system_prompt: cli.system_prompt.clone(),
        }
    }
}

/// Parse a CLI reasoning-effort value with a user-facing validation error.
fn parse_reasoning_effort(value: &str) -> Result<ReasoningEffort, String> {
    value.parse()
}

impl Cli {
    /// Execute the CLI command, loading settings and dispatching to the
    /// appropriate run mode.
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            Some(subcommand) => match subcommand {
                CliCommands::Auth { command } => command.handle_subcommand(),

                CliCommands::Print(args) => print_command(args).await,

                CliCommands::Rpc(args) => rpc_command(args).await,

                CliCommands::Completions(args) => completions_command(args),
            },

            None => interactive_command(self).await,
        }
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
    Completions(CompletionsArgs),
}

/// Trait for handling CLI subcommands.
pub trait SubcommandHandler {
    /// Handle the subcommand.
    fn handle_subcommand(&self) -> Result<()>;
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
