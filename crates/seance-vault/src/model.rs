use serde::{Deserialize, Serialize};

use crate::kdf::KdfParams;

pub const VAULT_SCHEMA_VERSION: u32 = 1;
pub const RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Host,
    PrivateKey,
    Snippet,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::PrivateKey => "private_key",
            Self::Snippet => "snippet",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "host" => Some(Self::Host),
            "private_key" => Some(Self::PrivateKey),
            "snippet" => Some(Self::Snippet),
            _ => None,
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
pub struct HostConfig {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub notes: Option<String>,
}

impl HostConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSummary {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub modified_at: i64,
}

#[derive(Debug, Clone)]
pub struct VaultStatus {
    pub initialized: bool,
    pub unlocked: bool,
    pub vault_path: String,
    pub device_unlock_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockMethod {
    Device,
    Passphrase,
}
