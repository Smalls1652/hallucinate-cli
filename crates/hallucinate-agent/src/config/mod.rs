//! Configuration module.

mod error;
mod paths;
mod settings;
mod tools;

use std::collections::HashMap;

pub use error::*;
pub use paths::*;
pub use settings::*;
pub use tools::*;

use crate::{
    agent::types::AgentConfig,
    ai::{
        provider::Provider,
        types::{Api, Model, ReasoningEffort},
    },
};

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

    /// Shell executable for the shell tool.
    pub shell: Option<Vec<String>>,

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
) -> Result<Model, ConfigError> {
    settings.require_configured_providers()?;

    if let Some(model_str) = model {
        return resolve_model_selector(model_str, settings, base_url)
            .ok_or_else(|| ConfigError::NoModelsAvailable);
    }

    if let Some(default_model) = &settings.default_model
        && let Some(model) = resolve_model_selector(default_model, settings, None)
    {
        return Ok(model);
    }

    available_models(settings)
        .into_iter()
        .next()
        .ok_or_else(|| ConfigError::NoModelsAvailable)
}

/// Resolve the shell used by the shell tool. The explicit CLI option takes
/// precedence over the persisted setting; the tool falls back to the process
/// environment when both are unset.
pub fn resolve_shell(
    settings: &self::settings::Settings,
    shell: Option<Vec<String>>,
) -> Option<Vec<String>> {
    shell.or_else(|| settings.tools.builtin.shell.shell.clone())
}

/// Resolve the shell tool timeout. The explicit CLI option takes precedence
/// over the persisted setting; the tool falls back to the default (60 s)
/// when both are unset.
pub fn resolve_shell_timeout(
    settings: &self::settings::Settings,
    cli_timeout: Option<u64>,
) -> u64 {
    cli_timeout.unwrap_or(settings.tools.builtin.shell.timeout)
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
        enable_tools: settings.tools.enable_tools,
        disabled_tools: settings.tools.disable_tools.clone(),
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
) -> Result<Box<dyn Provider>, ConfigError> {
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
) -> Result<Box<dyn Provider>, ConfigError> {
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
        .ok_or_else(|| {
            ConfigError::NoApiKeySetForProvider(
                model.provider.clone(),
                model.provider.to_ascii_uppercase(),
            )
        })?;

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

    #[test]
    fn settings_load_from_toml() {
        let settings: Settings = toml::from_str(
            r#"
            default_model = "example/model"
            provider = "example"
            base_url = "https://example.test/v1"
            timeout_ms = 30_000
            max_retries = 2
            max_turns = 5
            enable_tools = false
            enable_compaction = false
            max_concurrent_tool_calls = 2
            show_thinking = true
            temperature = 0.2
            max_tokens = 256

            [session_title]
            enabled = true
            model = "example/title-model"
            generation_mode = "before"

            [provider_headers]
            x-test = "enabled"
            "#,
        )
        .unwrap();

        assert_eq!(settings.default_model.as_deref(), Some("example/model"));
        assert_eq!(
            settings.base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert!(settings.tools.mcp_servers.is_empty());
        assert_eq!(settings.timeout_ms, 30_000);
        assert_eq!(settings.temperature, Some(0.2));
        assert!(settings.show_thinking);
        assert_eq!(settings.max_concurrent_tool_calls, 2);
        assert_eq!(settings.max_tokens, Some(256));
        assert!(settings.session_title.enabled);
        assert_eq!(
            settings.session_title.model.as_deref(),
            Some("example/title-model")
        );
        assert_eq!(
            settings.session_title.generation_mode,
            SessionTitleGenerationMode::Before
        );
        assert_eq!(
            settings.provider_headers.get("x-test"),
            Some(&"enabled".to_string())
        );
        assert!(settings.resolve_api_key(None).is_none());
    }

    #[test]
    fn no_provider_configuration_is_rejected() {
        let error = Settings::default()
            .require_configured_providers()
            .unwrap_err();
        assert!(error.to_string().contains("No providers configured"));

        let mut settings = Settings::default();
        settings
            .providers
            .insert("local".to_string(), Default::default());
        assert!(settings.require_configured_providers().is_ok());
    }

    #[test]
    fn saving_settings_does_not_serialize_credentials() {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-settings-test-{}-{}.toml",
            std::process::id(),
            temporary_suffix()
        ));

        Settings::default().save_to(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        assert!(!content.contains("credential_store"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn project_settings_merge_without_replacing_nested_global_settings() {
        let directory = std::env::temp_dir().join(format!(
            "hallucinate-settings-merge-{}-{}",
            std::process::id(),
            temporary_suffix()
        ));

        let project_directory = directory.join("project");
        std::fs::create_dir_all(project_directory.join(".hallucinate")).unwrap();

        let global_path = directory.join("global-settings.toml");
        let project_path = project_directory.join(".hallucinate/settings.toml");
        std::fs::write(
            &global_path,
            r#"
            provider = "openai"
            timeout_ms = 30_000

            [providers.openai]
            base_url = "https://example.test/v1"

            [providers.openai.models.example]
            context_window = 32_768
            "#,
        )
        .unwrap();

        std::fs::write(
            &project_path,
            r#"
            default_model = "openai/example"
            timeout_ms = 5_000

            [providers.openai]
            headers = { x-project = "enabled" }
            "#,
        )
        .unwrap();

        let mut merged = toml::Table::new();
        merge_settings_file(&mut merged, &global_path).unwrap();
        merge_settings_file(&mut merged, &project_path).unwrap();

        let settings: Settings = toml::Value::Table(merged).try_into().unwrap();

        assert_eq!(settings.default_model.as_deref(), Some("openai/example"));
        assert_eq!(settings.timeout_ms, 5_000);

        let provider = settings.providers.get("openai").unwrap();

        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(
            provider.headers.get("x-project"),
            Some(&"enabled".to_string())
        );
        assert!(provider.models.contains_key("example"));

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn models_are_loaded_grouped_by_provider() {
        let settings: Settings = toml::from_str(
            r#"
            default_model = "local/llama-3"

            [providers.local]
            base_url = "http://localhost:1234/v1"

            [providers.local.models.llama-3]
            name = "Llama 3"
            api = "openai-responses"
            reasoning = true
            supports_reasoning_effort = true
            input = ["text", "image"]
            context_window = 32768
            max_tokens = 4096

            [providers.local.models.llama-3.cost]
            input = 0.1
            output = 0.2
            "#,
        )
        .unwrap();

        let model = settings.find_configured_model("local/llama-3").unwrap();

        assert_eq!(model.provider, "local");
        assert_eq!(model.base_url, "http://localhost:1234/v1");
        assert_eq!(model.name, "Llama 3");
        assert_eq!(model.api, Api::OpenaiResponses);
        assert!(model.reasoning);
        assert!(model.supports_reasoning_effort);
        assert_eq!(model.input, vec!["text", "image"]);
        assert_eq!(model.context_window, 32_768);
        assert_eq!(model.max_tokens, 4096);
        assert_eq!(model.cost.input, 0.1);
        assert_eq!(model.cost.output, 0.2);
    }

    #[test]
    fn model_cost_is_optional_and_omitted_when_saving() {
        let settings: Settings = toml::from_str(
            r#"
            [providers.local.models.llama-3]
            "#,
        )
        .unwrap();

        let model = settings.find_configured_model("local/llama-3").unwrap();
        assert_eq!(model.cost.input, 0.0);
        assert_eq!(model.cost.output, 0.0);

        let serialized = toml::to_string(&settings).unwrap();
        assert!(!serialized.contains("cost"));
    }

    #[test]
    fn builtin_tool_config_implies_defaults() {
        let settings: Settings = toml::from_str(
            r#"
            [tools.builtin.shell]
            timeout = 3_600

            [tools.builtin.read]
            max_text_file_bytes = 1_000
            "#,
        )
        .unwrap();

        assert_ne!(
            settings.tools.builtin.shell.timeout,
            DEFAULT_SHELL_COMMAND_TIMEOUT
        );
        assert_eq!(
            settings.tools.builtin.shell.max_tool_result_bytes,
            DEFAULT_MAX_TOOL_RESULT_BYTES
        );

        assert_eq!(settings.tools.builtin.read.auto_resize_images, true);
        assert_ne!(
            settings.tools.builtin.read.max_text_file_bytes,
            DEFAULT_MAX_TEXT_FILE_BYTES
        );
        assert_eq!(
            settings.tools.builtin.read.max_image_source_bytes,
            DEFAULT_MAX_IMAGE_SOURCE_BYTES
        );
    }
}
