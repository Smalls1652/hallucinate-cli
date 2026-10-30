//! Hallucinate CLI - Terminal-based AI agent
//!
//! A terminal-based AI assistant that supports interactive, print, and RPC
//! modes.

use clap::Parser;
use hallucinate::cli::Cli;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    // Ensure directories exist
    hallucinate_agent::config::ensure_directories()?;

    if cli.debug
        && let Some(path) = hallucinate::logging::init_debug_logging()?
    {
        let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        eprintln!(
            "Debug logging enabled: {}",
            hallucinate_agent::config::display_path(path, &project_dir)
        );
    }

    cli.run().await?;

    Ok(())
}
