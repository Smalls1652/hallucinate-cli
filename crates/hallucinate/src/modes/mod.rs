//! Run modes for the application.

use std::collections::HashMap;

use hallucinate_agent::{
    ai::{
        model_catalog,
        provider::Provider,
        types::{Api, Model},
    },
    config::settings::Settings,
};

use crate::cli::args::Cli;

pub mod auth;
pub mod commands;
pub mod interactive;
pub mod print;
pub mod rpc;

/// Return the models declared by configured providers in deterministic order.
pub(crate) fn available_models(settings: &Settings) -> Vec<Model> {
    let mut models = model_catalog::all_models();
    models.extend(settings.configured_models());
    models.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.id.cmp(&right.id))
    });
    models
}

/// Resolve a model selector using provider-grouped user configuration first.
/// Unknown model IDs are supported for configured OpenAI-compatible providers.
pub(crate) fn resolve_model_selector(
    selector: &str,
    settings: &Settings,
    cli_base_url: Option<&str>,
) -> Option<Model> {
    if let Some(model) = settings.find_configured_model(selector) {
        return Some(model);
    }
    if let Some(model) = model_catalog::resolve_model(selector) {
        return Some(model);
    }

    // An unknown selector is still useful for a configured provider whose
    // model catalog is intentionally minimal. An unqualified selector uses
    // the configured default provider rather than implicitly selecting OpenAI.
    let (provider, model_id) = selector
        .split_once('/')
        .unwrap_or((&settings.provider, selector));
    if !settings.providers.contains_key(provider) {
        return None;
    }
    Some(Model::new(
        model_id,
        model_id,
        Api::OpenaiCompletions,
        provider,
        settings.resolve_provider_base_url(provider, cli_base_url, None),
        false,
        128_000,
        16_384,
    ))
}

/// Resolve the effective model shared by all run modes.
pub(crate) fn resolve_model(
    cli: &Cli,
    settings: &Settings,
) -> Result<Model, anyhow::Error> {
    settings.require_configured_providers()?;

    if let Some(model_str) = &cli.model {
        return resolve_model_selector(model_str, settings, cli.base_url.as_deref())
            .ok_or_else(|| anyhow::anyhow!("No models available"));
    }

    if let Some(default_model) = &settings.default_model
        && let Some(model) = resolve_model_selector(default_model, settings, None)
    {
        return Ok(model);
    }

    available_models(settings)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No models available"))
}

/// Resolve the shell used by the bash tool. The explicit CLI option takes
/// precedence over the persisted setting; the tool falls back to the process
/// environment when both are unset.
pub(crate) fn resolve_shell<'a>(
    cli: &'a Cli,
    settings: &'a Settings,
) -> Option<&'a str> {
    cli.shell.as_deref().or(settings.shell.as_deref())
}

/// Resolve the response limit. A model's declared maximum is the default and
/// also caps global/CLI overrides so switching models cannot send an invalid
/// limit to a provider.
pub(crate) fn resolve_max_tokens(
    cli: &Cli,
    settings: &Settings,
    model: &Model,
) -> Option<u32> {
    Some(
        cli.max_tokens
            .or(settings.max_tokens)
            .unwrap_or(model.max_tokens)
            .min(model.max_tokens),
    )
}

/// Resolve whether thinking/reasoning blocks should be shown to the user.
/// Command-line flags override the persisted setting.
pub(crate) fn resolve_show_thinking(
    cli: &Cli,
    settings: &Settings,
) -> bool {
    if cli.thinking {
        true
    } else if cli.no_thinking {
        false
    } else {
        settings.show_thinking
    }
}

/// Create an OpenAI-compatible provider for a particular model/provider group.
pub(crate) fn create_provider_for_model(
    cli: &Cli,
    settings: &Settings,
    model: &Model,
) -> Result<Box<dyn Provider>, anyhow::Error> {
    use hallucinate_agent::ai::providers::openai_compat::{
        OpenaiCompatConfig,
        OpenaiCompatProvider,
    };

    let api_key = settings
        .resolve_api_key_for(&model.provider, cli.api_key.as_deref())
        .or_else(|| {
            // Local OpenAI-compatible servers commonly do not require
            // authentication. A declared provider group opts into an empty
            // key; undeclared providers retain the helpful configuration
            // error used by the normal OpenAI path.
            settings
                .providers
                .contains_key(&model.provider)
                .then(|| String::new())
        })
        .ok_or_else(|| anyhow::anyhow!("No API key configured for provider '{}'. Set HALLUCINATE_API_KEY, HALLUCINATE_{}_API_KEY, configure a credential, or use --api-key", model.provider, model.provider.to_ascii_uppercase()))?;
    let mut headers: HashMap<String, String> = if model.provider == settings.provider {
        settings.provider_headers.clone()
    } else {
        HashMap::new()
    };
    if let Some(provider) = settings.providers.get(&model.provider) {
        headers.extend(provider.headers.clone());
    }
    let config = OpenaiCompatConfig {
        base_url: settings.resolve_provider_base_url(
            &model.provider,
            cli.base_url.as_deref(),
            Some(&model.base_url),
        ),
        api_key,
        headers,
        timeout_ms: settings.timeout_ms,
        max_retries: settings.max_retries,
    };

    Ok(Box::new(OpenaiCompatProvider::new(config)))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn model_resolution_requires_a_configured_provider() {
        let cli = Cli::parse_from(["hallucinate"]);
        let error = resolve_model(&cli, &Settings::default()).unwrap_err();
        assert!(error.to_string().contains("No providers configured"));
    }

    #[test]
    fn available_models_only_contains_configured_models() {
        let settings = Settings::default();
        assert!(available_models(&settings).is_empty());
    }
}
