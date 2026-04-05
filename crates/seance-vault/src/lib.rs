mod crypto;
mod kdf;
mod model;
mod storage;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use keyring::Entry;
use rusqlite::Connection;
use secrecy::ExposeSecret;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crypto::{SecretKey, decrypt, encrypt};
use kdf::KdfParams;
pub use model::{
    DeviceEnrollment, HostConfig, HostSummary, RecordKind, RecoveryBundle, UnlockMethod,
    VaultHeader, VaultStatus,
};
use model::{EncryptedRecord, RECORD_SCHEMA_VERSION, VAULT_SCHEMA_VERSION};
pub use secrecy::SecretString;

const DEVICE_STATE_KEY: &str = "local_device_id";
const KEYRING_SERVICE_NAME: &str = "com.seance.vault";

pub type VaultResult<T> = Result<T, VaultError>;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("vault I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("vault JSON error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("device key store error: {0}")]
    DeviceSecret(#[from] DeviceSecretError),
    #[error("failed to derive passphrase key: {message}")]
    PassphraseDerivationFailed { message: String },
    #[error("invalid KDF configuration: {message}")]
    InvalidKdfConfig { message: String },
    #[error("failed to encrypt vault material: {source}")]
    EncryptionFailed {
        source: chacha20poly1305::aead::Error,
    },
    #[error("failed to decrypt vault material: {source}")]
    DecryptionFailed {
        source: chacha20poly1305::aead::Error,
    },
    #[error("cipher initialization failed")]
    CipherInitFailed,
    #[error("vault has not been initialized yet")]
    VaultNotInitialized,
    #[error("vault is already initialized")]
    VaultAlreadyInitialized,
    #[error("vault is locked")]
    VaultLocked,
    #[error("invalid key length: expected {expected} bytes, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    #[error("invalid nonce length: expected {expected} bytes, got {actual}")]
    InvalidNonceLength { expected: usize, actual: usize },
    #[error("vault schema version {version} is not supported")]
    UnsupportedSchemaVersion { version: u32 },
    #[error("vault is corrupt: {0}")]
    CorruptVault(String),
    #[error("passphrase cannot be empty")]
    EmptyPassphrase,
    #[error("host label cannot be empty")]
    EmptyHostLabel,
    #[error("host hostname cannot be empty")]
    EmptyHostName,
    #[error("host username cannot be empty")]
    EmptyHostUser,
}

#[derive(Debug, Error)]
pub enum DeviceSecretError {
    #[error("key store backend error: {0}")]
    Backend(String),
    #[error("device secret is missing")]
    MissingSecret,
}

pub trait DeviceSecretStore: Send + Sync {
    fn get_secret(&self, account: &str) -> Result<Option<String>, DeviceSecretError>;
    fn set_secret(&self, account: &str, secret: &str) -> Result<(), DeviceSecretError>;
}

#[derive(Default)]
struct KeyringDeviceSecretStore;

impl DeviceSecretStore for KeyringDeviceSecretStore {
    fn get_secret(&self, account: &str) -> Result<Option<String>, DeviceSecretError> {
        let entry = Entry::new(KEYRING_SERVICE_NAME, account)
            .map_err(|err| DeviceSecretError::Backend(err.to_string()))?;

        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(DeviceSecretError::Backend(err.to_string())),
        }
    }

    fn set_secret(&self, account: &str, secret: &str) -> Result<(), DeviceSecretError> {
        let entry = Entry::new(KEYRING_SERVICE_NAME, account)
            .map_err(|err| DeviceSecretError::Backend(err.to_string()))?;
        entry
            .set_password(secret)
            .map_err(|err| DeviceSecretError::Backend(err.to_string()))
    }
}

pub struct VaultStore {
    vault_path: PathBuf,
    conn: Connection,
    header: Option<VaultHeader>,
    device_store: Arc<dyn DeviceSecretStore>,
    master_key: Option<SecretKey>,
    last_unlock_method: Option<UnlockMethod>,
}

impl VaultStore {
    pub fn open(path: impl AsRef<Path>) -> VaultResult<Self> {
        Self::open_with_device_store(path, Arc::new(KeyringDeviceSecretStore))
    }

    pub fn open_with_device_store(
        path: impl AsRef<Path>,
        device_store: Arc<dyn DeviceSecretStore>,
    ) -> VaultResult<Self> {
        let vault_path = path.as_ref().to_path_buf();
        if let Some(parent) = vault_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&vault_path)?;
        storage::initialize_schema(&conn)?;
        let header = storage::load_header(&conn)?;
        if let Some(header_ref) = header.as_ref() {
            storage::verify_header_integrity(header_ref)?;
        }

        Ok(Self {
            vault_path,
            conn,
            header,
            device_store,
            master_key: None,
            last_unlock_method: None,
        })
    }

    pub fn status(&self) -> VaultStatus {
        let device_unlock_available = self
            .header
            .as_ref()
            .and_then(|_| {
                storage::get_local_state(&self.conn, DEVICE_STATE_KEY)
                    .ok()
                    .flatten()
            })
            .is_some();

        VaultStatus {
            initialized: self.header.is_some(),
            unlocked: self.master_key.is_some(),
            vault_path: self.vault_path.display().to_string(),
            device_unlock_available,
        }
    }

    pub fn last_unlock_method(&self) -> Option<UnlockMethod> {
        self.last_unlock_method
    }

    pub fn create_vault(
        &mut self,
        passphrase: &SecretString,
        device_name: &str,
    ) -> VaultResult<()> {
        if self.header.is_some() {
            return Err(VaultError::VaultAlreadyInitialized);
        }
        if passphrase.expose_secret().trim().is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }

        let master_key = SecretKey::generate();
        let recovery_params = KdfParams::recommended();
        let recovery_wrap_key = recovery_params.derive_wrap_key(passphrase)?;
        let now = now_ts();

        let header = VaultHeader {
            vault_id: Uuid::new_v4().to_string(),
            schema_version: VAULT_SCHEMA_VERSION,
            cipher: "xchacha20poly1305".into(),
            recovery_kdf: recovery_params.clone(),
            created_at: now,
            updated_at: now,
            last_logical_clock: 0,
        };
        storage::insert_header(&self.conn, &header)?;

        let recovery_envelope = encrypt(
            &recovery_wrap_key,
            master_key.as_bytes(),
            recovery_aad(&header.vault_id, &recovery_params).as_bytes(),
        )?;
        let recovery_bundle = RecoveryBundle {
            bundle_id: Uuid::new_v4().to_string(),
            params: recovery_params,
            wrapping_nonce: recovery_envelope.nonce,
            wrapped_master_key: recovery_envelope.ciphertext,
            created_at: now,
            updated_at: now,
        };
        storage::insert_recovery_bundle(&self.conn, &recovery_bundle)?;

        self.header = Some(header);
        self.master_key = Some(master_key);
        self.last_unlock_method = Some(UnlockMethod::Passphrase);
        let _ = self.ensure_device_enrollment(device_name);
        Ok(())
    }

    pub fn try_unlock_with_device(&mut self) -> VaultResult<bool> {
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let Some(device_id) = storage::get_local_state(&self.conn, DEVICE_STATE_KEY)? else {
            return Ok(false);
        };
        let Some(enrollment) = storage::load_device_enrollment(&self.conn, &device_id)? else {
            return Ok(false);
        };
        if enrollment.revoked_at.is_some() {
            return Ok(false);
        }

        let account = device_account_name(&header.vault_id, &device_id);
        let Some(secret) = self.device_store.get_secret(&account)? else {
            return Ok(false);
        };

        let device_key_bytes = Zeroizing::new(
            BASE64
                .decode(secret.as_bytes())
                .map_err(|err| VaultError::CorruptVault(err.to_string()))?,
        );
        let device_key = SecretKey::from_slice(device_key_bytes.as_ref())?;
        let master_key = decrypt(
            &device_key,
            &enrollment.wrapping_nonce,
            &enrollment.wrapped_master_key,
            device_wrap_aad(&header.vault_id, &device_id).as_bytes(),
        )?;
        let master_key = SecretKey::from_slice(master_key.as_ref())?;

        self.master_key = Some(master_key);
        self.last_unlock_method = Some(UnlockMethod::Device);
        storage::update_device_last_used(&self.conn, &device_id, now_ts())?;
        Ok(true)
    }

    pub fn unlock_with_passphrase(
        &mut self,
        passphrase: &SecretString,
        device_name: &str,
    ) -> VaultResult<()> {
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let bundle = storage::load_recovery_bundle(&self.conn)?
            .ok_or_else(|| VaultError::CorruptVault("missing recovery bundle".into()))?;
        let wrap_key = bundle.params.derive_wrap_key(passphrase)?;
        let master_key = decrypt_recovery_master_key(&wrap_key, &header.vault_id, &bundle)?;
        let master_key = SecretKey::from_slice(master_key.as_ref())?;

        self.master_key = Some(master_key);
        self.last_unlock_method = Some(UnlockMethod::Passphrase);
        let _ = self.ensure_device_enrollment(device_name);
        Ok(())
    }

    pub fn rotate_passphrase(
        &mut self,
        old_passphrase: &SecretString,
        new_passphrase: &SecretString,
    ) -> VaultResult<()> {
        if new_passphrase.expose_secret().trim().is_empty() {
            return Err(VaultError::EmptyPassphrase);
        }

        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let bundle = storage::load_recovery_bundle(&self.conn)?
            .ok_or_else(|| VaultError::CorruptVault("missing recovery bundle".into()))?;
        let old_wrap_key = bundle.params.derive_wrap_key(old_passphrase)?;
        let master_key_plaintext =
            decrypt_recovery_master_key(&old_wrap_key, &header.vault_id, &bundle)?;

        let new_params = KdfParams::recommended();
        let new_wrap_key = new_params.derive_wrap_key(new_passphrase)?;
        let wrapped = encrypt(
            &new_wrap_key,
            master_key_plaintext.as_ref(),
            recovery_aad(&header.vault_id, &new_params).as_bytes(),
        )?;
        let now = now_ts();
        let new_bundle = RecoveryBundle {
            bundle_id: Uuid::new_v4().to_string(),
            params: new_params.clone(),
            wrapping_nonce: wrapped.nonce,
            wrapped_master_key: wrapped.ciphertext,
            created_at: now,
            updated_at: now,
        };
        storage::insert_recovery_bundle(&self.conn, &new_bundle)?;
        storage::update_header_after_rotation(&self.conn, now, &new_params)?;

        if let Some(current) = self.header.as_mut() {
            current.recovery_kdf = new_params;
            current.updated_at = now;
        }
        Ok(())
    }

    pub fn lock(&mut self) {
        self.master_key = None;
        self.last_unlock_method = None;
    }

    pub fn store_host(&mut self, mut host: HostConfig) -> VaultResult<HostSummary> {
        self.validate_host(&host)?;
        let master_key = self.master_key()?;
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;

        if host.id.is_empty() {
            host.id = Uuid::new_v4().to_string();
        }

        let logical_clock = storage::bump_logical_clock(&self.conn)?;
        let modified_at = now_ts();
        let record_key = SecretKey::generate();
        let payload = Zeroizing::new(serde_json::to_vec(&host)?);
        let aad = record_aad(
            &header.vault_id,
            &host.id,
            RecordKind::Host,
            RECORD_SCHEMA_VERSION,
        );
        let payload_envelope = encrypt(&record_key, payload.as_ref(), aad.as_bytes())?;
        let wrapped_record_key = encrypt(
            &master_key,
            record_key.as_bytes(),
            record_key_aad(&header.vault_id, &host.id).as_bytes(),
        )?;

        let record = EncryptedRecord {
            record_id: host.id.clone(),
            kind: RecordKind::Host,
            version: RECORD_SCHEMA_VERSION,
            logical_clock,
            modified_at,
            deleted_at: None,
            key_nonce: wrapped_record_key.nonce,
            wrapped_record_key: wrapped_record_key.ciphertext,
            payload_nonce: payload_envelope.nonce,
            payload_ciphertext: payload_envelope.ciphertext,
        };
        storage::upsert_record(&self.conn, &record)?;
        Ok(host.summary(modified_at))
    }

    pub fn list_hosts(&self) -> VaultResult<Vec<HostSummary>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::Host)?;
        let hosts = records
            .into_iter()
            .map(|record| {
                let host = self.decrypt_host_record(&record)?;
                Ok(host.summary(record.modified_at))
            })
            .collect::<VaultResult<Vec<_>>>()?;
        Ok(hosts)
    }

    pub fn load_host(&self, host_id: &str) -> VaultResult<Option<HostConfig>> {
        let Some(record) = storage::load_record(&self.conn, host_id, RecordKind::Host)? else {
            return Ok(None);
        };
        if record.deleted_at.is_some() {
            return Ok(None);
        }
        Ok(Some(self.decrypt_host_record(&record)?))
    }

    pub fn delete_host(&mut self, host_id: &str) -> VaultResult<bool> {
        let Some(mut record) = storage::load_record(&self.conn, host_id, RecordKind::Host)? else {
            return Ok(false);
        };
        if record.deleted_at.is_some() {
            return Ok(false);
        }

        record.logical_clock = storage::bump_logical_clock(&self.conn)?;
        record.modified_at = now_ts();
        record.deleted_at = Some(record.modified_at);
        storage::upsert_record(&self.conn, &record)?;
        Ok(true)
    }

    fn decrypt_host_record(&self, record: &EncryptedRecord) -> VaultResult<HostConfig> {
        let master_key = self.master_key()?;
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let wrapped_record_key = decrypt(
            &master_key,
            &record.key_nonce,
            &record.wrapped_record_key,
            record_key_aad(&header.vault_id, &record.record_id).as_bytes(),
        )?;
        let record_key = SecretKey::from_slice(wrapped_record_key.as_ref())?;
        let payload = decrypt(
            &record_key,
            &record.payload_nonce,
            &record.payload_ciphertext,
            record_aad(
                &header.vault_id,
                &record.record_id,
                record.kind,
                record.version,
            )
            .as_bytes(),
        )?;
        Ok(serde_json::from_slice(payload.as_ref())?)
    }

    fn validate_host(&self, host: &HostConfig) -> VaultResult<()> {
        if host.label.trim().is_empty() {
            return Err(VaultError::EmptyHostLabel);
        }
        if host.hostname.trim().is_empty() {
            return Err(VaultError::EmptyHostName);
        }
        if host.username.trim().is_empty() {
            return Err(VaultError::EmptyHostUser);
        }
        Ok(())
    }

    fn master_key(&self) -> VaultResult<SecretKey> {
        self.master_key.clone().ok_or(VaultError::VaultLocked)
    }

    fn ensure_device_enrollment(&mut self, device_name: &str) -> VaultResult<()> {
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let master_key = self.master_key()?;
        let device_id = match storage::get_local_state(&self.conn, DEVICE_STATE_KEY)? {
            Some(existing) => existing,
            None => {
                let created = Uuid::new_v4().to_string();
                storage::set_local_state(&self.conn, DEVICE_STATE_KEY, &created)?;
                created
            }
        };

        let account = device_account_name(&header.vault_id, &device_id);
        let device_key = match self.device_store.get_secret(&account)? {
            Some(secret) => {
                let bytes = Zeroizing::new(
                    BASE64
                        .decode(secret.as_bytes())
                        .map_err(|err| VaultError::CorruptVault(err.to_string()))?,
                );
                SecretKey::from_slice(bytes.as_ref())?
            }
            None => {
                let new_key = SecretKey::generate();
                self.device_store
                    .set_secret(&account, &BASE64.encode(new_key.as_bytes()))?;
                new_key
            }
        };

        let envelope = encrypt(
            &device_key,
            master_key.as_bytes(),
            device_wrap_aad(&header.vault_id, &device_id).as_bytes(),
        )?;
        let now = now_ts();
        let enrollment = DeviceEnrollment {
            device_id: device_id.clone(),
            device_name: device_name.into(),
            wrapping_nonce: envelope.nonce,
            wrapped_master_key: envelope.ciphertext,
            created_at: now,
            last_used_at: now,
            revoked_at: None,
        };
        storage::upsert_device_enrollment(&self.conn, &enrollment)?;
        Ok(())
    }
}

fn recovery_aad(vault_id: &str, params: &KdfParams) -> String {
    format!(
        "seance:v1:recovery:{vault_id}:m={}:t={}:p={}:salt={}",
        params.memory_kib,
        params.iterations,
        params.parallelism,
        BASE64.encode(&params.salt)
    )
}

fn legacy_recovery_aad(vault_id: &str) -> String {
    format!("seance:v1:recovery:{vault_id}")
}

fn decrypt_recovery_master_key(
    wrap_key: &SecretKey,
    vault_id: &str,
    bundle: &RecoveryBundle,
) -> VaultResult<Zeroizing<Vec<u8>>> {
    decrypt(
        wrap_key,
        &bundle.wrapping_nonce,
        &bundle.wrapped_master_key,
        recovery_aad(vault_id, &bundle.params).as_bytes(),
    )
    .or_else(|_| {
        decrypt(
            wrap_key,
            &bundle.wrapping_nonce,
            &bundle.wrapped_master_key,
            legacy_recovery_aad(vault_id).as_bytes(),
        )
    })
}

fn device_wrap_aad(vault_id: &str, device_id: &str) -> String {
    format!("seance:v1:device:{vault_id}:{device_id}")
}

fn record_key_aad(vault_id: &str, record_id: &str) -> String {
    format!("seance:v1:record-key:{vault_id}:{record_id}")
}

fn record_aad(vault_id: &str, record_id: &str, kind: RecordKind, version: u32) -> String {
    format!(
        "seance:v1:record:{vault_id}:{record_id}:{}:{version}",
        kind.as_str()
    )
}

fn device_account_name(vault_id: &str, device_id: &str) -> String {
    format!("{vault_id}:{device_id}")
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};
    use tempfile::tempdir;

    #[derive(Default)]
    struct MemoryDeviceSecretStore {
        secrets: Mutex<HashMap<String, String>>,
    }

    impl DeviceSecretStore for MemoryDeviceSecretStore {
        fn get_secret(&self, account: &str) -> Result<Option<String>, DeviceSecretError> {
            Ok(self.secrets.lock().unwrap().get(account).cloned())
        }

        fn set_secret(&self, account: &str, secret: &str) -> Result<(), DeviceSecretError> {
            self.secrets
                .lock()
                .unwrap()
                .insert(account.into(), secret.into());
            Ok(())
        }
    }

    fn make_vault() -> (tempfile::TempDir, VaultStore) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.sqlite");
        let vault =
            VaultStore::open_with_device_store(path, Arc::new(MemoryDeviceSecretStore::default()))
                .unwrap();
        (dir, vault)
    }

    fn secret(value: &str) -> SecretString {
        SecretString::from(value.to_owned())
    }

    #[test]
    fn creates_unlocks_and_stores_hosts() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let summary = vault
            .store_host(HostConfig {
                id: String::new(),
                label: "Production".into(),
                hostname: "prod.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("main cluster".into()),
            })
            .unwrap();

        assert_eq!(summary.label, "Production");
        assert_eq!(vault.list_hosts().unwrap().len(), 1);

        vault.lock();
        assert!(vault.try_unlock_with_device().unwrap());

        let host = vault.load_host(&summary.id).unwrap().unwrap();
        assert_eq!(host.hostname, "prod.example.com");
    }

    #[test]
    fn rotates_passphrase_without_losing_data() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("old passphrase"), "test-device")
            .unwrap();
        let host = vault
            .store_host(HostConfig {
                id: String::new(),
                label: "Staging".into(),
                hostname: "staging.example.com".into(),
                port: 22,
                username: "deployer".into(),
                notes: None,
            })
            .unwrap();

        vault
            .rotate_passphrase(&secret("old passphrase"), &secret("new passphrase"))
            .unwrap();
        vault.lock();
        vault
            .unlock_with_passphrase(&secret("new passphrase"), "test-device")
            .unwrap();

        let restored = vault.load_host(&host.id).unwrap().unwrap();
        assert_eq!(restored.username, "deployer");
    }

    #[test]
    fn unlocks_legacy_recovery_bundle_aad() {
        let (_dir, mut vault) = make_vault();
        let passphrase = secret("legacy passphrase");
        vault.create_vault(&passphrase, "test-device").unwrap();

        let header = storage::load_header(&vault.conn).unwrap().unwrap();
        let bundle = storage::load_recovery_bundle(&vault.conn).unwrap().unwrap();
        let wrap_key = bundle.params.derive_wrap_key(&passphrase).unwrap();
        let master_key = decrypt(
            &wrap_key,
            &bundle.wrapping_nonce,
            &bundle.wrapped_master_key,
            recovery_aad(&header.vault_id, &bundle.params).as_bytes(),
        )
        .unwrap();
        let legacy_wrapped = encrypt(
            &wrap_key,
            master_key.as_ref(),
            legacy_recovery_aad(&header.vault_id).as_bytes(),
        )
        .unwrap();

        storage::insert_recovery_bundle(
            &vault.conn,
            &RecoveryBundle {
                wrapping_nonce: legacy_wrapped.nonce,
                wrapped_master_key: legacy_wrapped.ciphertext,
                ..bundle
            },
        )
        .unwrap();

        vault.lock();
        vault
            .unlock_with_passphrase(&secret("legacy passphrase"), "test-device")
            .unwrap();

        assert!(vault.status().unlocked);
    }

    #[test]
    fn deletes_hosts_using_tombstones() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("test passphrase"), "test-device")
            .unwrap();
        let host = vault
            .store_host(HostConfig {
                id: String::new(),
                label: "Disposable".into(),
                hostname: "tmp.example.com".into(),
                port: 22,
                username: "demo".into(),
                notes: None,
            })
            .unwrap();

        assert!(vault.delete_host(&host.id).unwrap());
        assert!(vault.load_host(&host.id).unwrap().is_none());
        assert!(vault.list_hosts().unwrap().is_empty());
    }
}
