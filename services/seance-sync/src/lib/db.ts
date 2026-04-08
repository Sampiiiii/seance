import { HttpError } from "./http";

export type AuthenticatedUser = {
  userId: string;
  primaryEmail: string;
};

export type VaultSummaryRow = {
  vault_id: string;
  display_name: string;
  role: string;
  created_at: number;
  updated_at: number;
  deleted_at: number | null;
  current_stream_seq: number;
  current_logical_clock: number;
  latest_snapshot_id: string | null;
};

export type VaultCommitRow = {
  vault_id: string;
  stream_seq: number;
  commit_id: string;
  author_user_id: string;
  author_device_id: string;
  base_logical_clock: number;
  result_logical_clock: number;
  record_count: number;
  r2_object_key: string;
  payload_sha256: string;
  created_at: number;
};

export type SnapshotManifestRow = {
  snapshot_id: string;
  vault_id: string;
  stream_seq_at_snapshot: number;
  logical_clock_at_snapshot: number;
  r2_object_key: string;
  payload_sha256: string;
  compressed_size: number;
  created_at: number;
};

type UserRow = {
  user_id: string;
  primary_email: string;
  created_at: number;
  updated_at: number;
  disabled_at: number | null;
};

type SessionRow = {
  session_id: string;
  user_id: string;
  refresh_token_hash: string;
  issued_at: number;
  expires_at: number;
  revoked_at: number | null;
  primary_email: string;
  disabled_at: number | null;
};

type MagicLinkRow = {
  challenge_id: string;
  email: string;
  expires_at: number;
  consumed_at: number | null;
};

export async function insertMagicLinkChallenge(
  db: D1Database,
  params: {
    challengeId: string;
    email: string;
    hashedToken: string;
    expiresAt: number;
    createdAt: number;
    ipHash: string | null;
    userAgentHash: string | null;
  },
): Promise<void> {
  await db
    .prepare(
      `INSERT INTO magic_link_challenges (
         challenge_id, email, hashed_token, expires_at, consumed_at, created_at, ip_hash, user_agent_hash
       ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7)`,
    )
    .bind(
      params.challengeId,
      params.email,
      params.hashedToken,
      params.expiresAt,
      params.createdAt,
      params.ipHash,
      params.userAgentHash,
    )
    .run();
}

export async function consumeMagicLinkChallenge(
  db: D1Database,
  hashedToken: string,
  now: number,
): Promise<MagicLinkRow> {
  const row = await db
    .prepare(
      `SELECT challenge_id, email, expires_at, consumed_at
         FROM magic_link_challenges
        WHERE hashed_token = ?1`,
    )
    .bind(hashedToken)
    .first<MagicLinkRow>();

  if (!row) {
    throw new HttpError(401, "Magic link is invalid.");
  }
  if (row.consumed_at !== null) {
    throw new HttpError(401, "Magic link has already been used.");
  }
  if (row.expires_at < now) {
    throw new HttpError(401, "Magic link has expired.");
  }

  await db
    .prepare("UPDATE magic_link_challenges SET consumed_at = ?1 WHERE challenge_id = ?2")
    .bind(now, row.challenge_id)
    .run();

  return row;
}

export async function getOrCreateUser(
  db: D1Database,
  email: string,
  now: number,
): Promise<AuthenticatedUser> {
  const existing = await db
    .prepare(
      `SELECT user_id, primary_email, created_at, updated_at, disabled_at
         FROM users
        WHERE primary_email = ?1`,
    )
    .bind(email)
    .first<UserRow>();

  if (existing) {
    if (existing.disabled_at !== null) {
      throw new HttpError(403, "User is disabled.");
    }
    return { userId: existing.user_id, primaryEmail: existing.primary_email };
  }

  const userId = crypto.randomUUID();
  await db
    .prepare(
      `INSERT INTO users (user_id, primary_email, created_at, updated_at, disabled_at)
       VALUES (?1, ?2, ?3, ?4, NULL)`,
    )
    .bind(userId, email, now, now)
    .run();

  return { userId, primaryEmail: email };
}

export async function insertSession(
  db: D1Database,
  params: {
    sessionId: string;
    userId: string;
    refreshTokenHash: string;
    issuedAt: number;
    expiresAt: number;
  },
): Promise<void> {
  await db
    .prepare(
      `INSERT INTO sessions (session_id, user_id, refresh_token_hash, issued_at, expires_at, revoked_at)
       VALUES (?1, ?2, ?3, ?4, ?5, NULL)`,
    )
    .bind(
      params.sessionId,
      params.userId,
      params.refreshTokenHash,
      params.issuedAt,
      params.expiresAt,
    )
    .run();
}

export async function rotateRefreshSession(
  db: D1Database,
  currentHash: string,
  replacementHash: string,
  now: number,
  expiresAt: number,
): Promise<AuthenticatedUser> {
  const row = await db
    .prepare(
      `SELECT s.session_id, s.user_id, s.refresh_token_hash, s.issued_at, s.expires_at, s.revoked_at,
              u.primary_email, u.disabled_at
         FROM sessions s
         JOIN users u ON u.user_id = s.user_id
        WHERE s.refresh_token_hash = ?1`,
    )
    .bind(currentHash)
    .first<SessionRow>();

  if (!row || row.revoked_at !== null || row.expires_at < now || row.disabled_at !== null) {
    throw new HttpError(401, "Refresh token is invalid.");
  }

  await db
    .prepare(
      `UPDATE sessions
          SET refresh_token_hash = ?1,
              issued_at = ?2,
              expires_at = ?3
        WHERE session_id = ?4`,
    )
    .bind(replacementHash, now, expiresAt, row.session_id)
    .run();

  return {
    userId: row.user_id,
    primaryEmail: row.primary_email,
  };
}

export async function revokeSession(
  db: D1Database,
  refreshTokenHash: string,
  now: number,
): Promise<void> {
  await db
    .prepare("UPDATE sessions SET revoked_at = ?1 WHERE refresh_token_hash = ?2")
    .bind(now, refreshTokenHash)
    .run();
}

export async function assertUserActive(
  db: D1Database,
  userId: string,
): Promise<AuthenticatedUser> {
  const row = await db
    .prepare(
      `SELECT user_id, primary_email, created_at, updated_at, disabled_at
         FROM users
        WHERE user_id = ?1`,
    )
    .bind(userId)
    .first<UserRow>();

  if (!row || row.disabled_at !== null) {
    throw new HttpError(401, "User is not authorized.");
  }

  return { userId: row.user_id, primaryEmail: row.primary_email };
}

export async function listVaultsForUser(
  db: D1Database,
  userId: string,
): Promise<VaultSummaryRow[]> {
  const { results } = await db
    .prepare(
      `SELECT v.vault_id, v.display_name, m.role, v.created_at, v.updated_at, v.deleted_at,
              v.current_stream_seq, v.current_logical_clock, v.latest_snapshot_id
         FROM vaults v
         JOIN vault_memberships m ON m.vault_id = v.vault_id
        WHERE m.user_id = ?1
          AND m.revoked_at IS NULL
        ORDER BY v.created_at ASC`,
    )
    .bind(userId)
    .all<VaultSummaryRow>();
  return results ?? [];
}

export async function getVaultForUser(
  db: D1Database,
  vaultId: string,
  userId: string,
): Promise<VaultSummaryRow> {
  const row = await db
    .prepare(
      `SELECT v.vault_id, v.display_name, m.role, v.created_at, v.updated_at, v.deleted_at,
              v.current_stream_seq, v.current_logical_clock, v.latest_snapshot_id
         FROM vaults v
         JOIN vault_memberships m ON m.vault_id = v.vault_id
        WHERE v.vault_id = ?1
          AND m.user_id = ?2
          AND m.revoked_at IS NULL`,
    )
    .bind(vaultId, userId)
    .first<VaultSummaryRow>();

  if (!row || row.deleted_at !== null) {
    throw new HttpError(404, "Vault was not found.");
  }
  return row;
}

export async function createVaultForOwner(
  db: D1Database,
  params: {
    vaultId: string;
    ownerUserId: string;
    displayName: string;
    snapshotId: string;
    logicalClock: number;
    now: number;
  },
): Promise<VaultSummaryRow> {
  await db
    .prepare(
      `INSERT INTO vaults (
         vault_id, owner_user_id, display_name, created_at, updated_at, deleted_at,
         current_stream_seq, current_logical_clock, latest_snapshot_id
       ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, ?6, ?7)`,
    )
    .bind(
      params.vaultId,
      params.ownerUserId,
      params.displayName,
      params.now,
      params.now,
      params.logicalClock,
      params.snapshotId,
    )
    .run();

  await db
    .prepare(
      `INSERT INTO vault_memberships (vault_id, user_id, role, invited_at, accepted_at, revoked_at)
       VALUES (?1, ?2, 'owner', ?3, ?3, NULL)`,
    )
    .bind(params.vaultId, params.ownerUserId, params.now)
    .run();

  return getVaultForUser(db, params.vaultId, params.ownerUserId);
}

export async function insertSnapshotManifest(
  db: D1Database,
  manifest: SnapshotManifestRow,
): Promise<void> {
  await db
    .prepare(
      `INSERT INTO vault_snapshot_manifests (
         snapshot_id, vault_id, stream_seq_at_snapshot, logical_clock_at_snapshot,
         r2_object_key, payload_sha256, compressed_size, created_at
       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)`,
    )
    .bind(
      manifest.snapshot_id,
      manifest.vault_id,
      manifest.stream_seq_at_snapshot,
      manifest.logical_clock_at_snapshot,
      manifest.r2_object_key,
      manifest.payload_sha256,
      manifest.compressed_size,
      manifest.created_at,
    )
    .run();
}

export async function setLatestSnapshot(
  db: D1Database,
  params: {
    vaultId: string;
    snapshotId: string;
    logicalClock: number;
    now: number;
  },
): Promise<void> {
  await db
    .prepare(
      `UPDATE vaults
          SET latest_snapshot_id = ?1,
              current_logical_clock = MAX(current_logical_clock, ?2),
              updated_at = ?3
        WHERE vault_id = ?4`,
    )
    .bind(params.snapshotId, params.logicalClock, params.now, params.vaultId)
    .run();
}

export async function getLatestSnapshotManifest(
  db: D1Database,
  vaultId: string,
): Promise<SnapshotManifestRow> {
  const row = await db
    .prepare(
      `SELECT snapshot_id, vault_id, stream_seq_at_snapshot, logical_clock_at_snapshot,
              r2_object_key, payload_sha256, compressed_size, created_at
         FROM vault_snapshot_manifests
        WHERE vault_id = ?1
        ORDER BY created_at DESC
        LIMIT 1`,
    )
    .bind(vaultId)
    .first<SnapshotManifestRow>();

  if (!row) {
    throw new HttpError(404, "Vault snapshot was not found.");
  }
  return row;
}

export async function listCommitsAfterSeq(
  db: D1Database,
  vaultId: string,
  afterSeq: number,
  limit: number,
): Promise<VaultCommitRow[]> {
  const { results } = await db
    .prepare(
      `SELECT vault_id, stream_seq, commit_id, author_user_id, author_device_id, base_logical_clock,
              result_logical_clock, record_count, r2_object_key, payload_sha256, created_at
         FROM vault_commits
        WHERE vault_id = ?1
          AND stream_seq > ?2
        ORDER BY stream_seq ASC
        LIMIT ?3`,
    )
    .bind(vaultId, afterSeq, limit)
    .all<VaultCommitRow>();
  return results ?? [];
}

export async function createRoom(
  db: D1Database,
  params: {
    roomId: string;
    vaultId: string;
    creatorUserId: string;
    roomName: string;
    roomKind: string;
    sourceHostId: string | null;
    createdAt: number;
  },
): Promise<void> {
  await db
    .prepare(
      `INSERT INTO multiplayer_rooms (
         room_id, vault_id, creator_user_id, room_name, room_kind, source_host_id, created_at
       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)`,
    )
    .bind(
      params.roomId,
      params.vaultId,
      params.creatorUserId,
      params.roomName,
      params.roomKind,
      params.sourceHostId,
      params.createdAt,
    )
    .run();
}

export async function getRoomForUser(
  db: D1Database,
  roomId: string,
  userId: string,
): Promise<{ room_id: string; vault_id: string; room_kind: string }> {
  const row = await db
    .prepare(
      `SELECT r.room_id, r.vault_id, r.room_kind
         FROM multiplayer_rooms r
         JOIN vault_memberships m ON m.vault_id = r.vault_id
        WHERE r.room_id = ?1
          AND m.user_id = ?2
          AND m.revoked_at IS NULL`,
    )
    .bind(roomId, userId)
    .first<{ room_id: string; vault_id: string; room_kind: string }>();

  if (!row) {
    throw new HttpError(404, "Room was not found.");
  }
  return row;
}

export async function insertAuditRow(
  db: D1Database,
  params: {
    auditId: string;
    userId: string | null;
    vaultId: string | null;
    action: string;
    targetId: string | null;
    occurredAt: number;
    ipHash: string | null;
    metadataJson: string | null;
  },
): Promise<void> {
  await db
    .prepare(
      `INSERT INTO audit_log (
         audit_id, user_id, vault_id, action, target_id, occurred_at, ip_hash, metadata_json
       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)`,
    )
    .bind(
      params.auditId,
      params.userId,
      params.vaultId,
      params.action,
      params.targetId,
      params.occurredAt,
      params.ipHash,
      params.metadataJson,
    )
    .run();
}

