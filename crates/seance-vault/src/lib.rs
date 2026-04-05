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
use rand_core::{Infallible, TryCryptoRng, TryRng};
use rusqlite::Connection;
use secrecy::ExposeSecret;
use ssh_key::{
    Algorithm as SshAlgorithm, LineEnding, PrivateKey as SshPrivateKey, PublicKey,
    private::RsaKeypair,
};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crypto::{SecretKey, decrypt, encrypt};
use kdf::KdfParams;
pub use model::{
    CredentialSummary, DeviceEnrollment, GenerateKeyAlgorithm, GenerateKeyRequest, HostAuthRef,
    HostSummary, ImportKeyRequest, KeySummary, PrivateKeyAlgorithm, PrivateKeySource, RecordKind,
    RecoveryBundle, UnlockMethod, VaultHeader, VaultHostProfile, VaultPasswordCredential,
    VaultPrivateKey, VaultStatus,
};

use model::{EncryptedRecord, RECORD_SCHEMA_VERSION, VAULT_SCHEMA_VERSION};
pub use secrecy::SecretString;

const DEVICE_STATE_KEY: &str = "local_device_id";
const KEYRING_SERVICE_NAME: &str = "com.seance.vault";
const DEFAULT_RSA_BITS: u32 = 4096;

#[derive(Default)]
struct SystemRng;

impl TryRng for SystemRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes).expect("OS random generator unavailable");
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes).expect("OS random generator unavailable");
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(dst).expect("OS random generator unavailable");
        Ok(())
    }
}

impl TryCryptoRng for SystemRng {}

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
    #[error("SSH key error: {0}")]
    SshKey(#[from] ssh_key::Error),
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
    #[error("credential label cannot be empty")]
    EmptyCredentialLabel,
    #[error("credential secret cannot be empty")]
    EmptyCredentialSecret,
    #[error("private key label cannot be empty")]
    EmptyPrivateKeyLabel,
    #[error("private key contents cannot be empty")]
    EmptyPrivateKey,
    #[error("host auth order references missing credential {0}")]
    MissingCredentialReference(String),
    #[error("host auth order references missing private key {0}")]
    MissingPrivateKeyReference(String),
    #[error("unsupported private key algorithm")]
    UnsupportedPrivateKeyAlgorithm,
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

    pub fn store_host_profile(&mut self, mut host: VaultHostProfile) -> VaultResult<HostSummary> {
        self.validate_host(&host)?;
        self.validate_host_auth_refs(&host)?;
        if host.id.is_empty() {
            host.id = Uuid::new_v4().to_string();
        }
        self.store_record(&mut host, RecordKind::Host)?;
        let modified_at = self.record_modified_at(&host.id, RecordKind::Host)?;
        Ok(host.summary(modified_at))
    }

    pub fn list_host_profiles(&self) -> VaultResult<Vec<HostSummary>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::Host)?;
        records
            .into_iter()
            .map(|record| {
                let host: VaultHostProfile = self.decrypt_record(&record)?;
                Ok(host.summary(record.modified_at))
            })
            .collect()
    }

    pub fn load_host_profile(&self, host_id: &str) -> VaultResult<Option<VaultHostProfile>> {
        self.load_record_payload(host_id, RecordKind::Host)
    }

    pub fn delete_host_profile(&mut self, host_id: &str) -> VaultResult<bool> {
        self.delete_record(host_id, RecordKind::Host)
    }

    pub fn store_password_credential(
        &mut self,
        mut credential: VaultPasswordCredential,
    ) -> VaultResult<CredentialSummary> {
        self.validate_password_credential(&credential)?;
        if credential.id.is_empty() {
            credential.id = Uuid::new_v4().to_string();
        }
        self.store_record(&mut credential, RecordKind::PasswordCredential)?;
        let modified_at =
            self.record_modified_at(&credential.id, RecordKind::PasswordCredential)?;
        Ok(credential.summary(modified_at))
    }

    pub fn list_password_credentials(&self) -> VaultResult<Vec<CredentialSummary>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::PasswordCredential)?;
        records
            .into_iter()
            .map(|record| {
                let credential: VaultPasswordCredential = self.decrypt_record(&record)?;
                Ok(credential.summary(record.modified_at))
            })
            .collect()
    }

    pub fn load_password_credential(
        &self,
        id: &str,
    ) -> VaultResult<Option<VaultPasswordCredential>> {
        self.load_record_payload(id, RecordKind::PasswordCredential)
    }

    pub fn delete_password_credential(&mut self, id: &str) -> VaultResult<bool> {
        self.delete_record(id, RecordKind::PasswordCredential)
    }

    pub fn store_private_key(&mut self, mut key: VaultPrivateKey) -> VaultResult<KeySummary> {
        self.validate_private_key(&key)?;
        if key.id.is_empty() {
            key.id = Uuid::new_v4().to_string();
        }
        self.store_record(&mut key, RecordKind::PrivateKey)?;
        let modified_at = self.record_modified_at(&key.id, RecordKind::PrivateKey)?;
        Ok(key.summary(modified_at))
    }

    pub fn list_private_keys(&self) -> VaultResult<Vec<KeySummary>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::PrivateKey)?;
        records
            .into_iter()
            .map(|record| {
                let key: VaultPrivateKey = self.decrypt_record(&record)?;
                Ok(key.summary(record.modified_at))
            })
            .collect()
    }

    pub fn load_private_key(&self, id: &str) -> VaultResult<Option<VaultPrivateKey>> {
        self.load_record_payload(id, RecordKind::PrivateKey)
    }

    pub fn delete_private_key(&mut self, id: &str) -> VaultResult<bool> {
        self.delete_record(id, RecordKind::PrivateKey)
    }

    pub fn generate_private_key(&mut self, request: GenerateKeyRequest) -> VaultResult<KeySummary> {
        if request.label.trim().is_empty() {
            return Err(VaultError::EmptyPrivateKeyLabel);
        }

        let (private_key, algorithm) = match request.algorithm {
            GenerateKeyAlgorithm::Ed25519 => (
                SshPrivateKey::random(&mut SystemRng, SshAlgorithm::Ed25519)?,
                PrivateKeyAlgorithm::Ed25519,
            ),
            GenerateKeyAlgorithm::Rsa { bits } => {
                let bits = if bits == 0 { DEFAULT_RSA_BITS } else { bits };
                let keypair = RsaKeypair::random(&mut SystemRng, bits as usize)?;
                (
                    SshPrivateKey::from(keypair),
                    PrivateKeyAlgorithm::Rsa { bits },
                )
            }
        };

        let key = VaultPrivateKey {
            id: String::new(),
            label: request.label,
            algorithm,
            public_key_openssh: private_key.public_key().to_openssh()?,
            private_key_pem: private_key.to_openssh(LineEnding::LF)?.to_string(),
            encrypted_at_rest: private_key.is_encrypted(),
            source: PrivateKeySource::Generated,
        };
        self.store_private_key(key)
    }

    pub fn import_private_key(&mut self, request: ImportKeyRequest) -> VaultResult<KeySummary> {
        let private_key = SshPrivateKey::from_openssh(&request.private_key_pem)?;
        let algorithm = private_key_algorithm(private_key.public_key())?;
        let key = VaultPrivateKey {
            id: String::new(),
            label: request.label,
            algorithm,
            public_key_openssh: private_key.public_key().to_openssh()?,
            private_key_pem: request.private_key_pem,
            encrypted_at_rest: private_key.is_encrypted(),
            source: PrivateKeySource::Imported,
        };
        self.store_private_key(key)
    }

    fn record_modified_at(&self, id: &str, kind: RecordKind) -> VaultResult<i64> {
        let record = storage::load_record(&self.conn, id, kind)?
            .ok_or_else(|| VaultError::CorruptVault(format!("missing {kind:?} record {id}")))?;
        Ok(record.modified_at)
    }

    fn load_record_payload<T: serde::de::DeserializeOwned>(
        &self,
        record_id: &str,
        kind: RecordKind,
    ) -> VaultResult<Option<T>> {
        let Some(record) = storage::load_record(&self.conn, record_id, kind)? else {
            return Ok(None);
        };
        if record.deleted_at.is_some() {
            return Ok(None);
        }
        Ok(Some(self.decrypt_record(&record)?))
    }

    fn store_record<T: serde::Serialize + RecordIdentity>(
        &mut self,
        value: &mut T,
        kind: RecordKind,
    ) -> VaultResult<()> {
        let master_key = self.master_key()?;
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        if value.record_id().is_empty() {
            value.set_record_id(Uuid::new_v4().to_string());
        }

        let logical_clock = storage::bump_logical_clock(&self.conn)?;
        let modified_at = now_ts();
        let record_key = SecretKey::generate();
        let payload = Zeroizing::new(serde_json::to_vec(value)?);
        let aad = record_aad(
            &header.vault_id,
            value.record_id(),
            kind,
            RECORD_SCHEMA_VERSION,
        );
        let payload_envelope = encrypt(&record_key, payload.as_ref(), aad.as_bytes())?;
        let wrapped_record_key = encrypt(
            &master_key,
            record_key.as_bytes(),
            record_key_aad(&header.vault_id, value.record_id()).as_bytes(),
        )?;

        let record = EncryptedRecord {
            record_id: value.record_id().to_string(),
            kind,
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
        Ok(())
    }

    fn decrypt_record<T: serde::de::DeserializeOwned>(
        &self,
        record: &EncryptedRecord,
    ) -> VaultResult<T> {
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

    fn delete_record(&mut self, record_id: &str, kind: RecordKind) -> VaultResult<bool> {
        let Some(mut record) = storage::load_record(&self.conn, record_id, kind)? else {
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

    fn validate_host(&self, host: &VaultHostProfile) -> VaultResult<()> {
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

    fn validate_host_auth_refs(&self, host: &VaultHostProfile) -> VaultResult<()> {
        for auth in &host.auth_order {
            match auth {
                HostAuthRef::Password { credential_id } => {
                    if self
                        .load_record_payload::<VaultPasswordCredential>(
                            credential_id,
                            RecordKind::PasswordCredential,
                        )?
                        .is_none()
                    {
                        return Err(VaultError::MissingCredentialReference(
                            credential_id.clone(),
                        ));
                    }
                }
                HostAuthRef::PrivateKey {
                    key_id,
                    passphrase_credential_id,
                } => {
                    if self
                        .load_record_payload::<VaultPrivateKey>(key_id, RecordKind::PrivateKey)?
                        .is_none()
                    {
                        return Err(VaultError::MissingPrivateKeyReference(key_id.clone()));
                    }
                    if let Some(passphrase_id) = passphrase_credential_id
                        && self
                            .load_record_payload::<VaultPasswordCredential>(
                                passphrase_id,
                                RecordKind::PasswordCredential,
                            )?
                            .is_none()
                    {
                        return Err(VaultError::MissingCredentialReference(
                            passphrase_id.clone(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_password_credential(
        &self,
        credential: &VaultPasswordCredential,
    ) -> VaultResult<()> {
        if credential.label.trim().is_empty() {
            return Err(VaultError::EmptyCredentialLabel);
        }
        if credential.secret.trim().is_empty() {
            return Err(VaultError::EmptyCredentialSecret);
        }
        Ok(())
    }

    fn validate_private_key(&self, key: &VaultPrivateKey) -> VaultResult<()> {
        if key.label.trim().is_empty() {
            return Err(VaultError::EmptyPrivateKeyLabel);
        }
        if key.private_key_pem.trim().is_empty() {
            return Err(VaultError::EmptyPrivateKey);
        }

        let private_key = SshPrivateKey::from_openssh(&key.private_key_pem)?;
        let parsed_algorithm = private_key_algorithm(private_key.public_key())?;
        if parsed_algorithm != key.algorithm {
            return Err(VaultError::CorruptVault(
                "private key algorithm metadata does not match the encoded key".into(),
            ));
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

trait RecordIdentity {
    fn record_id(&self) -> &str;
    fn set_record_id(&mut self, id: String);
}

impl RecordIdentity for VaultHostProfile {
    fn record_id(&self) -> &str {
        &self.id
    }

    fn set_record_id(&mut self, id: String) {
        self.id = id;
    }
}

impl RecordIdentity for VaultPasswordCredential {
    fn record_id(&self) -> &str {
        &self.id
    }

    fn set_record_id(&mut self, id: String) {
        self.id = id;
    }
}

impl RecordIdentity for VaultPrivateKey {
    fn record_id(&self) -> &str {
        &self.id
    }

    fn set_record_id(&mut self, id: String) {
        self.id = id;
    }
}

fn private_key_algorithm(public_key: &PublicKey) -> VaultResult<PrivateKeyAlgorithm> {
    match public_key.algorithm() {
        SshAlgorithm::Ed25519 => Ok(PrivateKeyAlgorithm::Ed25519),
        SshAlgorithm::Rsa { .. } => {
            let bits = public_key
                .key_data()
                .rsa()
                .map(|rsa| mpint_bit_length(rsa.n().as_positive_bytes().unwrap_or_default()))
                .unwrap_or(DEFAULT_RSA_BITS);
            Ok(PrivateKeyAlgorithm::Rsa { bits })
        }
        _ => Err(VaultError::UnsupportedPrivateKeyAlgorithm),
    }
}

fn mpint_bit_length(bytes: &[u8]) -> u32 {
    let Some(&first) = bytes.first() else {
        return 0;
    };
    ((bytes.len() - 1) as u32 * 8) + (8 - first.leading_zeros())
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
    fn creates_unlocks_and_stores_host_profiles() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let password = vault
            .store_password_credential(VaultPasswordCredential {
                id: String::new(),
                label: "prod password".into(),
                username_hint: Some("root".into()),
                secret: "hunter2".into(),
            })
            .unwrap();

        let summary = vault
            .store_host_profile(VaultHostProfile {
                id: String::new(),
                label: "Production".into(),
                hostname: "prod.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("main cluster".into()),
                auth_order: vec![HostAuthRef::Password {
                    credential_id: password.id.clone(),
                }],
            })
            .unwrap();

        assert_eq!(summary.label, "Production");
        assert_eq!(vault.list_host_profiles().unwrap().len(), 1);

        vault.lock();
        assert!(vault.try_unlock_with_device().unwrap());

        let host = vault.load_host_profile(&summary.id).unwrap().unwrap();
        assert_eq!(host.hostname, "prod.example.com");
    }

    #[test]
    fn rotates_passphrase_without_losing_data() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("old passphrase"), "test-device")
            .unwrap();
        let summary = vault
            .store_password_credential(VaultPasswordCredential {
                id: String::new(),
                label: "demo".into(),
                username_hint: None,
                secret: "password".into(),
            })
            .unwrap();

        vault
            .rotate_passphrase(&secret("old passphrase"), &secret("new passphrase"))
            .unwrap();
        vault.lock();
        vault
            .unlock_with_passphrase(&secret("new passphrase"), "test-device")
            .unwrap();

        let restored = vault
            .load_password_credential(&summary.id)
            .unwrap()
            .unwrap();
        assert_eq!(restored.secret, "password");
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
    fn stores_lists_and_deletes_password_credentials() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("test passphrase"), "test-device")
            .unwrap();

        let credential = vault
            .store_password_credential(VaultPasswordCredential {
                id: String::new(),
                label: "db".into(),
                username_hint: Some("postgres".into()),
                secret: "secret".into(),
            })
            .unwrap();

        assert_eq!(vault.list_password_credentials().unwrap().len(), 1);
        assert!(vault.delete_password_credential(&credential.id).unwrap());
        assert!(
            vault
                .load_password_credential(&credential.id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn imports_and_generates_private_keys() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("test passphrase"), "test-device")
            .unwrap();

        let generated = vault
            .generate_private_key(GenerateKeyRequest {
                label: "generated-ed25519".into(),
                algorithm: GenerateKeyAlgorithm::Ed25519,
            })
            .unwrap();
        assert!(matches!(generated.algorithm, PrivateKeyAlgorithm::Ed25519));

        let generated_rsa = vault
            .generate_private_key(GenerateKeyRequest {
                label: "generated-rsa".into(),
                algorithm: GenerateKeyAlgorithm::Rsa { bits: 4096 },
            })
            .unwrap();
        assert!(matches!(
            generated_rsa.algorithm,
            PrivateKeyAlgorithm::Rsa { bits: 4096 }
        ));

        let pem = vault
            .load_private_key(&generated.id)
            .unwrap()
            .unwrap()
            .private_key_pem;

        let imported = vault
            .import_private_key(ImportKeyRequest {
                label: "imported".into(),
                private_key_pem: pem,
            })
            .unwrap();
        assert_eq!(imported.label, "imported");
    }

    #[test]
    fn validates_host_auth_references() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("test passphrase"), "test-device")
            .unwrap();

        let err = vault
            .store_host_profile(VaultHostProfile {
                id: String::new(),
                label: "Broken".into(),
                hostname: "example.com".into(),
                port: 22,
                username: "root".into(),
                notes: None,
                auth_order: vec![HostAuthRef::Password {
                    credential_id: "missing".into(),
                }],
            })
            .unwrap_err();

        assert!(matches!(err, VaultError::MissingCredentialReference(_)));
    }
}
