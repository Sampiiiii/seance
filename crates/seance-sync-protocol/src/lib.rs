//! Hosted sync protocol types shared between the desktop app and the sync service.

mod model;

pub use model::{
    AuthSession, BootstrapEnvelope, CommitUploadRequest, CommitUploadResponse, CommitsPageResponse,
    CreateRoomRequest, CreateRoomResponse, CreateVaultRequest, CreateVaultResponse, EventEnvelope,
    GetVaultResponse, JoinRoomRequest, JoinRoomResponse, ListVaultsResponse, LogoutRequest,
    LogoutResponse, MagicLinkStartRequest, MagicLinkStartResponse, MagicLinkVerifyRequest,
    MagicLinkVerifyResponse, MembershipRole, MultiplayerStreamTokenRequest,
    MultiplayerStreamTokenResponse, RefreshSessionRequest, RefreshSessionResponse,
    ReplicationCursor, RoomKind, ServerCommitEnvelope, SnapshotUploadRequest,
    SnapshotUploadResponse, SyncUser, VaultSummary,
};

#[cfg(test)]
mod tests {
    use super::*;
    use seance_vault::{VaultDelta, VaultSnapshot};
    use serde_json::json;

    #[test]
    fn replication_cursor_serializes_roundtrip() {
        let cursor = ReplicationCursor {
            vault_id: "vault-123".into(),
            stream_seq: 42,
            logical_clock: 9,
        };

        let encoded = serde_json::to_string(&cursor).unwrap();
        let decoded: ReplicationCursor = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, cursor);
    }

    #[test]
    fn bootstrap_envelope_accepts_vault_snapshot_payloads() {
        let snapshot: VaultSnapshot = serde_json::from_value(json!({
            "header": {
                "vault_id": "vault-123",
                "schema_version": 1,
                "cipher": "chacha20poly1305",
                "recovery_kdf": {
                    "algorithm": "argon2id",
                    "memory_kib": 65536,
                    "iterations": 3,
                    "parallelism": 1,
                    "salt": []
                },
                "created_at": 1,
                "updated_at": 2,
                "last_logical_clock": 0
            },
            "recovery_bundle": {
                "bundle_id": "bundle-1",
                "params": {
                    "algorithm": "argon2id",
                    "memory_kib": 65536,
                    "iterations": 3,
                    "parallelism": 1,
                    "salt": []
                },
                "wrapping_nonce": [],
                "wrapped_master_key": [],
                "created_at": 1,
                "updated_at": 1
            },
            "device_enrollments": [],
            "records": []
        }))
        .unwrap();

        let envelope = BootstrapEnvelope {
            vault: VaultSummary {
                vault_id: "vault-123".into(),
                display_name: "Personal".into(),
                role: MembershipRole::Owner,
                created_at: 1,
                updated_at: 2,
                deleted_at: None,
                current_stream_seq: 0,
                current_logical_clock: 0,
                latest_snapshot_id: Some("snap-1".into()),
            },
            snapshot,
            commits_after_snapshot: vec![],
            cursor: ReplicationCursor {
                vault_id: "vault-123".into(),
                stream_seq: 0,
                logical_clock: 0,
            },
        };

        let encoded = serde_json::to_string(&envelope).unwrap();
        let decoded: BootstrapEnvelope = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.cursor.stream_seq, 0);
    }

    #[test]
    fn commit_upload_request_embeds_vault_delta() {
        let request = CommitUploadRequest {
            idempotency_key: "idem-1".into(),
            author_device_id: "device-1".into(),
            base_logical_clock: 4,
            delta: VaultDelta {
                vault_id: "vault-123".into(),
                from_clock: 4,
                to_clock: 5,
                records: vec![],
            },
        };

        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: CommitUploadRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.delta.to_clock, 5);
    }
}
