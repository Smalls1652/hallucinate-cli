//! Shell completions command module.

use anyhow::Result;
use clap::{Args, CommandFactory};
use clap_complete::Shell;

use crate::cli::{CLI_NAME, Cli};

/// Arguments for shell completion. 
#[derive(Args, Debug, Clone)]
pub struct CompletionsArgs {
    /// The shell to generate completions for.
    pub shell: Shell
}

/// Generate shell completions for the configured shell.
pub fn completions_command(args: &CompletionsArgs) -> Result<()> {
    clap_complete::generate(
        args.shell,
        &mut Cli::command(),
        CLI_NAME,
        &mut std::io::stdout()
    );

    Ok(())
}
