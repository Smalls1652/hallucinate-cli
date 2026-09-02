//! Configuration module.

use std::collections::HashMap;

use crate::ai::{model_catalog, provider::Provider, types::{Api, Model}};

pub mod paths;
pub mod settings;

#[derive(Clone)]
pub struct CommonArgs {
    /// Model to use as a configured model ID or "provider/model" pair.
    pub model: Option<String>,

    /// API key for the provider. Overrides environment variables and stored
    /// credentials. Environment fallback also accepts OPENAI_API_KEY.
    pub api_key: Option<String>,

    /// Base URL for the API (for OpenAI-compatible providers).
    pub base_url: Option<String>,

    /// Shell executable for the bash tool. Overrides the settings file and
    /// SHELL environment variable.
    pub shell: Option<String>,

    /// Print mode: send prompt and print the response, then exit.
    pub print: Option<String>,

    /// Run in RPC mode (JSON-RPC over stdin/stdout).
    pub rpc: bool,

    /// Run in interactive mode (default when no other mode is specified).
    pub interactive: bool,

    /// Resume this persisted session in interactive mode.
    pub session: Option<String>,

    /// Maximum number of tokens in the response.
    pub max_tokens: Option<u32>,

    /// Show model thinking/reasoning in output. Overrides the settings file.
    pub thinking: bool,

    /// Temperature for sampling (0.0 - 2.0).
    pub temperature: Option<f64>,

    /// System prompt to use.
    pub system_prompt: Option<String>,

    /// Attach an image to a print-mode prompt. May be repeated.
    pub images: Vec<std::path::PathBuf>,

    /// Load a skill Markdown file or a directory of skills. May be repeated.
    pub skills: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project skills.
    pub no_skills: bool,

    /// Load a prompt template Markdown file or directory. May be repeated.
    pub prompt_templates: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project prompt templates.
    pub no_prompt_templates: bool,

    /// Write diagnostic logs to the application cache directory.
    pub debug: bool,
}

/// Return the models declared by configured providers in deterministic order.
pub fn available_models(settings: &self::settings::Settings) -> Vec<Model> {
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
pub fn resolve_model_selector(
    selector: &str,
    settings: &self::settings::Settings,
    base_url: Option<&String>,
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
        settings.resolve_provider_base_url(provider, base_url.map(|x| x.as_str()), None),
        false,
        128_000,
        16_384,
    ))
}

/// Resolve the effective model shared by all run modes.
pub fn resolve_model(
    settings: &self::settings::Settings,
    model: Option<&String>,
    base_url: Option<&String>
) -> Result<Model, anyhow::Error> {
    settings.require_configured_providers()?;

    if let Some(model_str) = model {
        return resolve_model_selector(model_str, settings, base_url.as_deref())
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
pub fn resolve_shell(
    settings: &self::settings::Settings,
    shell: Option<String>
) -> Option<String> {
    shell.or_else(|| settings.shell.clone())
}

/// Resolve the response limit. A model's declared maximum is the default and
/// also caps global/CLI overrides so switching models cannot send an invalid
/// limit to a provider.
pub fn resolve_max_tokens(
    settings: &self::settings::Settings,
    max_tokens: Option<u32>,
    model: &Model,
) -> Option<u32> {
    Some(
        max_tokens
            .or(settings.max_tokens)
            .unwrap_or(model.max_tokens)
            .min(model.max_tokens),
    )
}

/// Resolve whether thinking/reasoning blocks should be shown to the user.
/// Command-line flags override the persisted setting.
pub fn resolve_show_thinking(
    settings: &self::settings::Settings,
    show_thinking: bool,
) -> bool {
    if show_thinking {
        show_thinking
    } else {
        settings.show_thinking
    }
}

/// Create an OpenAI-compatible provider for a particular model/provider group.
pub fn create_provider_for_model(
    settings: &self::settings::Settings,
    base_url: Option<&String>,
    api_key: Option<&String>,
    model: &Model,
) -> Result<Box<dyn Provider>, anyhow::Error> {
    use crate::ai::providers::openai_compat::{
        OpenaiCompatConfig,
        OpenaiCompatProvider,
    };

    let api_key = settings
        .resolve_api_key_for(&model.provider, api_key.map(|x| x.as_str()))
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
            base_url.map(|x| x.as_str()),
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
    use super::*;
    use super::settings::*;

    #[test]
    fn available_models_only_contains_configured_models() {
        let settings = Settings::default();
        assert!(available_models(&settings).is_empty());
    }
}
