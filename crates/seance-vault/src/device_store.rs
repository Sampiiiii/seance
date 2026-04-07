// Owns platform-backed storage for the per-device vault wrapping secret.
use std::sync::Arc;

#[cfg(not(target_os = "macos"))]
use keyring::Entry;
use thiserror::Error;

pub(crate) const KEYRING_SERVICE_NAME: &str = "com.seance.vault";

#[derive(Debug, Error)]
pub enum DeviceSecretError {
    #[error("key store backend error: {0}")]
    Backend(String),
    #[error("device secret is missing")]
    MissingSecret,
    #[error("device secret backend is unavailable in this build")]
    UnavailableInThisBuild,
}

pub trait DeviceSecretStore: Send + Sync {
    fn get_secret(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceSecretError>;
    fn set_secret(&self, account: &str, secret: &[u8]) -> Result<(), DeviceSecretError>;
}

pub(crate) fn default_device_secret_store() -> Arc<dyn DeviceSecretStore> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacosBiometricDeviceSecretStore::default())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(KeyringDeviceSecretStore)
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
struct KeyringDeviceSecretStore;

#[cfg(not(target_os = "macos"))]
impl DeviceSecretStore for KeyringDeviceSecretStore {
    fn get_secret(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceSecretError> {
        let entry = Entry::new(KEYRING_SERVICE_NAME, account)
            .map_err(|err| DeviceSecretError::Backend(err.to_string()))?;

        match entry.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(DeviceSecretError::Backend(err.to_string())),
        }
    }

    fn set_secret(&self, account: &str, secret: &[u8]) -> Result<(), DeviceSecretError> {
        let entry = Entry::new(KEYRING_SERVICE_NAME, account)
            .map_err(|err| DeviceSecretError::Backend(err.to_string()))?;
        entry
            .set_secret(secret)
            .map_err(|err| DeviceSecretError::Backend(err.to_string()))
    }
}

#[cfg(target_os = "macos")]
mod macos;
