mod crypto;
mod device_store;
mod kdf;
mod model;
mod storage;

use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
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
use device_store::default_device_secret_store;
pub use device_store::{DeviceSecretError, DeviceSecretStore};
use kdf::KdfParams;
pub use model::{
    ApplyDeltaReport, CredentialSummary, DeviceEnrollment, GenerateKeyAlgorithm,
    GenerateKeyRequest, HostAuthRef, HostAuthSummary, HostSummary, ImportKeyRequest, KeySummary,
    PortForwardMode, PortForwardSummary, PrivateKeyAlgorithm, PrivateKeySource, RecordKind,
    RecoveryBundle, SyncCursor, UnlockMethod, VaultDelta, VaultDeltaRecord, VaultHeader,
    VaultHostProfile, VaultPasswordCredential, VaultPortForwardProfile, VaultPrivateKey,
    VaultSnapshot, VaultStatus,
};

use model::{EncryptedRecord, RECORD_SCHEMA_VERSION, RecordSyncState, VAULT_SCHEMA_VERSION};
pub use secrecy::SecretString;

const DEVICE_STATE_KEY: &str = "local_device_id";
const DEVICE_UNLOCK_MESSAGE_KEY: &str = "device_unlock_message";
const DEVICE_UNLOCK_BUILD_MESSAGE: &str = "Touch ID device unlock is unavailable in this build. Launch a signed Seance.app bundle to enroll and use Touch ID.";
const DEVICE_UNLOCK_ENROLLMENT_FAILED_MESSAGE: &str = "Vault unlocked, but Touch ID enrollment failed. Launch a signed Seance.app bundle and unlock again with your passphrase to re-enroll this device.";
const DEVICE_UNLOCK_REENROLL_MESSAGE: &str = "Touch ID device unlock needs to be re-enrolled. Unlock the vault with your passphrase once to repair it.";
const DEFAULT_RSA_BITS: u32 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateKeyInspection {
    pub algorithm: PrivateKeyAlgorithm,
    pub public_key_openssh: String,
    pub encrypted_at_rest: bool,
}

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

pub fn inspect_private_key_pem(private_key_pem: &str) -> VaultResult<PrivateKeyInspection> {
    let private_key = SshPrivateKey::from_openssh(private_key_pem)?;
    Ok(PrivateKeyInspection {
        algorithm: private_key_algorithm(private_key.public_key())?,
        public_key_openssh: private_key.public_key().to_openssh()?,
        encrypted_at_rest: private_key.is_encrypted(),
    })
}

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
    #[error("port forward label cannot be empty")]
    EmptyPortForwardLabel,
    #[error("port forward host reference cannot be empty")]
    EmptyPortForwardHostReference,
    #[error("port forward listen address cannot be empty")]
    EmptyPortForwardListenAddress,
    #[error("port forward target address cannot be empty")]
    EmptyPortForwardTargetAddress,
    #[error("port forward port values must be in the range 1-65535")]
    InvalidPortForwardPort,
    #[error("host auth order references missing credential {0}")]
    MissingCredentialReference(String),
    #[error("host auth order references missing private key {0}")]
    MissingPrivateKeyReference(String),
    #[error("port forward references missing host {0}")]
    MissingHostReference(String),
    #[error("unsupported private key algorithm")]
    UnsupportedPrivateKeyAlgorithm,
    #[error("credential {credential_id} is still referenced by host {host_id}")]
    CredentialInUse {
        credential_id: String,
        host_id: String,
    },
    #[error("private key {key_id} is still referenced by host {host_id}")]
    PrivateKeyInUse { key_id: String, host_id: String },
    #[error("host {host_id} is still referenced by port forward {port_forward_id}")]
    HostInUseByPortForward {
        host_id: String,
        port_forward_id: String,
    },
    #[error(
        "port forward {label} duplicates existing listen endpoint for host {host_id} ({listen_address}:{listen_port})"
    )]
    DuplicatePortForwardListenEndpoint {
        host_id: String,
        label: String,
        listen_address: String,
        listen_port: u16,
    },
    #[error("a full snapshot must be applied before importing deltas")]
    SyncBootstrapRequired,
    #[error("cannot apply a full snapshot to an initialized vault")]
    SnapshotRequiresUninitializedVault,
    #[error("delta belongs to vault {actual_vault_id}, expected {expected_vault_id}")]
    VaultIdMismatch {
        expected_vault_id: String,
        actual_vault_id: String,
    },
    #[error("delta starts at clock {delta_from_clock}, but local vault is only at {local_clock}")]
    DeltaOutOfOrder {
        local_clock: u64,
        delta_from_clock: u64,
    },
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
        Self::open_with_device_store(path, default_device_secret_store())
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
        let device_unlock_message = storage::get_local_state(&self.conn, DEVICE_UNLOCK_MESSAGE_KEY)
            .ok()
            .flatten();
        let device_id = self.header.as_ref().and_then(|_| {
            storage::get_local_state(&self.conn, DEVICE_STATE_KEY)
                .ok()
                .flatten()
        });
        let device_unlock_available = self.header.is_some()
            && device_unlock_message.is_none()
            && device_id
                .as_deref()
                .and_then(|device_id| {
                    storage::load_device_enrollment(&self.conn, device_id)
                        .ok()
                        .flatten()
                })
                .is_some_and(|enrollment| enrollment.revoked_at.is_none());

        VaultStatus {
            initialized: self.header.is_some(),
            unlocked: self.master_key.is_some(),
            vault_path: self.vault_path.display().to_string(),
            device_unlock_available,
            device_unlock_message,
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
        self.refresh_device_enrollment(device_name);
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
        let secret = match self.device_store.get_secret(&account) {
            Ok(Some(secret)) => secret,
            Ok(None) => return Ok(false),
            Err(DeviceSecretError::UnavailableInThisBuild) => {
                self.persist_device_unlock_message(Some(DEVICE_UNLOCK_BUILD_MESSAGE));
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };

        let Ok(device_key) = SecretKey::from_slice(secret.as_ref()) else {
            self.persist_device_unlock_message(Some(DEVICE_UNLOCK_REENROLL_MESSAGE));
            return Ok(false);
        };
        let Ok(master_key) = decrypt(
            &device_key,
            &enrollment.wrapping_nonce,
            &enrollment.wrapped_master_key,
            device_wrap_aad(&header.vault_id, &device_id).as_bytes(),
        ) else {
            self.persist_device_unlock_message(Some(DEVICE_UNLOCK_REENROLL_MESSAGE));
            return Ok(false);
        };
        let Ok(master_key) = SecretKey::from_slice(master_key.as_ref()) else {
            self.persist_device_unlock_message(Some(DEVICE_UNLOCK_REENROLL_MESSAGE));
            return Ok(false);
        };

        self.master_key = Some(master_key);
        self.last_unlock_method = Some(UnlockMethod::Device);
        self.persist_device_unlock_message(None);
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
        self.refresh_device_enrollment(device_name);
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

    pub fn current_cursor(&self) -> SyncCursor {
        SyncCursor {
            logical_clock: self
                .header
                .as_ref()
                .map(|header| header.last_logical_clock)
                .unwrap_or_default(),
        }
    }

    pub fn export_snapshot(&self) -> VaultResult<VaultSnapshot> {
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let recovery_bundle = storage::load_recovery_bundle(&self.conn)?
            .ok_or_else(|| VaultError::CorruptVault("missing recovery bundle".into()))?;
        let device_enrollments = storage::list_device_enrollments(&self.conn)?;
        let records = storage::list_all_records(&self.conn)?
            .into_iter()
            .map(VaultDeltaRecord::from)
            .collect();

        Ok(VaultSnapshot {
            header,
            recovery_bundle,
            device_enrollments,
            records,
        })
    }

    pub fn export_delta(&self, since: SyncCursor) -> VaultResult<VaultDelta> {
        let header = self.header.clone().ok_or(VaultError::VaultNotInitialized)?;
        let from_clock = since.logical_clock.min(header.last_logical_clock);
        let records = storage::list_records_after_clock(&self.conn, from_clock)?
            .into_iter()
            .map(VaultDeltaRecord::from)
            .collect();

        Ok(VaultDelta {
            vault_id: header.vault_id,
            from_clock,
            to_clock: header.last_logical_clock,
            records,
        })
    }

    pub fn apply_snapshot(&mut self, mut snapshot: VaultSnapshot) -> VaultResult<()> {
        if self.header.is_some() || storage::count_records(&self.conn)? > 0 {
            return Err(VaultError::SnapshotRequiresUninitializedVault);
        }

        storage::verify_header_integrity(&snapshot.header)?;
        let max_record_clock = snapshot
            .records
            .iter()
            .map(|record| record.logical_clock)
            .max()
            .unwrap_or_default();
        if snapshot.header.last_logical_clock < max_record_clock {
            snapshot.header.last_logical_clock = max_record_clock;
        }

        storage::insert_header(&self.conn, &snapshot.header)?;
        storage::insert_recovery_bundle(&self.conn, &snapshot.recovery_bundle)?;
        storage::clear_device_enrollments(&self.conn)?;
        for enrollment in &snapshot.device_enrollments {
            storage::upsert_device_enrollment(&self.conn, enrollment)?;
        }
        for record in snapshot.records {
            storage::upsert_record(&self.conn, &delta_record_to_synced_record(record))?;
        }

        self.header = Some(snapshot.header);
        Ok(())
    }

    pub fn apply_delta(&mut self, delta: VaultDelta) -> VaultResult<ApplyDeltaReport> {
        let header = self
            .header
            .clone()
            .ok_or(VaultError::SyncBootstrapRequired)?;
        if delta.vault_id != header.vault_id {
            return Err(VaultError::VaultIdMismatch {
                expected_vault_id: header.vault_id,
                actual_vault_id: delta.vault_id,
            });
        }

        let local_cursor = self.current_cursor().logical_clock;
        if local_cursor < delta.from_clock {
            return Err(VaultError::DeltaOutOfOrder {
                local_clock: local_cursor,
                delta_from_clock: delta.from_clock,
            });
        }

        let mut applied_records = 0usize;
        let mut skipped_records = 0usize;
        let mut new_cursor = local_cursor.max(delta.to_clock);

        for delta_record in delta.records {
            new_cursor = new_cursor.max(delta_record.logical_clock);
            let incoming = delta_record_to_synced_record(delta_record);
            match storage::load_record(&self.conn, &incoming.record_id, incoming.kind)? {
                Some(existing) if compare_record_precedence(&incoming, &existing).is_le() => {
                    skipped_records += 1;
                }
                _ => {
                    storage::upsert_record(&self.conn, &incoming)?;
                    applied_records += 1;
                }
            }
        }

        storage::set_last_logical_clock(&self.conn, new_cursor)?;
        if let Some(header) = self.header.as_mut() {
            header.last_logical_clock = new_cursor;
            header.updated_at = now_ts();
        }

        Ok(ApplyDeltaReport {
            applied_records,
            skipped_records,
            new_cursor: SyncCursor {
                logical_clock: new_cursor,
            },
        })
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

    /// Returns each host with its auth references in one vault scan.
    pub fn list_host_auth_summaries(&self) -> VaultResult<Vec<HostAuthSummary>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::Host)?;
        records
            .into_iter()
            .map(|record| {
                let host: VaultHostProfile = self.decrypt_record(&record)?;
                Ok(host.auth_summary())
            })
            .collect()
    }

    pub fn load_host_profile(&self, host_id: &str) -> VaultResult<Option<VaultHostProfile>> {
        self.load_record_payload(host_id, RecordKind::Host)
    }

    pub fn delete_host_profile(&mut self, host_id: &str) -> VaultResult<bool> {
        self.ensure_host_not_referenced(host_id)?;
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
        self.ensure_credential_not_referenced(id)?;
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
        self.ensure_private_key_not_referenced(id)?;
        self.delete_record(id, RecordKind::PrivateKey)
    }

    pub fn store_port_forward(
        &mut self,
        mut port_forward: VaultPortForwardProfile,
    ) -> VaultResult<PortForwardSummary> {
        self.validate_port_forward(&port_forward)?;
        if port_forward.id.is_empty() {
            port_forward.id = Uuid::new_v4().to_string();
        }
        self.store_record(&mut port_forward, RecordKind::PortForward)?;
        let modified_at = self.record_modified_at(&port_forward.id, RecordKind::PortForward)?;
        Ok(port_forward.summary(modified_at))
    }

    pub fn list_port_forwards(&self) -> VaultResult<Vec<PortForwardSummary>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::PortForward)?;
        records
            .into_iter()
            .map(|record| {
                let port_forward: VaultPortForwardProfile = self.decrypt_record(&record)?;
                Ok(port_forward.summary(record.modified_at))
            })
            .collect()
    }

    pub fn load_port_forward(&self, id: &str) -> VaultResult<Option<VaultPortForwardProfile>> {
        self.load_record_payload(id, RecordKind::PortForward)
    }

    pub fn delete_port_forward(&mut self, id: &str) -> VaultResult<bool> {
        self.delete_record(id, RecordKind::PortForward)
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
        let previous_sync_clock = storage::load_record(&self.conn, value.record_id(), kind)?
            .and_then(|record| record.last_synced_clock);

        let logical_clock = storage::bump_logical_clock(&self.conn)?;
        let modified_at = now_ts();
        if let Some(header) = self.header.as_mut() {
            header.last_logical_clock = logical_clock;
            header.updated_at = modified_at;
        }
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
            last_synced_clock: previous_sync_clock,
            sync_state: RecordSyncState::Pending,
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
        record.sync_state = RecordSyncState::Pending;
        if let Some(header) = self.header.as_mut() {
            header.last_logical_clock = record.logical_clock;
            header.updated_at = record.modified_at;
        }
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

    fn ensure_credential_not_referenced(&self, credential_id: &str) -> VaultResult<()> {
        for host in self.active_hosts()? {
            if host.auth_order.iter().any(|auth| {
                matches!(
                    auth,
                    HostAuthRef::Password { credential_id: id } if id == credential_id
                ) || matches!(
                    auth,
                    HostAuthRef::PrivateKey {
                        passphrase_credential_id: Some(id),
                        ..
                    } if id == credential_id
                )
            }) {
                return Err(VaultError::CredentialInUse {
                    credential_id: credential_id.to_string(),
                    host_id: host.id,
                });
            }
        }
        Ok(())
    }

    fn ensure_private_key_not_referenced(&self, key_id: &str) -> VaultResult<()> {
        for host in self.active_hosts()? {
            if host.auth_order.iter().any(
                |auth| matches!(auth, HostAuthRef::PrivateKey { key_id: id, .. } if id == key_id),
            ) {
                return Err(VaultError::PrivateKeyInUse {
                    key_id: key_id.to_string(),
                    host_id: host.id,
                });
            }
        }
        Ok(())
    }

    fn ensure_host_not_referenced(&self, host_id: &str) -> VaultResult<()> {
        for port_forward in self.active_port_forwards()? {
            if port_forward.host_id == host_id {
                return Err(VaultError::HostInUseByPortForward {
                    host_id: host_id.to_string(),
                    port_forward_id: port_forward.id,
                });
            }
        }
        Ok(())
    }

    fn active_hosts(&self) -> VaultResult<Vec<VaultHostProfile>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::Host)?;
        records
            .into_iter()
            .map(|record| self.decrypt_record(&record))
            .collect()
    }

    fn active_port_forwards(&self) -> VaultResult<Vec<VaultPortForwardProfile>> {
        let records = storage::list_records_by_kind(&self.conn, RecordKind::PortForward)?;
        records
            .into_iter()
            .map(|record| self.decrypt_record(&record))
            .collect()
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

    fn validate_port_forward(&self, port_forward: &VaultPortForwardProfile) -> VaultResult<()> {
        if port_forward.label.trim().is_empty() {
            return Err(VaultError::EmptyPortForwardLabel);
        }
        if port_forward.host_id.trim().is_empty() {
            return Err(VaultError::EmptyPortForwardHostReference);
        }
        if port_forward.listen_address.trim().is_empty() {
            return Err(VaultError::EmptyPortForwardListenAddress);
        }
        if port_forward.target_address.trim().is_empty() {
            return Err(VaultError::EmptyPortForwardTargetAddress);
        }
        if port_forward.listen_port == 0 || port_forward.target_port == 0 {
            return Err(VaultError::InvalidPortForwardPort);
        }
        if self.load_host_profile(&port_forward.host_id)?.is_none() {
            return Err(VaultError::MissingHostReference(
                port_forward.host_id.clone(),
            ));
        }

        let normalized_listen_address = port_forward.listen_address.trim();
        for existing in self.active_port_forwards()? {
            if existing.id == port_forward.id {
                continue;
            }
            if existing.host_id == port_forward.host_id
                && existing.mode == port_forward.mode
                && existing.listen_port == port_forward.listen_port
                && existing
                    .listen_address
                    .eq_ignore_ascii_case(normalized_listen_address)
            {
                return Err(VaultError::DuplicatePortForwardListenEndpoint {
                    host_id: port_forward.host_id.clone(),
                    label: existing.label,
                    listen_address: existing.listen_address,
                    listen_port: existing.listen_port,
                });
            }
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
        let device_key = SecretKey::generate();
        self.device_store
            .set_secret(&account, device_key.as_bytes())?;

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

    fn refresh_device_enrollment(&mut self, device_name: &str) {
        match self.ensure_device_enrollment(device_name) {
            Ok(()) => self.persist_device_unlock_message(None),
            Err(_) => {
                self.persist_device_unlock_message(Some(DEVICE_UNLOCK_ENROLLMENT_FAILED_MESSAGE))
            }
        }
    }

    fn persist_device_unlock_message(&self, message: Option<&str>) {
        match message {
            Some(message) => {
                let _ = storage::set_local_state(&self.conn, DEVICE_UNLOCK_MESSAGE_KEY, message);
            }
            None => {
                let _ = storage::delete_local_state(&self.conn, DEVICE_UNLOCK_MESSAGE_KEY);
            }
        }
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

impl RecordIdentity for VaultPortForwardProfile {
    fn record_id(&self) -> &str {
        &self.id
    }

    fn set_record_id(&mut self, id: String) {
        self.id = id;
    }
}

fn delta_record_to_synced_record(record: VaultDeltaRecord) -> EncryptedRecord {
    EncryptedRecord {
        record_id: record.record_id,
        kind: record.kind,
        version: record.version,
        logical_clock: record.logical_clock,
        modified_at: record.modified_at,
        deleted_at: record.deleted_at,
        key_nonce: record.key_nonce,
        wrapped_record_key: record.wrapped_record_key,
        payload_nonce: record.payload_nonce,
        payload_ciphertext: record.payload_ciphertext,
        last_synced_clock: Some(record.logical_clock),
        sync_state: RecordSyncState::Synced,
    }
}

fn compare_record_precedence(left: &EncryptedRecord, right: &EncryptedRecord) -> Ordering {
    left.logical_clock
        .cmp(&right.logical_clock)
        .then_with(|| left.modified_at.cmp(&right.modified_at))
        .then_with(|| left.record_id.cmp(&right.record_id))
        .then_with(|| left.deleted_at.cmp(&right.deleted_at))
        .then_with(|| left.key_nonce.cmp(&right.key_nonce))
        .then_with(|| left.wrapped_record_key.cmp(&right.wrapped_record_key))
        .then_with(|| left.payload_nonce.cmp(&right.payload_nonce))
        .then_with(|| left.payload_ciphertext.cmp(&right.payload_ciphertext))
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
        secrets: Mutex<HashMap<String, Vec<u8>>>,
        fail_on_get: Mutex<Option<DeviceSecretError>>,
        fail_on_set: Mutex<Option<DeviceSecretError>>,
    }

    impl MemoryDeviceSecretStore {
        fn secret_for(&self, account: &str) -> Option<Vec<u8>> {
            self.secrets.lock().unwrap().get(account).cloned()
        }

        fn fail_next_get(&self, error: DeviceSecretError) {
            *self.fail_on_get.lock().unwrap() = Some(error);
        }

        fn fail_next_set(&self, error: DeviceSecretError) {
            *self.fail_on_set.lock().unwrap() = Some(error);
        }
    }

    impl DeviceSecretStore for MemoryDeviceSecretStore {
        fn get_secret(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceSecretError> {
            if let Some(error) = self.fail_on_get.lock().unwrap().take() {
                return Err(error);
            }
            Ok(self.secrets.lock().unwrap().get(account).cloned())
        }

        fn set_secret(&self, account: &str, secret: &[u8]) -> Result<(), DeviceSecretError> {
            if let Some(error) = self.fail_on_set.lock().unwrap().take() {
                return Err(error);
            }
            self.secrets
                .lock()
                .unwrap()
                .insert(account.into(), secret.to_vec());
            Ok(())
        }
    }

    fn make_vault() -> (tempfile::TempDir, VaultStore) {
        let (_dir, vault, _) = make_vault_with_store();
        (_dir, vault)
    }

    fn make_vault_with_store() -> (tempfile::TempDir, VaultStore, Arc<MemoryDeviceSecretStore>) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.sqlite");
        let store = Arc::new(MemoryDeviceSecretStore::default());
        let vault = VaultStore::open_with_device_store(path, store.clone()).unwrap();
        (dir, vault, store)
    }

    fn secret(value: &str) -> SecretString {
        SecretString::from(value.to_owned())
    }

    fn make_secondary_vault(dir: &tempfile::TempDir) -> (VaultStore, Arc<MemoryDeviceSecretStore>) {
        let path = dir.path().join("secondary.sqlite");
        let store = Arc::new(MemoryDeviceSecretStore::default());
        let vault = VaultStore::open_with_device_store(path, store.clone()).unwrap();
        (vault, store)
    }

    fn seed_password_and_key(
        vault: &mut VaultStore,
    ) -> (CredentialSummary, KeySummary, HostSummary) {
        let credential = vault
            .store_password_credential(VaultPasswordCredential {
                id: String::new(),
                label: "prod password".into(),
                username_hint: Some("root".into()),
                secret: "hunter2".into(),
            })
            .unwrap();
        let key = vault
            .generate_private_key(GenerateKeyRequest {
                label: "deploy".into(),
                algorithm: GenerateKeyAlgorithm::Ed25519,
            })
            .unwrap();
        let host = vault
            .store_host_profile(VaultHostProfile {
                id: String::new(),
                label: "Production".into(),
                hostname: "prod.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("main cluster".into()),
                auth_order: vec![
                    HostAuthRef::Password {
                        credential_id: credential.id.clone(),
                    },
                    HostAuthRef::PrivateKey {
                        key_id: key.id.clone(),
                        passphrase_credential_id: Some(credential.id.clone()),
                    },
                ],
            })
            .unwrap();

        (credential, key, host)
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

    #[test]
    fn stores_raw_device_secret_bytes_for_device_unlock() {
        let (_dir, mut vault, store) = make_vault_with_store();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let header = storage::load_header(&vault.conn).unwrap().unwrap();
        let device_id = storage::get_local_state(&vault.conn, DEVICE_STATE_KEY)
            .unwrap()
            .unwrap();
        let account = device_account_name(&header.vault_id, &device_id);
        let stored_secret = store.secret_for(&account).unwrap();

        assert_eq!(stored_secret.len(), 32);

        vault.lock();
        assert!(vault.try_unlock_with_device().unwrap());
    }

    #[test]
    fn passphrase_unlock_reenrolls_device_with_fresh_secret() {
        let (_dir, mut vault, store) = make_vault_with_store();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let header = storage::load_header(&vault.conn).unwrap().unwrap();
        let device_id = storage::get_local_state(&vault.conn, DEVICE_STATE_KEY)
            .unwrap()
            .unwrap();
        let account = device_account_name(&header.vault_id, &device_id);
        let original_secret = store.secret_for(&account).unwrap();

        vault.lock();
        vault
            .unlock_with_passphrase(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let refreshed_secret = store.secret_for(&account).unwrap();
        assert_ne!(refreshed_secret, original_secret);
        assert_eq!(vault.status().device_unlock_message, None);

        vault.lock();
        assert!(vault.try_unlock_with_device().unwrap());
    }

    #[test]
    fn status_reports_device_unlock_warning_and_disables_device_unlock() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();
        storage::set_local_state(
            &vault.conn,
            DEVICE_UNLOCK_MESSAGE_KEY,
            DEVICE_UNLOCK_BUILD_MESSAGE,
        )
        .unwrap();

        let status = vault.status();
        assert!(!status.device_unlock_available);
        assert_eq!(
            status.device_unlock_message.as_deref(),
            Some(DEVICE_UNLOCK_BUILD_MESSAGE)
        );
    }

    #[test]
    fn unlock_with_passphrase_records_enrollment_warning_without_failing() {
        let (_dir, mut vault, store) = make_vault_with_store();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();
        vault.lock();
        store.fail_next_set(DeviceSecretError::UnavailableInThisBuild);

        vault
            .unlock_with_passphrase(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let status = vault.status();
        assert!(status.unlocked);
        assert!(!status.device_unlock_available);
        assert_eq!(
            status.device_unlock_message.as_deref(),
            Some(DEVICE_UNLOCK_ENROLLMENT_FAILED_MESSAGE)
        );
    }

    #[test]
    fn successful_passphrase_unlock_clears_previous_enrollment_warning() {
        let (_dir, mut vault, store) = make_vault_with_store();
        store.fail_next_set(DeviceSecretError::UnavailableInThisBuild);
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        assert_eq!(
            vault.status().device_unlock_message.as_deref(),
            Some(DEVICE_UNLOCK_ENROLLMENT_FAILED_MESSAGE)
        );

        vault.lock();
        vault
            .unlock_with_passphrase(&secret("correct horse battery staple"), "test-device")
            .unwrap();

        let status = vault.status();
        assert_eq!(status.device_unlock_message, None);
        assert!(status.device_unlock_available);

        vault.lock();
        assert!(vault.try_unlock_with_device().unwrap());
    }

    #[test]
    fn device_unlock_build_failure_records_warning_and_falls_back() {
        let (_dir, mut vault, store) = make_vault_with_store();
        vault
            .create_vault(&secret("correct horse battery staple"), "test-device")
            .unwrap();
        vault.lock();
        store.fail_next_get(DeviceSecretError::UnavailableInThisBuild);

        assert!(!vault.try_unlock_with_device().unwrap());
        assert_eq!(
            vault.status().device_unlock_message.as_deref(),
            Some(DEVICE_UNLOCK_BUILD_MESSAGE)
        );
    }

    #[test]
    fn snapshot_import_bootstraps_second_device_and_preserves_records() {
        let (dir, mut source) = make_vault();
        source
            .create_vault(&secret("snapshot passphrase"), "source")
            .unwrap();
        let (credential, key, host) = seed_password_and_key(&mut source);

        let snapshot = source.export_snapshot().unwrap();
        let (mut target, _) = make_secondary_vault(&dir);
        target.apply_snapshot(snapshot.clone()).unwrap();

        assert_eq!(target.export_snapshot().unwrap(), snapshot);

        target
            .unlock_with_passphrase(&secret("snapshot passphrase"), "target")
            .unwrap();

        let restored_host = target.load_host_profile(&host.id).unwrap().unwrap();
        assert_eq!(restored_host.hostname, "prod.example.com");
        assert!(
            target
                .load_password_credential(&credential.id)
                .unwrap()
                .is_some()
        );
        assert!(target.load_private_key(&key.id).unwrap().is_some());
        assert!(target.status().device_unlock_available);
    }

    #[test]
    fn delta_import_converges_host_updates_and_tombstones() {
        let (dir, mut source) = make_vault();
        source
            .create_vault(&secret("delta passphrase"), "source")
            .unwrap();
        let (_, _, host) = seed_password_and_key(&mut source);

        let snapshot = source.export_snapshot().unwrap();
        let (mut target, _) = make_secondary_vault(&dir);
        target.apply_snapshot(snapshot).unwrap();
        source
            .store_host_profile(VaultHostProfile {
                id: host.id.clone(),
                label: "Production".into(),
                hostname: "prod-2.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("rotated".into()),
                auth_order: source
                    .load_host_profile(&host.id)
                    .unwrap()
                    .unwrap()
                    .auth_order,
            })
            .unwrap();

        let delta = source
            .export_delta(SyncCursor { logical_clock: 3 })
            .unwrap();
        let report = target.apply_delta(delta).unwrap();
        assert_eq!(report.applied_records, 1);

        target
            .unlock_with_passphrase(&secret("delta passphrase"), "target")
            .unwrap();
        let restored_host = target.load_host_profile(&host.id).unwrap().unwrap();
        assert_eq!(restored_host.hostname, "prod-2.example.com");

        source.delete_host_profile(&host.id).unwrap();
        target.lock();
        let tombstone_delta = source
            .export_delta(SyncCursor {
                logical_clock: report.new_cursor.logical_clock,
            })
            .unwrap();
        target.apply_delta(tombstone_delta).unwrap();
        target
            .unlock_with_passphrase(&secret("delta passphrase"), "target")
            .unwrap();
        assert!(target.load_host_profile(&host.id).unwrap().is_none());
    }

    #[test]
    fn concurrent_equal_clock_edits_converge_after_delta_exchange() {
        let (dir, mut source) = make_vault();
        source
            .create_vault(&secret("conflict passphrase"), "source")
            .unwrap();
        let (_, _, host) = seed_password_and_key(&mut source);
        let base_cursor = source.current_cursor();

        let snapshot = source.export_snapshot().unwrap();
        let (mut target, _) = make_secondary_vault(&dir);
        target.apply_snapshot(snapshot).unwrap();
        source
            .unlock_with_passphrase(&secret("conflict passphrase"), "source")
            .unwrap();
        target
            .unlock_with_passphrase(&secret("conflict passphrase"), "target")
            .unwrap();

        let source_auth = source
            .load_host_profile(&host.id)
            .unwrap()
            .unwrap()
            .auth_order;
        let target_auth = target
            .load_host_profile(&host.id)
            .unwrap()
            .unwrap()
            .auth_order;
        source
            .store_host_profile(VaultHostProfile {
                id: host.id.clone(),
                label: "Production".into(),
                hostname: "source.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("source edit".into()),
                auth_order: source_auth,
            })
            .unwrap();
        target
            .store_host_profile(VaultHostProfile {
                id: host.id.clone(),
                label: "Production".into(),
                hostname: "target.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("target edit".into()),
                auth_order: target_auth,
            })
            .unwrap();

        assert_eq!(
            source.current_cursor().logical_clock,
            target.current_cursor().logical_clock
        );
        source
            .apply_delta(target.export_delta(base_cursor.clone()).unwrap())
            .unwrap();
        target
            .apply_delta(source.export_delta(base_cursor).unwrap())
            .unwrap();

        let source_host = source.load_host_profile(&host.id).unwrap().unwrap();
        let target_host = target.load_host_profile(&host.id).unwrap().unwrap();
        assert_eq!(source_host, target_host);
    }

    #[test]
    fn deleting_referenced_credentials_and_keys_is_rejected() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("refs passphrase"), "source")
            .unwrap();
        let (credential, key, _) = seed_password_and_key(&mut vault);

        let credential_err = vault
            .delete_password_credential(&credential.id)
            .unwrap_err();
        assert!(matches!(
            credential_err,
            VaultError::CredentialInUse { credential_id, .. } if credential_id == credential.id
        ));

        let key_err = vault.delete_private_key(&key.id).unwrap_err();
        assert!(matches!(
            key_err,
            VaultError::PrivateKeyInUse { key_id, .. } if key_id == key.id
        ));
    }

    #[test]
    fn stores_lists_and_loads_port_forwards() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("tunnel passphrase"), "source")
            .unwrap();
        let (_, _, host) = seed_password_and_key(&mut vault);

        let summary = vault
            .store_port_forward(VaultPortForwardProfile {
                id: String::new(),
                host_id: host.id.clone(),
                label: "postgres local".into(),
                mode: PortForwardMode::Local,
                listen_address: "127.0.0.1".into(),
                listen_port: 5433,
                target_address: "127.0.0.1".into(),
                target_port: 5432,
                notes: Some("app tunnel".into()),
            })
            .unwrap();

        let loaded = vault.load_port_forward(&summary.id).unwrap().unwrap();
        assert_eq!(loaded.label, "postgres local");
        assert_eq!(vault.list_port_forwards().unwrap().len(), 1);
    }

    #[test]
    fn duplicate_port_forward_listen_endpoints_are_rejected_per_host_and_mode() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("tunnel passphrase"), "source")
            .unwrap();
        let (_, _, host) = seed_password_and_key(&mut vault);

        vault
            .store_port_forward(VaultPortForwardProfile {
                id: String::new(),
                host_id: host.id.clone(),
                label: "first".into(),
                mode: PortForwardMode::Local,
                listen_address: "127.0.0.1".into(),
                listen_port: 8080,
                target_address: "127.0.0.1".into(),
                target_port: 80,
                notes: None,
            })
            .unwrap();

        let err = vault
            .store_port_forward(VaultPortForwardProfile {
                id: String::new(),
                host_id: host.id.clone(),
                label: "second".into(),
                mode: PortForwardMode::Local,
                listen_address: "127.0.0.1".into(),
                listen_port: 8080,
                target_address: "127.0.0.1".into(),
                target_port: 8081,
                notes: None,
            })
            .unwrap_err();

        assert!(matches!(
            err,
            VaultError::DuplicatePortForwardListenEndpoint {
                host_id,
                listen_port: 8080,
                ..
            } if host_id == host.id
        ));
    }

    #[test]
    fn deleting_host_referenced_by_port_forward_is_rejected() {
        let (_dir, mut vault) = make_vault();
        vault
            .create_vault(&secret("tunnel passphrase"), "source")
            .unwrap();
        let (_, _, host) = seed_password_and_key(&mut vault);
        let port_forward = vault
            .store_port_forward(VaultPortForwardProfile {
                id: String::new(),
                host_id: host.id.clone(),
                label: "reverse web".into(),
                mode: PortForwardMode::Remote,
                listen_address: "127.0.0.1".into(),
                listen_port: 9000,
                target_address: "127.0.0.1".into(),
                target_port: 3000,
                notes: None,
            })
            .unwrap();

        let err = vault.delete_host_profile(&host.id).unwrap_err();
        assert!(matches!(
            err,
            VaultError::HostInUseByPortForward {
                host_id,
                port_forward_id
            } if host_id == host.id && port_forward_id == port_forward.id
        ));
    }

    #[test]
    fn out_of_order_delta_requires_snapshot_or_prior_deltas() {
        let (dir, mut source) = make_vault();
        source
            .create_vault(&secret("ordering passphrase"), "source")
            .unwrap();
        let (_, _, host) = seed_password_and_key(&mut source);

        let snapshot = source.export_snapshot().unwrap();
        let (mut target, _) = make_secondary_vault(&dir);
        target.apply_snapshot(snapshot).unwrap();

        let auth = source
            .load_host_profile(&host.id)
            .unwrap()
            .unwrap()
            .auth_order;
        source
            .store_host_profile(VaultHostProfile {
                id: host.id.clone(),
                label: "Production".into(),
                hostname: "step-one.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("step one".into()),
                auth_order: auth.clone(),
            })
            .unwrap();
        source
            .store_host_profile(VaultHostProfile {
                id: host.id.clone(),
                label: "Production".into(),
                hostname: "step-two.example.com".into(),
                port: 22,
                username: "root".into(),
                notes: Some("step two".into()),
                auth_order: auth,
            })
            .unwrap();

        let err = target
            .apply_delta(
                source
                    .export_delta(SyncCursor { logical_clock: 4 })
                    .unwrap(),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            VaultError::DeltaOutOfOrder {
                local_clock: 3,
                delta_from_clock: 4
            }
        ));
    }
}
