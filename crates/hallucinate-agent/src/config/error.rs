use std::path::PathBuf;

use crate::ai::auth::KeyringAuthError;

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    /// A generic I/O error.
    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),

    /// A TOML deserialization error.
    #[error("TOML deserialization error: {0}")]
    TomlDeserializationError(#[from] toml::de::Error),

    /// A TOML serialization error.
    #[error("TOML serialization error: {0}")]
    TomlSerializationError(#[from] toml::ser::Error),

    /// A keyring authentication error.
    #[error("{0}")]
    KeyringError(#[from] KeyringAuthError),

    /// Failed to parse a settings file at the given path with a TOML
    /// deserialization error.
    #[error("failed to parse settings file '{0:?}': {1}")]
    SettingsParseFailure(PathBuf, #[source] toml::de::Error),

    /// The settings file must contain a TOML table.
    #[error("settings file '{0:?}' must contain a TOML table")]
    NoTomlTable(PathBuf),

    /// Failed to deserialize the merged settings.
    #[error("failed to deserialize merged settings: {0}")]
    MergedSettingsParseFailure(#[source] toml::de::Error),

    /// No providers are configured in the settings file.
    #[error(
        "No providers configured. Add a provider to settings.toml before starting hallucinate."
    )]
    NoProvidersConfigured,

    /// The settings file path could not be determined.
    #[error("could not determine the settings file path")]
    SettingsFilePathResolutionFailure,

    /// The settings file path has no parent directory.
    #[error("settings path has no parent directory")]
    SettingsFilePathHasNoParent,

    /// A required API key property is missing.
    #[error("{0} cannot be empty")]
    MissingApiKeyProperty(ApiKeyProperty),

    /// No models are available.
    #[error("No models available")]
    NoModelsAvailable,

    /// No API key is set for the given provider.
    #[error(
        "No API key configured for provider '{0}'. Set HALLUCINATE_API_KEY, HALLUCINATE_{1}_API_KEY, configure a credential, or use --api-key"
    )]
    NoApiKeySetForProvider(String, String),
}

#[derive(Debug)]
/// Indicates which property is missing for an API key.
pub enum ApiKeyProperty {
    /// The provider ID.
    ProviderId,
    /// The API key.
    ApiKey,
}

impl std::fmt::Display for ApiKeyProperty {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let value = match self {
            ApiKeyProperty::ProviderId => "provider ID",
            ApiKeyProperty::ApiKey => "API key",
        };

        write!(f, "{value}")
    }
}
