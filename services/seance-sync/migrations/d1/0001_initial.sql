CREATE TABLE IF NOT EXISTS users (
    user_id TEXT PRIMARY KEY,
    primary_email TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    disabled_at INTEGER
);

CREATE TABLE IF NOT EXISTS magic_link_challenges (
    challenge_id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    hashed_token TEXT NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    consumed_at INTEGER,
    created_at INTEGER NOT NULL,
    ip_hash TEXT,
    user_agent_hash TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    refresh_token_hash TEXT NOT NULL UNIQUE,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    FOREIGN KEY (user_id) REFERENCES users(user_id)
);

CREATE TABLE IF NOT EXISTS vaults (
    vault_id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    current_stream_seq INTEGER NOT NULL DEFAULT 0,
    current_logical_clock INTEGER NOT NULL DEFAULT 0,
    latest_snapshot_id TEXT,
    FOREIGN KEY (owner_user_id) REFERENCES users(user_id)
);

CREATE TABLE IF NOT EXISTS vault_memberships (
    vault_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL,
    invited_at INTEGER NOT NULL,
    accepted_at INTEGER,
    revoked_at INTEGER,
    PRIMARY KEY (vault_id, user_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(vault_id),
    FOREIGN KEY (user_id) REFERENCES users(user_id)
);

CREATE TABLE IF NOT EXISTS vault_replicas (
    vault_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    device_name_hint TEXT,
    last_seen_stream_seq INTEGER NOT NULL DEFAULT 0,
    last_seen_logical_clock INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER,
    PRIMARY KEY (vault_id, device_id),
    FOREIGN KEY (vault_id) REFERENCES vaults(vault_id)
);

CREATE TABLE IF NOT EXISTS vault_snapshot_manifests (
    snapshot_id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    stream_seq_at_snapshot INTEGER NOT NULL,
    logical_clock_at_snapshot INTEGER NOT NULL,
    r2_object_key TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    compressed_size INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (vault_id) REFERENCES vaults(vault_id)
);

CREATE TABLE IF NOT EXISTS vault_commits (
    vault_id TEXT NOT NULL,
    stream_seq INTEGER NOT NULL,
    commit_id TEXT NOT NULL UNIQUE,
    author_user_id TEXT NOT NULL,
    author_device_id TEXT NOT NULL,
    base_logical_clock INTEGER NOT NULL,
    result_logical_clock INTEGER NOT NULL,
    record_count INTEGER NOT NULL,
    r2_object_key TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, stream_seq),
    FOREIGN KEY (vault_id) REFERENCES vaults(vault_id),
    FOREIGN KEY (author_user_id) REFERENCES users(user_id)
);

CREATE TABLE IF NOT EXISTS vault_idempotency_keys (
    vault_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    stream_seq INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, device_id, idempotency_key),
    FOREIGN KEY (vault_id) REFERENCES vaults(vault_id)
);

CREATE TABLE IF NOT EXISTS multiplayer_rooms (
    room_id TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    creator_user_id TEXT NOT NULL,
    room_name TEXT NOT NULL,
    room_kind TEXT NOT NULL,
    source_host_id TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (vault_id) REFERENCES vaults(vault_id),
    FOREIGN KEY (creator_user_id) REFERENCES users(user_id)
);

CREATE TABLE IF NOT EXISTS audit_log (
    audit_id TEXT PRIMARY KEY,
    user_id TEXT,
    vault_id TEXT,
    action TEXT NOT NULL,
    target_id TEXT,
    occurred_at INTEGER NOT NULL,
    ip_hash TEXT,
    metadata_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_magic_link_email ON magic_link_challenges(email, created_at);
CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_vault_memberships_user ON vault_memberships(user_id, revoked_at);
CREATE INDEX IF NOT EXISTS idx_vault_commits_after_seq ON vault_commits(vault_id, stream_seq);
CREATE INDEX IF NOT EXISTS idx_vault_snapshot_recent ON vault_snapshot_manifests(vault_id, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_log_vault_time ON audit_log(vault_id, occurred_at);

