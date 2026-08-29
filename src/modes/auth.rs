//! Command-line management of saved provider authentication.

use std::env;

use crate::{
    cli::args::AuthCommand,
    config::{paths, settings::Settings},
};

/// Run an authentication-store command without starting an agent session.
pub fn run(
    command: &AuthCommand,
    settings: &Settings,
    cli_api_key: Option<&str>,
) -> Result<(), anyhow::Error> {
    let mut settings = settings.clone();
    
    match command {
        AuthCommand::Set { provider, api_key } => {
            let key = api_key
                .as_deref()
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .or_else(|| cli_api_key.filter(|key| !key.is_empty()).map(str::to_owned))
                .or_else(|| environment_api_key(&settings, provider))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No API key supplied. Use --api-key or set HALLUCINATE_{}_API_KEY",
                        provider
                            .to_ascii_uppercase()
                            .replace(|character: char| !character.is_ascii_alphanumeric(), "_")
                    )
                })?;
            
            settings.save_api_key(provider, &key)?;
            
            let project_dir = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            
            println!(
                "Saved credentials for provider '{}' in {}.",
                provider,
                paths::settings_file()
                    .map(|path| paths::display_path(path, &project_dir))
                    .unwrap_or_else(|| "the settings file".to_string())
            );
        },
        
        AuthCommand::Remove { provider } => {
            if settings.remove_api_key(provider)? {
                println!("Removed saved credentials for provider '{}'.", provider);
            } else {
                println!("No saved credentials found for provider '{}'.", provider);
            }
        },
        
        AuthCommand::List => {
            let mut providers: Vec<_> = settings.credential_store.provider_ids().collect();
            providers.sort_unstable();
            if providers.is_empty() {
                println!("No saved provider credentials.");
            } else {
                for provider in providers {
                    println!("{}", provider);
                }
            }
        },
    }
    
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
    
    env::var(variable)
        .ok()
        .filter(|key| !key.is_empty())
        .or_else(|| {
            (provider == settings.provider)
                .then(|| env::var("HALLUCINATE_API_KEY").ok())
                .flatten()
                .filter(|key| !key.is_empty())
        })
        .or_else(|| {
            (provider == "openai")
                .then(|| env::var("OPENAI_API_KEY").ok())
                .flatten()
                .filter(|key| !key.is_empty())
        })
}
