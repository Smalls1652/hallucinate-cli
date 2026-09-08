//! Platform-native credentials used to authenticate with providers.
//!
//! API keys are stored by provider ID in the operating system's secure
//! credential store, rather than being serialized to the settings file.

macro_rules! credential_entry {
    ($provider_id:expr) => {
        keyring_core::Entry::new(
            format!("hallucinate__{}", $provider_id).as_str(),
            $provider_id,
        )
    };
}

/// Retrieve a stored API key credential for the given provider from the
/// platform keyring.
///
/// Returns the API key string on success, or an error if the credential cannot
/// be retrieved from the keyring.
pub fn get_stored_credential(provider_id: &str) -> anyhow::Result<String> {
    let entry = credential_entry!(provider_id)?;

    let api_key = entry.get_password()?;

    Ok(api_key)
}

/// Store an API key credential for the given provider in the platform keyring.
///
/// The key is persisted in the OS-native keyring under a name derived from the
/// provider ID.
pub fn set_stored_credential(
    provider_id: &str,
    api_key: &str,
) -> anyhow::Result<()> {
    let entry = credential_entry!(provider_id)?;

    entry.set_password(api_key)?;

    Ok(())
}

/// Remove a stored API key credential for the given provider from the platform
/// keyring.
///
/// Returns an error if the credential cannot be deleted.
pub fn remove_stored_credential(provider_id: &str) -> anyhow::Result<()> {
    let entry = credential_entry!(provider_id)?;

    entry.delete_credential()?;

    Ok(())
}

/// Set the default keyring store to the platform-native implementation.
///
/// On macOS this uses the keychain, on Linux the keyutils store, and on Windows
/// the native Windows keyring. In tests a mock store is used.
pub fn set_platform_keyring() -> anyhow::Result<()> {
    let store = cfg_select! {
        target_os = "macos" => apple_native_keyring_store::keychain::Store::new()?,
        target_os = "linux" => linux_keyutils_keyring_store::Store::new()?,
        target_os = "windows" => windows_native_keyring_store::Store::new()?,
        test => keyring_core::mock::Store::new()?,
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
