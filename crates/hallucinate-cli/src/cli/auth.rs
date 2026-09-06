//! Command-line management of saved provider authentication.

use anyhow::Result;
use clap::{Args, Subcommand};
use hallucinate_agent::config::{paths, settings::Settings};

use crate::cli::SubcommandHandler;

/// Operations on the persisted provider credential store.
#[derive(Debug, Clone, Subcommand)]
pub enum AuthCommand {
    /// Save or replace an API key. If omitted, a configured environment
    /// variable is used (for example HALLUCINATE_OPENAI_API_KEY).
    Set(AuthSetArgs),

    /// Remove a provider's saved API key.
    Remove(AuthRemoveArgs),

    /// List providers with saved credentials without revealing their keys.
    List,
}

impl SubcommandHandler for AuthCommand {
    fn handle_subcommand(
        &self,
        settings: &Settings,
    ) -> anyhow::Result<()> {
        match self {
            AuthCommand::Set(args) => auth_set_command(args, settings),

            AuthCommand::Remove(args) => auth_remove_command(args, settings),

            AuthCommand::List => auth_list_command(settings),
        }
    }
}

/// Arguments for saving an API key.
#[derive(Args, Debug, Clone)]
pub struct AuthSetArgs {
    /// Provider ID, such as llamacpp, openai, or openrouter.
    pub provider: String,

    /// API key. Prefer an environment variable when possible so the key
    /// is not recorded in shell history.
    #[arg(long = "api-key", env = "HALLUCINATE_API_KEY")]
    pub api_key: Option<String>,
}

/// Save an API key for a provider.
pub fn auth_set_command(
    args: &AuthSetArgs,
    settings: &Settings,
) -> Result<()> {
    let mut settings = settings.clone();

    let key = args
        .api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .or_else(|| environment_api_key(&settings, &args.provider))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No API key supplied. Use --api-key or set HALLUCINATE_{}_API_KEY",
                &args
                    .provider
                    .to_ascii_uppercase()
                    .replace(|character: char| !character.is_ascii_alphanumeric(), "_")
            )
        })?;

    settings.save_api_key(&args.provider, &key)?;

    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    println!(
        "Saved credentials for provider '{}' in {}.",
        &args.provider,
        paths::settings_file()
            .map(|path| paths::display_path(path, &project_dir))
            .unwrap_or_else(|| "the settings file".to_string())
    );

    Ok(())
}

/// Find a key from the provider-specific or legacy environment variables.
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
pub fn auth_remove_command(
    args: &AuthRemoveArgs,
    settings: &Settings,
) -> Result<()> {
    let mut settings = settings.clone();

    if settings.remove_api_key(&args.provider)? {
        println!(
            "Removed saved credentials for provider '{}'.",
            &args.provider
        );
    } else {
        println!(
            "No saved credentials found for provider '{}'.",
            &args.provider
        );
    }

    Ok(())
}

/// List providers that have saved credentials.
pub fn auth_list_command(settings: &Settings) -> Result<()> {
    let mut providers: Vec<_> = settings.credential_store.provider_ids().collect();
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
