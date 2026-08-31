//! Application settings.
//!
//! Manages persistent TOML configuration including model defaults, provider
//! settings, and credential storage.

use std::{
    collections::HashMap,
    fs,
    io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    ai::{
        auth::CredentialStore,
        types::{Api, Model, ModelCostRates},
    },
    config::paths,
};

/// A model defined under a user provider in `settings.toml`.
///
/// Model IDs are map keys rather than fields, which keeps the configuration
/// grouped by provider and makes it possible to use IDs containing characters
/// such as `/` when they are quoted in TOML.
/// A model defined under a user provider in `settings.toml`.
///
/// Model IDs are map keys rather than fields, which keeps the configuration
/// grouped by provider and makes it possible to use IDs containing characters
/// such as `/` when they are quoted in TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSettings {
    /// Human-readable name for the model.
    #[serde(default)]
    pub name: Option<String>,
    /// API variant used by the provider (e.g., OpenAI Chat Completions or Responses).
    #[serde(default = "default_api")]
    pub api: Api,
    /// Override the base URL for this specific model.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Whether this model supports reasoning/thinking output.
    #[serde(default)]
    pub reasoning: bool,
    /// Supported input modalities (e.g., "text", "image").
    #[serde(default = "default_input")]
    pub input: Vec<String>,
    /// Maximum context window size in tokens.
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    /// Maximum number of tokens the model can output.
    #[serde(default = "default_model_max_tokens")]
    pub max_tokens: u32,
    /// Cost rates per million tokens for input and output.
    #[serde(default)]
    pub cost: ModelCostRates,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            name: None,
            api: default_api(),
            base_url: None,
            reasoning: false,
            input: default_input(),
            context_window: default_context_window(),
            max_tokens: default_model_max_tokens(),
            cost: ModelCostRates::default(),
        }
    }
}

/// A user-defined provider and the models belonging to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// OpenAI-compatible endpoint. The model's `api` selects Chat
    /// Completions or Responses wire format.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional inline key. Prefer the credential store or environment
    /// variables for secrets; this is accepted for convenient local setups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Additional HTTP headers to send with every request.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    /// Models provided by this provider, keyed by model ID.
    #[serde(default)]
    pub models: HashMap<String, ModelSettings>,
}

/// Application-wide settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Default model to use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// Credential namespace for the OpenAI-compatible provider (usually
    /// "openai"). Defaults to `"openai"`.
    #[serde(default = "default_provider")]
    pub provider: String,

    /// Default base URL for the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Shell executable used by the bash tool. When unset, the user's SHELL
    /// environment variable (or the platform fallback) is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,

    /// Stored credentials.
    #[serde(default, skip_serializing_if = "CredentialStore::is_empty")]
    pub credential_store: CredentialStore,

    /// Provider-specific headers for the legacy/default provider.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub provider_headers: HashMap<String, String>,

    /// Providers and their models. Models are deliberately nested under a
    /// provider instead of using pi's flat provider/model catalog shape.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderSettings>,

    /// Request timeout in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Number of retries for transient LLM API failures.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Automatically resize image attachments to provider-friendly limits.
    #[serde(default = "default_auto_resize_images")]
    pub auto_resize_images: bool,

    /// Maximum number of turns.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Enable tool use by default.
    #[serde(default = "default_enable_tools")]
    pub enable_tools: bool,

    /// Enable context compaction.
    #[serde(default = "default_enable_compaction")]
    pub enable_compaction: bool,

    /// Show model thinking/reasoning in user-facing output.
    #[serde(default)]
    pub show_thinking: bool,

    /// Default temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Default max tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Whether these settings include a project-local override. This is kept
    /// out of serialization so credential commands can update the global
    /// file without copying project settings into it.
    #[serde(skip)]
    project_override_loaded: bool,
}

/// Generate a unique temporary file suffix using PID and nanosecond timestamp.
fn temporary_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());

    format!("{}-{}", std::process::id(), timestamp)
}

/// Set file permissions to owner-only (0o600) on Unix systems.
///
/// On non-Unix platforms this is a no-op.
fn set_owner_only_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Default credential provider namespace.
fn default_provider() -> String {
    "openai".to_string()
}

/// Default API variant for models.
fn default_api() -> Api {
    Api::OpenaiCompletions
}

/// Default input modalities for a model.
fn default_input() -> Vec<String> {
    vec!["text".to_string()]
}

/// Default context window size in tokens.
fn default_context_window() -> u32 {
    128_000
}

/// Default OpenAI-compatible base URL.
fn default_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

/// Default maximum output tokens for a model.
fn default_model_max_tokens() -> u32 {
    16_384
}

/// Default request timeout in milliseconds.
fn default_timeout_ms() -> u64 {
    600_000
}

/// Default number of retries for transient LLM API failures.
fn default_max_retries() -> u32 {
    3
}

/// Whether to automatically resize image attachments by default.
fn default_auto_resize_images() -> bool {
    true
}

/// Default maximum number of conversation turns.
fn default_max_turns() -> u32 {
    120
}

/// Whether to enable tool use by default.
fn default_enable_tools() -> bool {
    true
}

/// Whether to enable context compaction by default.
fn default_enable_compaction() -> bool {
    true
}

/// Merge a settings file into an already parsed TOML table.
fn merge_settings_file(
    merged: &mut toml::Table,
    path: &Path,
) -> Result<(), anyhow::Error> {
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&content).map_err(|error| {
        anyhow::anyhow!(
            "failed to parse settings file '{}': {error}",
            path.display()
        )
    })?;

    let toml::Value::Table(table) = value else {
        anyhow::bail!(
            "settings file '{}' must contain a TOML table",
            path.display()
        );
    };
    merge_toml_tables(merged, table);

    Ok(())
}

/// Recursively merge TOML tables, overlaying values from `overlay` into `base`.
///
/// Nested tables are merged recursively while scalar values and arrays are
/// replaced entirely.
fn merge_toml_tables(
    base: &mut toml::Table,
    overlay: toml::Table,
) {
    for (key, value) in overlay {
        if let Some(toml::Value::Table(base_table)) = base.get_mut(&key)
            && let toml::Value::Table(overlay_table) = value
        {
            merge_toml_tables(base_table, overlay_table);
        } else {
            base.insert(key, value);
        }
    }
}

fn deserialize_settings(merged: toml::Table) -> Result<Settings, anyhow::Error> {
    if merged.is_empty() {
        return Ok(Settings::default());
    }

    toml::Value::Table(merged)
        .try_into()
        .map_err(|error| anyhow::anyhow!("failed to deserialize merged settings: {error}"))
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_model: None,
            provider: default_provider(),
            base_url: None,
            shell: None,
            credential_store: CredentialStore::new(),
            provider_headers: HashMap::new(),
            providers: HashMap::new(),
            timeout_ms: default_timeout_ms(),
            max_retries: default_max_retries(),
            auto_resize_images: default_auto_resize_images(),
            max_turns: default_max_turns(),
            enable_tools: default_enable_tools(),
            enable_compaction: default_enable_compaction(),
            show_thinking: false,
            temperature: None,
            max_tokens: None,
            project_override_loaded: false,
        }
    }
}

impl Settings {
    /// Ensure that at least one provider was explicitly configured.
    ///
    /// The application does not fall back to a built-in provider or model.
    /// Keeping this check on settings also makes all run modes enforce the
    /// same configuration requirement.
    pub fn require_configured_providers(&self) -> Result<(), anyhow::Error> {
        if self.providers.is_empty() {
            anyhow::bail!(
                "No providers configured. Add a provider to settings.toml before starting hallucinate."
            );
        }

        Ok(())
    }

    /// Load the global settings without applying a project override.
    pub fn load_global() -> Result<Self, anyhow::Error> {
        let mut merged = toml::Table::new();

        if let Some(path) = paths::settings_file() {
            merge_settings_file(&mut merged, &path)?;
        }

        deserialize_settings(merged)
    }

    /// Load and merge the global settings with the project-local override.
    ///
    /// The global file is loaded first and the project file at
    /// `<cwd>/.hallucinate/settings.toml` is applied on top of it. TOML
    /// tables are merged recursively, so a project can override one provider
    /// or model field without having to repeat the rest of the global
    /// configuration. Scalar values and arrays in the project file replace
    /// their global counterparts.
    pub fn load() -> Result<Self, anyhow::Error> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());

        Self::load_from(cwd)
    }

    /// Load settings using an explicit project working directory.
    pub fn load_from(cwd: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let mut merged = toml::Table::new();

        if let Some(path) = paths::settings_file() {
            merge_settings_file(&mut merged, &path)?;
        }

        let project_path = paths::project_settings_file(cwd);
        merge_settings_file(&mut merged, &project_path)?;

        let mut settings = deserialize_settings(merged)?;
        settings.project_override_loaded = project_path.exists();

        Ok(settings)
    }

    /// Persist settings to the user's configuration file.
    ///
    /// The file is written with owner-only permissions and replaced
    /// atomically. This is important because credentials are stored in the
    /// settings file alongside the other application settings.
    pub fn save(&self) -> Result<(), anyhow::Error> {
        let path = paths::settings_file()
            .ok_or_else(|| anyhow::anyhow!("could not determine the settings file path"))?;

        self.save_to(&path)
    }

    /// Persist settings to a specific path.
    ///
    /// This is also useful to callers that maintain a separate configuration
    /// location, and keeps the file-writing behavior testable without
    /// modifying the user's real settings.
    pub fn save_to(
        &self,
        path: &Path,
    ) -> Result<(), anyhow::Error> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("settings path has no parent directory"))?;
        fs::create_dir_all(parent)?;

        let content = toml::to_string_pretty(self)?;

        let temporary_path = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("settings.toml"),
            temporary_suffix()
        ));

        fs::write(&temporary_path, content)?;
        set_owner_only_permissions(&temporary_path)?;

        if let Err(error) = fs::rename(&temporary_path, path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error.into());
        }

        Ok(())
    }

    /// Save an API key for a provider and persist the updated settings.
    pub fn save_api_key(
        &mut self,
        provider: &str,
        api_key: &str,
    ) -> Result<(), anyhow::Error> {
        let provider = provider.trim();
        if provider.is_empty() {
            anyhow::bail!("provider ID cannot be empty");
        }

        if api_key.trim().is_empty() {
            anyhow::bail!("API key cannot be empty");
        }

        if self.project_override_loaded {
            let mut global = Self::load_global()?;
            global.credential_store.set(provider, api_key);
            global.save()?;
        }
        self.credential_store.set(provider, api_key);

        if !self.project_override_loaded {
            self.save()?;
        }

        Ok(())
    }

    /// Remove a provider's stored API key and persist the updated settings.
    pub fn remove_api_key(
        &mut self,
        provider: &str,
    ) -> Result<bool, anyhow::Error> {
        let removed = if self.project_override_loaded {
            let mut global = Self::load_global()?;
            let removed = global.credential_store.remove(provider);

            if removed {
                global.save()?;
            }

            removed
        } else {
            self.credential_store.remove(provider)
        };

        self.credential_store.remove(provider);
        if removed && !self.project_override_loaded {
            self.save()?;
        }

        Ok(removed)
    }

    /// Return all models defined in provider groups, in deterministic order.
    pub fn configured_models(&self) -> Vec<Model> {
        let mut providers: Vec<_> = self.providers.iter().collect();
        providers.sort_by(|left, right| left.0.cmp(right.0));

        providers
            .into_iter()
            .flat_map(|(provider_id, provider)| {
                let provider_url = provider.base_url.clone().unwrap_or_else(|| {
                    if provider_id == &self.provider {
                        self.base_url.clone().unwrap_or_else(default_base_url)
                    } else {
                        default_base_url()
                    }
                });

                let mut models: Vec<_> = provider.models.iter().collect();
                models.sort_by(|left, right| left.0.cmp(right.0));
                models.into_iter().map(move |(model_id, config)| {
                    let mut model = Model::new(
                        model_id,
                        config.name.clone().unwrap_or_else(|| model_id.clone()),
                        config.api.clone(),
                        provider_id,
                        config
                            .base_url
                            .clone()
                            .unwrap_or_else(|| provider_url.clone()),
                        config.reasoning,
                        config.context_window,
                        config.max_tokens,
                    );
                    model.input = config.input.clone();
                    model.cost = config.cost.clone();

                    model
                })
            })
            .collect()
    }

    /// Get the effective API key for a provider from CLI override, environment,
    /// provider settings, or stored credentials.
    pub fn resolve_api_key_for(
        &self,
        provider: &str,
        cli_key: Option<&str>,
    ) -> Option<String> {
        if let Some(key) = cli_key.filter(|key| !key.is_empty()) {
            return Some(key.to_string());
        }

        for variable in ["HALLUCINATE_API_KEY", "OPENAI_API_KEY"] {
            if let Ok(key) = std::env::var(variable)
                && !key.is_empty()
            {
                return Some(key);
            }
        }

        // Provider-specific environment variables make multiple configured
        // providers practical without putting keys in the TOML file.
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
        if let Ok(key) = std::env::var(variable)
            && !key.is_empty()
        {
            return Some(key);
        }

        self.providers
            .get(provider)
            .and_then(|provider| provider.api_key.clone())
            .or_else(|| {
                self.credential_store
                    .get(provider)
                    .map(|c| c.api_key.clone())
            })
            .or_else(|| {
                (provider == self.provider).then(|| {
                    self.credential_store
                        .get("openai")
                        .map(|credential| credential.api_key.clone())
                })?
            })
    }

    /// Get the effective API key for the configured default provider.
    #[allow(dead_code)]
    pub fn resolve_api_key(
        &self,
        cli_key: Option<&str>,
    ) -> Option<String> {
        self.resolve_api_key_for(&self.provider, cli_key)
    }

    /// Get the effective base URL for the configured default provider.
    #[allow(dead_code)]
    pub fn resolve_base_url(
        &self,
        cli_url: Option<&str>,
    ) -> String {
        self.resolve_provider_base_url(&self.provider, cli_url, None)
    }

    /// Get the endpoint for a selected provider. A model-level endpoint is a
    /// final fallback and is useful for a provider with a single exceptional
    /// model.
    pub fn resolve_provider_base_url(
        &self,
        provider: &str,
        cli_url: Option<&str>,
        model_url: Option<&str>,
    ) -> String {
        cli_url
            .map(str::to_owned)
            .or_else(|| {
                self.providers
                    .get(provider)
                    .and_then(|p| p.base_url.clone())
            })
            .or_else(|| {
                (provider == self.provider)
                    .then(|| self.base_url.clone())
                    .flatten()
            })
            .or_else(|| model_url.map(str::to_owned))
            .unwrap_or_else(default_base_url)
    }

    /// Resolve a configured model by either `provider/model` or (when
    /// unambiguous) its model ID.
    pub fn find_configured_model(
        &self,
        selector: &str,
    ) -> Option<Model> {
        let models = self.configured_models();

        if let Some((provider, model_id)) = selector.split_once('/') {
            return models
                .into_iter()
                .find(|model| model.provider == provider && model.id == model_id);
        }

        let mut matches = models.into_iter().filter(|model| model.id == selector);

        let first = matches.next()?;

        matches.next().is_none().then_some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_load_from_toml() {
        let settings: Settings = toml::from_str(
            r#"
            default_model = "example/model"
            provider = "example"
            base_url = "https://example.test/v1"
            shell = "/bin/bash"
            timeout_ms = 30_000
            max_retries = 2
            auto_resize_images = false
            max_turns = 5
            enable_tools = false
            enable_compaction = false
            show_thinking = true
            temperature = 0.2
            max_tokens = 256

            [credential_store.credentials.example]
            provider_id = "example"
            api_key = "sk-test"
            stored_at = 0

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
        assert_eq!(settings.shell.as_deref(), Some("/bin/bash"));
        assert_eq!(settings.timeout_ms, 30_000);
        assert!(!settings.auto_resize_images);
        assert_eq!(settings.temperature, Some(0.2));
        assert!(settings.show_thinking);
        assert_eq!(settings.max_tokens, Some(256));
        assert_eq!(
            settings.provider_headers.get("x-test"),
            Some(&"enabled".to_string())
        );
        assert_eq!(settings.resolve_api_key(None).as_deref(), Some("sk-test"));
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
    fn saving_settings_persists_credentials() {
        let path = std::env::temp_dir().join(format!(
            "hallucinate-settings-test-{}-{}.toml",
            std::process::id(),
            temporary_suffix()
        ));

        let mut settings = Settings::default();
        settings.credential_store.set("openrouter", "sk-test");
        settings.save_to(&path).unwrap();

        let loaded: Settings = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(
            loaded
                .credential_store
                .get("openrouter")
                .map(|credential| credential.api_key.as_str()),
            Some("sk-test")
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn project_settings_merge_without_replacing_nested_global_settings() {
        let directory = std::env::temp_dir().join(format!(
            "hallucinate-settings-merge-{}-{}",
            std::process::id(),
            temporary_suffix()
        ));

        let project_directory = directory.join("project");
        fs::create_dir_all(project_directory.join(".hallucinate")).unwrap();

        let global_path = directory.join("global-settings.toml");
        let project_path = project_directory.join(".hallucinate/settings.toml");
        fs::write(
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

        fs::write(
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

        let _ = fs::remove_dir_all(directory);
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
        assert_eq!(model.input, vec!["text", "image"]);
        assert_eq!(model.context_window, 32_768);
        assert_eq!(model.max_tokens, 4096);
        assert_eq!(model.cost.input, 0.1);
        assert_eq!(model.cost.output, 0.2);
    }
}
