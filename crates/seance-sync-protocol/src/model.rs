use seance_vault::{VaultDelta, VaultSnapshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncUser {
    pub user_id: String,
    pub primary_email: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub disabled_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Owner,
    Admin,
    Editor,
    Viewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultSummary {
    pub vault_id: String,
    pub display_name: String,
    pub role: MembershipRole,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub current_stream_seq: u64,
    pub current_logical_clock: u64,
    pub latest_snapshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationCursor {
    pub vault_id: String,
    pub stream_seq: u64,
    pub logical_clock: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCommitEnvelope {
    pub stream_seq: u64,
    pub commit_id: String,
    pub author_device_id: String,
    pub created_at: i64,
    pub delta: VaultDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapEnvelope {
    pub vault: VaultSummary,
    pub snapshot: VaultSnapshot,
    pub commits_after_snapshot: Vec<ServerCommitEnvelope>,
    pub cursor: ReplicationCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSession {
    pub access_token: String,
    pub refresh_token: String,
    pub user: SyncUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MagicLinkStartRequest {
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MagicLinkStartResponse {
    pub challenge_sent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MagicLinkVerifyRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MagicLinkVerifyResponse {
    pub session: AuthSession,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefreshSessionRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefreshSessionResponse {
    pub session: AuthSession,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogoutRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogoutResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListVaultsResponse {
    pub vaults: Vec<VaultSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateVaultRequest {
    pub vault_id: String,
    pub display_name: String,
    pub bootstrap_snapshot: VaultSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateVaultResponse {
    pub vault: VaultSummary,
    pub cursor: ReplicationCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetVaultResponse {
    pub vault: VaultSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotUploadRequest {
    pub base_stream_seq: u64,
    pub idempotency_key: String,
    pub snapshot: VaultSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotUploadResponse {
    pub snapshot_id: String,
    pub stream_seq_at_snapshot: u64,
    pub logical_clock_at_snapshot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitUploadRequest {
    pub idempotency_key: String,
    pub author_device_id: String,
    pub base_logical_clock: u64,
    pub delta: VaultDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitUploadResponse {
    pub accepted_stream_seq: u64,
    pub replication_cursor: ReplicationCursor,
    pub head_logical_clock: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitsPageResponse {
    pub commits: Vec<ServerCommitEnvelope>,
    pub next_after_seq: u64,
    pub head_stream_seq: u64,
    pub head_logical_clock: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    TerminalSession,
    BroadcastInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateRoomRequest {
    pub vault_id: String,
    pub room_name: String,
    pub kind: RoomKind,
    pub source_host_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateRoomResponse {
    pub room_id: String,
    pub vault_id: String,
    pub kind: RoomKind,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JoinRoomRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JoinRoomResponse {
    pub room_id: String,
    pub participant_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiplayerStreamTokenRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiplayerStreamTokenResponse {
    pub room_id: String,
    pub token: String,
    pub expires_at: i64,
    pub moq_origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventEnvelope {
    CommitAvailable {
        vault_id: String,
        stream_seq: u64,
        logical_clock: u64,
        commit_id: String,
        created_at: i64,
    },
    SnapshotCompacted {
        vault_id: String,
        snapshot_id: String,
        stream_seq_at_snapshot: u64,
    },
    MembershipChanged {
        vault_id: String,
        user_id: String,
        role: MembershipRole,
    },
    VaultDeleted {
        vault_id: String,
        deleted_at: i64,
    },
}
