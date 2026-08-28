//! Hallucinate CLI - AI coding agent powered by LLMs.
//!
//! A terminal-based AI assistant that supports interactive, print, and RPC
//! modes.

mod agent;
mod ai;
mod cli;
mod config;
mod logging;
mod modes;
mod session;
mod tui;

use clap::Parser;
use cli::args::{Cli, Command};
use config::settings::Settings;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    // Ensure directories exist
    config::paths::ensure_directories()?;

    if cli.debug
        && let Some(path) = logging::init_debug_logging()?
    {
        eprintln!("Debug logging enabled: {}", path.display());
    }
    tracing::debug!(
        print_mode = cli.print.is_some(),
        rpc_mode = cli.rpc,
        interactive_mode = cli.interactive,
        "parsed command-line options"
    );

    // All modes share the same persisted settings and CLI overrides. Do not
    // replace a malformed settings file with defaults while modifying auth;
    // that could discard unrelated configuration.
    let settings = if cli.command.is_some() {
        Settings::load()?
    } else {
        Settings::load().unwrap_or_default()
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
