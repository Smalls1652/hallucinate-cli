//! Hallucinate CLI - AI coding agent powered by LLMs.
//!
//! A terminal-based AI assistant that supports interactive, print, and RPC
//! modes.

mod cli;
mod logging;
mod modes;

use clap::Parser;
use cli::args::{Cli, Command};
use hallucinate_agent::config::settings::Settings;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    // Ensure directories exist
    hallucinate_agent::config::paths::ensure_directories()?;

    if cli.debug
        && let Some(path) = logging::init_debug_logging()?
    {
        let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        eprintln!(
            "Debug logging enabled: {}",
            hallucinate_agent::config::paths::display_path(path, &project_dir)
        );
    }

    tracing::debug!(
        print_mode = cli.print.is_some(),
        rpc_mode = cli.rpc,
        interactive_mode = cli.interactive,
        "parsed command-line options"
    );

    // All run modes share the same persisted settings and CLI overrides. Do
    // not replace a malformed settings file with defaults while modifying
    // auth; that could discard unrelated configuration.
    let settings = if cli.command.is_some() {
        // Authentication commands persist only the global settings. Project
        // overrides are runtime configuration and must not be copied into
        // the user's global settings file.
        Settings::load_global()?
    } else {
        Settings::load()?
    };

    // Authentication management is deliberately a separate command so it can
    // be used before an API key is available and without starting the TUI.
    if let Some(Command::Auth { command }) = &cli.command {
        modes::auth::run(command, &settings, cli.api_key.as_deref())?;
        return Ok(());
    }

    // Run the appropriate mode
    if cli.print.is_some() {
        modes::print::run(&cli, &settings).await?
    } else if cli.rpc {
        modes::rpc::run(&cli, &settings).await?;
    } else {
        modes::interactive::run(&cli, &settings).await?;
    }

    Ok(())
}
