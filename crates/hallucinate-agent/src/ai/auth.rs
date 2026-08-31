//! Credentials used to authenticate with providers.
//!
//! The credential store is embedded in
//! [`Settings`](crate::config::settings::Settings) and is serialized to the
//! user's settings file. It intentionally stores API keys by provider ID so
//! multiple OpenAI-compatible providers can be used in the same installation.
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

/// A stored API key credential.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    pub provider_id: String,
    pub api_key: String,
    pub stored_at: u64,
}

/// Credentials stored as part of `Settings` and serialized to its settings
/// file.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CredentialStore {
    #[serde(default)]
    credentials: HashMap<String, Credential>,
}

impl CredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(
        &self,
        provider_id: &str,
    ) -> Option<&Credential> {
        self.credentials.get(provider_id)
    }

    /// Store or replace the API key for a provider.
    pub fn set(
        &mut self,
        provider_id: impl Into<String>,
        api_key: impl Into<String>,
    ) {
        let provider_id = provider_id.into();

        self.credentials.insert(
            provider_id.clone(),
            Credential {
                provider_id,
                api_key: api_key.into(),
                stored_at: unix_timestamp(),
            },
        );
    }

    /// Remove credentials for a provider, returning whether they existed.
    pub fn remove(
        &mut self,
        provider_id: &str,
    ) -> bool {
        self.credentials.remove(provider_id).is_some()
    }

    /// Return provider IDs with stored credentials in no particular order.
    pub fn provider_ids(&self) -> impl Iterator<Item = &str> {
        self.credentials.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_store_roundtrips_values() {
        let mut store = CredentialStore::new();
        assert!(store.get("openai").is_none());

        store.set("openai", "sk-test-key");
        let cred = store.get("openai").unwrap();
        assert_eq!(cred.api_key, "sk-test-key");
        assert_eq!(cred.provider_id, "openai");
        assert!(cred.stored_at > 0);

        assert!(store.remove("openai"));
        assert!(!store.remove("openai"));
        assert!(store.get("openai").is_none());
    }

    #[test]
    fn setting_a_provider_replaces_its_existing_key() {
        let mut store = CredentialStore::new();
        store.set("local", "first");
        store.set("local", "second");

        assert_eq!(store.get("local").unwrap().api_key, "second");
        assert_eq!(store.provider_ids().collect::<Vec<_>>(), vec!["local"]);
    }
}
