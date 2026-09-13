//! Command-line management of saved provider authentication.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use hallucinate_agent::config::settings::Settings;

use crate::cli::SubcommandHandler;

/// Operations on the persisted provider credential store.
#[derive(Debug, Clone, Subcommand)]
pub enum AuthCommand {
    /// Save or replace an API key. If omitted, a configured environment
    /// variable is used (for example HALLUCINATE_OPENAI_API_KEY); otherwise,
    /// the command prompts for a masked key.
    Set(AuthSetArgs),

    /// Remove a provider's saved API key.
    Remove(AuthRemoveArgs),

    /// List providers with saved credentials without revealing their keys.
    List,
}

impl SubcommandHandler for AuthCommand {
    fn handle_subcommand(&self) -> anyhow::Result<()> {
        match self {
            AuthCommand::Set(args) => auth_set_command(args),

            AuthCommand::Remove(args) => auth_remove_command(args),

            AuthCommand::List => auth_list_command(),
        }
    }
}

/// Arguments for saving an API key.
#[derive(Args, Debug, Clone)]
pub struct AuthSetArgs {
    /// Provider ID, such as llamacpp, openai, or openrouter.
    pub provider: String,

    /// API key. Prefer an environment variable when possible so the key
    /// is not recorded in shell history. If omitted and no configured
    /// environment variable is available, the command prompts for it.
    #[arg(long = "api-key", env = "HALLUCINATE_API_KEY")]
    pub api_key: Option<String>,
}

/// Save an API key for a provider.
pub fn auth_set_command(args: &AuthSetArgs) -> Result<()> {
    let settings = Settings::load_global()?;

    let key = args
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .or_else(|| environment_api_key(&settings, &args.provider))
        .map(Ok)
        .unwrap_or_else(prompt_for_api_key)?;

    settings.save_api_key(&args.provider, &key)?;

    println!(
        "Saved credentials for provider '{}' in the platform keyring.",
        args.provider,
    );

    Ok(())
}

/// Prompt for a masked API key, rejecting an empty response.
fn prompt_for_api_key() -> Result<String> {
    let config = rpassword::ConfigBuilder::new()
        .password_feedback_mask('*')
        .build();

    let key = rpassword::prompt_password_with_config("API key: ", config)?;

    if key.is_empty() {
        bail!("API key cannot be empty");
    }

    Ok(key)
}

/// Stored credentials are intentionally not used as a fallback for `set`:
/// running `auth set` should either receive a new key or explicitly use the
/// environment, rather than silently re-saving the old value.
fn environment_api_key(
    settings: &Settings,
    provider: &str,
) -> Option<String> {
    let variable = format!(
        "HALLUCINATE_{}_API_KEY",
        provider
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    );

    std::env::var(variable)
        .ok()
        .filter(|key| !key.is_empty())
        .or_else(|| {
            (provider == settings.provider)
                .then(|| std::env::var("HALLUCINATE_API_KEY").ok())
                .flatten()
                .filter(|key| !key.is_empty())
        })
        .or_else(|| {
            (provider == "openai")
                .then(|| std::env::var("OPENAI_API_KEY").ok())
                .flatten()
                .filter(|key| !key.is_empty())
        })
}

/// Arguments for removing a saved API key.
#[derive(Args, Debug, Clone)]
pub struct AuthRemoveArgs {
    /// Provider ID, such as llamacpp, openai, or openrouter.
    pub provider: String,
}

/// Remove a saved API key for a provider.
pub fn auth_remove_command(args: &AuthRemoveArgs) -> Result<()> {
    let settings = Settings::load_global()?;

    if settings.remove_api_key(&args.provider)? {
        println!(
            "Removed saved credentials for provider '{}'.",
            args.provider
        );
    } else {
        println!(
            "No saved credentials found for provider '{}'.",
            args.provider
        );
    }

    Ok(())
}

/// List providers that have saved credentials.
pub fn auth_list_command() -> Result<()> {
    let settings = Settings::load_global()?;

    let mut providers = settings.providers_with_stored_api_keys()?;
    providers.sort_unstable();

    if providers.is_empty() {
        println!("No saved provider credentials.");
    } else {
        for provider in providers {
            println!("{}", provider);
        }
    }
    Ok(())
}
