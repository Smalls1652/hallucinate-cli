//! Platform-native credentials used to authenticate with providers.
//!
//! API keys are serialized as a single provider-ID-to-key JSON object in one
//! operating-system credential, rather than being serialized to the settings
//! file or stored in separate keyring entries.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "hallucinate";
const KEYRING_ACCOUNT: &str = "hallucinate";

/// API keys persisted together in the platform keyring.
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredCredentials {
    #[serde(flatten)]
    api_keys: HashMap<String, String>,
}

/// Return the single credential entry used by hallucinate.
fn credential_entry() -> Result<keyring_core::Entry, keyring_core::Error> {
    keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
}

/// Load all provider API keys from the platform keyring.
fn load_stored_credentials() -> anyhow::Result<StoredCredentials> {
    let entry = credential_entry()?;
    let credentials = entry.get_password()?;

    serde_json::from_str(&credentials)
        .map_err(|error| anyhow::anyhow!("failed to parse stored keyring credentials: {error}"))
}

/// Save all provider API keys to the platform keyring.
fn save_stored_credentials(credentials: &StoredCredentials) -> anyhow::Result<()> {
    let entry = credential_entry()?;
    let credentials = serde_json::to_string(credentials)
        .map_err(|error| anyhow::anyhow!("failed to serialize keyring credentials: {error}"))?;

    entry.set_password(&credentials)?;

    Ok(())
}

/// Retrieve a stored API key credential for the given provider from the
/// platform keyring.
///
/// Returns an error if the keyring entry or the provider's credential cannot
/// be retrieved.
pub fn get_stored_credential(provider_id: &str) -> anyhow::Result<String> {
    let credentials = load_stored_credentials()?;

    credentials
        .api_keys
        .get(provider_id)
        .cloned()
        .ok_or_else(|| keyring_core::Error::NoEntry.into())
}

/// Store an API key credential for the given provider in the platform keyring.
///
/// All provider IDs and API keys are persisted together as JSON in the single
/// `hallucinate` keyring entry.
pub fn set_stored_credential(
    provider_id: &str,
    api_key: &str,
) -> anyhow::Result<()> {
    let mut credentials = match load_stored_credentials() {
        Ok(credentials) => credentials,
        Err(error) if is_missing_keyring_entry(&error) => StoredCredentials::default(),
        Err(error) => return Err(error),
    };

    credentials
        .api_keys
        .insert(provider_id.to_string(), api_key.to_string());
    save_stored_credentials(&credentials)
}

/// Remove a stored API key credential for the given provider from the platform
/// keyring.
///
/// The shared keyring entry is deleted when the removed credential was its last
/// provider API key.
pub fn remove_stored_credential(provider_id: &str) -> anyhow::Result<()> {
    let mut credentials = load_stored_credentials()?;

    if credentials.api_keys.remove(provider_id).is_none() {
        return Err(keyring_core::Error::NoEntry.into());
    }

    if credentials.api_keys.is_empty() {
        credential_entry()?.delete_credential()?;
    } else {
        save_stored_credentials(&credentials)?;
    }

    Ok(())
}

/// Return whether a keyring operation failed because the shared entry is
/// absent.
fn is_missing_keyring_entry(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<keyring_core::Error>(),
        Some(keyring_core::Error::NoEntry)
    )
}

/// Set the default keyring store to the platform-native implementation.
///
/// On macOS this uses the keychain, on Linux the keyutils store, and on Windows
/// the native Windows keyring. In tests a mock store is used.
pub fn set_platform_keyring() -> anyhow::Result<()> {
    let store = cfg_select! {
        test => keyring_core::mock::Store::new()?,
        target_os = "macos" => apple_native_keyring_store::keychain::Store::new()?,
        target_os = "linux" => linux_keyutils_keyring_store::Store::new()?,
        target_os = "windows" => windows_native_keyring_store::Store::new()?,
        _ => panic!("Unsupported platform :("),
    };

    keyring_core::set_default_store(store);

    Ok(())
}

/// Clear the default keyring store, releasing the platform-native keyring.
pub fn release_platform_keyring() -> anyhow::Result<()> {
    keyring_core::unset_default_store();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_provider_api_keys_in_one_json_credential() {
        set_platform_keyring().unwrap();

        set_stored_credential("openai", "openai-key").unwrap();
        set_stored_credential("local", "local-key").unwrap();

        let stored_json = credential_entry().unwrap().get_password().unwrap();
        let stored_credentials: HashMap<String, String> =
            serde_json::from_str(&stored_json).unwrap();
        assert_eq!(
            stored_credentials,
            HashMap::from([
                ("openai".to_string(), "openai-key".to_string()),
                ("local".to_string(), "local-key".to_string()),
            ])
        );
        assert_eq!(get_stored_credential("openai").unwrap(), "openai-key");
        assert_eq!(get_stored_credential("local").unwrap(), "local-key");

        remove_stored_credential("openai").unwrap();
        assert!(is_missing_keyring_entry(
            &get_stored_credential("openai").unwrap_err()
        ));
        assert_eq!(get_stored_credential("local").unwrap(), "local-key");

        remove_stored_credential("local").unwrap();
        assert!(is_missing_keyring_entry(
            &credential_entry()
                .unwrap()
                .get_password()
                .unwrap_err()
                .into()
        ));

        release_platform_keyring().unwrap();
    }
}
