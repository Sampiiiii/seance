use serde::{Deserialize, Serialize};

use crate::kdf::KdfParams;

pub const VAULT_SCHEMA_VERSION: u32 = 1;
pub const RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Host,
    PasswordCredential,
    PrivateKey,
    Snippet,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::PasswordCredential => "password_credential",
            Self::PrivateKey => "private_key",
            Self::Snippet => "snippet",
        }
    }
}

impl std::str::FromStr for RecordKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "host" => Ok(Self::Host),
            "password_credential" => Ok(Self::PasswordCredential),
            "private_key" => Ok(Self::PrivateKey),
            "snippet" => Ok(Self::Snippet),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultHeader {
    pub vault_id: String,
    pub schema_version: u32,
    pub cipher: String,
    pub recovery_kdf: KdfParams,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_logical_clock: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryBundle {
    pub bundle_id: String,
    pub params: KdfParams,
    pub wrapping_nonce: Vec<u8>,
    pub wrapped_master_key: Vec<u8>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEnrollment {
    pub device_id: String,
    pub device_name: String,
    pub wrapping_nonce: Vec<u8>,
    pub wrapped_master_key: Vec<u8>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedRecord {
    pub record_id: String,
    pub kind: RecordKind,
    pub version: u32,
    pub logical_clock: u64,
    pub modified_at: i64,
    pub deleted_at: Option<i64>,
    pub key_nonce: Vec<u8>,
    pub wrapped_record_key: Vec<u8>,
    pub payload_nonce: Vec<u8>,
    pub payload_ciphertext: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultHostProfile {
    pub id: String,
    pub label: String,
    pub hostname: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub auth_order: Vec<HostAuthRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostAuthRef {
    Password {
        credential_id: String,
    },
    PrivateKey {
        key_id: String,
        #[serde(default)]
        passphrase_credential_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultPasswordCredential {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub username_hint: Option<String>,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultPrivateKey {
    pub id: String,
    pub label: String,
    pub algorithm: PrivateKeyAlgorithm,
    pub public_key_openssh: String,
    pub private_key_pem: String,
    pub encrypted_at_rest: bool,
    pub source: PrivateKeySource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrivateKeyAlgorithm {
    Ed25519,
    Rsa { bits: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrivateKeySource {
    Imported,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateKeyAlgorithm {
    Ed25519,
    Rsa { bits: u32 },
}

#[derive(Debug, Clone)]
pub struct GenerateKeyRequest {
    pub label: String,
    pub algorithm: GenerateKeyAlgorithm,
}

#[derive(Debug, Clone)]
pub struct ImportKeyRequest {
    pub label: String,
    pub private_key_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSummary {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub modified_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub id: String,
    pub label: String,
    pub username_hint: Option<String>,
    pub modified_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySummary {
    pub id: String,
    pub label: String,
    pub algorithm: PrivateKeyAlgorithm,
    pub encrypted_at_rest: bool,
    pub source: PrivateKeySource,
    pub modified_at: i64,
}

impl VaultHostProfile {
    pub fn summary(&self, modified_at: i64) -> HostSummary {
        HostSummary {
            id: self.id.clone(),
            label: self.label.clone(),
            hostname: self.hostname.clone(),
            port: self.port,
            username: self.username.clone(),
            modified_at,
        }
    }
}

impl VaultPasswordCredential {
    pub fn summary(&self, modified_at: i64) -> CredentialSummary {
        CredentialSummary {
            id: self.id.clone(),
            label: self.label.clone(),
            username_hint: self.username_hint.clone(),
            modified_at,
        }
    }
}

impl VaultPrivateKey {
    pub fn summary(&self, modified_at: i64) -> KeySummary {
        KeySummary {
            id: self.id.clone(),
            label: self.label.clone(),
            algorithm: self.algorithm.clone(),
            encrypted_at_rest: self.encrypted_at_rest,
            source: self.source.clone(),
            modified_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VaultStatus {
    pub initialized: bool,
    pub unlocked: bool,
    pub vault_path: String,
    pub device_unlock_available: bool,
    pub device_unlock_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockMethod {
    Device,
    Passphrase,
}

fn default_ssh_port() -> u16 {
    22
}
