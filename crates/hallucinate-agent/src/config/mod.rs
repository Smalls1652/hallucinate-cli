//! Configuration module.

use std::collections::HashMap;

use crate::{
    agent::types::AgentConfig,
    ai::{
        provider::Provider,
        types::{Api, Model, ReasoningEffort},
    },
};

pub mod paths;
pub mod settings;

/// Command-line values which affect runtime model and agent configuration.
///
/// Frontend-only choices such as the selected mode, session, or presentation
/// settings remain in the application that owns them.
#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    /// Selected configured model or `provider/model` pair.
    pub model: Option<String>,

    /// Explicit provider credential.
    pub api_key: Option<String>,

    /// Explicit OpenAI-compatible provider endpoint.
    pub base_url: Option<String>,

    /// Shell executable for the bash tool.
    pub shell: Option<String>,

    /// Maximum response tokens.
    pub max_tokens: Option<u32>,

    /// Sampling temperature.
    pub temperature: Option<f64>,

    /// Requested model reasoning effort.
    pub reasoning_effort: Option<ReasoningEffort>,

    /// Explicit system prompt.
    pub system_prompt: Option<String>,
}

/// Return the models declared by configured providers in deterministic order.
pub fn available_models(settings: &self::settings::Settings) -> Vec<Model> {
    let mut models = settings.configured_models();

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
    base_url: Option<&String>,
) -> Result<Model, anyhow::Error> {
    settings.require_configured_providers()?;

    if let Some(model_str) = model {
        return resolve_model_selector(model_str, settings, base_url)
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
    shell: Option<String>,
) -> Option<String> {
    shell.or_else(|| settings.shell.clone())
}

/// Resolve the bash tool timeout. The explicit CLI option takes precedence
/// over the persisted setting; the tool falls back to the default (60 s)
/// when both are unset.
pub fn resolve_shell_timeout(
    settings: &self::settings::Settings,
    cli_timeout: Option<u64>,
) -> u64 {
    cli_timeout.unwrap_or(settings.shell_timeout)
}

/// Derive agent request configuration from persisted settings, a selected
/// model, and explicit runtime overrides.
pub fn agent_config(
    settings: &self::settings::Settings,
    model: &Model,
    overrides: &RuntimeOverrides,
) -> AgentConfig {
    AgentConfig {
        max_turns: settings.max_turns,
        max_context_tokens: model.context_window,
        enable_compaction: settings.enable_compaction,
        enable_tools: settings.enable_tools,
        temperature: overrides.temperature.or(settings.temperature),
        reasoning_effort: model.supports_reasoning_effort.then(|| {
            overrides
                .reasoning_effort
                .unwrap_or_else(ReasoningEffort::default_for_supported_model)
        }),
        max_tokens: resolve_max_tokens(settings, overrides.max_tokens, model),
        include_usage: settings
            .providers
            .get(&model.provider)
            .and_then(|provider| provider.include_usage),
        include_tool_definitions: true,
        max_concurrent_tool_calls: settings.max_concurrent_tool_calls,
    }
}

/// Create the provider selected by a model using runtime override precedence.
pub fn create_provider(
    settings: &self::settings::Settings,
    overrides: &RuntimeOverrides,
    model: &Model,
) -> Result<Box<dyn Provider>, anyhow::Error> {
    create_provider_for_model(
        settings,
        overrides.base_url.as_ref(),
        overrides.api_key.as_ref(),
        model,
    )
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
    use crate::ai::providers::openai_compat::{OpenaiCompatConfig, OpenaiCompatProvider};

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
                .then(String::new)
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
    use super::{settings::*, *};

    #[test]
    fn available_models_only_contains_configured_models() {
        let settings = Settings::default();
        assert!(available_models(&settings).is_empty());
    }

    #[test]
    fn agent_config_prefers_runtime_temperature_over_settings() {
        let mut settings = Settings::default();
        settings.temperature = Some(0.2);

        let model = Model::new(
            "test",
            "Test",
            Api::OpenaiCompletions,
            "openai",
            "https://example.test/v1",
            false,
            128_000,
            4_096,
        );

        let overrides = RuntimeOverrides {
            temperature: Some(0.8),
            max_tokens: Some(8_000),
            ..RuntimeOverrides::default()
        };

        let config = agent_config(&settings, &model, &overrides);

        assert_eq!(config.temperature, Some(0.8));
        assert_eq!(config.max_tokens, Some(4_096));
    }
}
