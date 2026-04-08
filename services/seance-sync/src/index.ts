import { RoomAuthority } from "./durable/room-authority";
import { VaultAuthority } from "./durable/vault-authority";
import {
  authenticateRequest,
  finishMagicLinkSignin,
  issueSession,
  revokeRefreshToken,
  rotateSession,
} from "./lib/auth";
import {
  createRoom,
  createVaultForOwner,
  getLatestSnapshotManifest,
  getVaultForUser,
  insertAuditRow,
  insertMagicLinkChallenge,
  insertSnapshotManifest,
  listCommitsAfterSeq,
  listVaultsForUser,
  type VaultCommitRow,
} from "./lib/db";
import { nowTs, randomToken, sha256Hex, signToken } from "./lib/crypto";
import { emptyResponse, jsonResponse, readJson, requireMethod, textResponse, withErrorBoundary, HttpError } from "./lib/http";
import { commitObjectKey, jsonPayloadDigest, snapshotObjectKey } from "./lib/r2";

export { RoomAuthority, VaultAuthority };

type MagicLinkStartRequest = { email: string };
type MagicLinkVerifyRequest = { token: string };
type RefreshSessionRequest = { refresh_token: string };
type LogoutRequest = { refresh_token: string };
type CreateVaultRequest = {
  vault_id: string;
  display_name: string;
  bootstrap_snapshot: {
    header: { vault_id: string; last_logical_clock: number };
  };
};
type SnapshotUploadRequest = {
  base_stream_seq: number;
  idempotency_key: string;
  snapshot: {
    header: { vault_id: string; last_logical_clock: number };
  };
};
type CommitUploadRequest = {
  idempotency_key: string;
  author_device_id: string;
  base_logical_clock: number;
  delta: {
    vault_id: string;
    from_clock: number;
    to_clock: number;
    records: Array<unknown>;
  };
};
type CreateRoomRequest = {
  vault_id: string;
  room_name: string;
  kind: "terminal_session" | "broadcast_input";
  source_host_id?: string | null;
};

function baseUrl(env: Env): string {
  return env.APP_BASE_URL;
}

async function audit(
  env: Env,
  params: {
    userId?: string | null;
    vaultId?: string | null;
    action: string;
    targetId?: string | null;
    request: Request;
    metadata?: Record<string, unknown>;
  },
): Promise<void> {
  const ipHash = await sha256Hex(
    params.request.headers.get("cf-connecting-ip") ??
      params.request.headers.get("x-forwarded-for") ??
      "unknown",
  );
  await insertAuditRow(env.DB, {
    auditId: crypto.randomUUID(),
    userId: params.userId ?? null,
    vaultId: params.vaultId ?? null,
    action: params.action,
    targetId: params.targetId ?? null,
    occurredAt: nowTs(),
    ipHash,
    metadataJson: params.metadata ? JSON.stringify(params.metadata) : null,
  });
}

function summaryFromRow(row: Awaited<ReturnType<typeof getVaultForUser>>): Record<string, unknown> {
  return {
    vault_id: row.vault_id,
    display_name: row.display_name,
    role: row.role,
    created_at: row.created_at,
    updated_at: row.updated_at,
    deleted_at: row.deleted_at,
    current_stream_seq: row.current_stream_seq,
    current_logical_clock: row.current_logical_clock,
    latest_snapshot_id: row.latest_snapshot_id,
  };
}

async function loadCommitEnvelope(
  env: Env,
  row: VaultCommitRow,
): Promise<Record<string, unknown>> {
  const object = await env.SYNC_BLOBS.get(row.r2_object_key);
  if (!object) {
    throw new HttpError(500, `Commit payload ${row.commit_id} is missing from R2.`);
  }
  const delta = await object.json<unknown>();
  return {
    stream_seq: row.stream_seq,
    commit_id: row.commit_id,
    author_device_id: row.author_device_id,
    created_at: row.created_at,
    delta,
  };
}

async function handleFetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname;

  if (path === "/healthz") {
    return textResponse("ok");
  }

  if (path === "/v1/auth/magic-link/start") {
    requireMethod(request, "POST");
    const body = await readJson<MagicLinkStartRequest>(request);
    const email = body.email.trim().toLowerCase();
    if (!email.includes("@")) {
      throw new HttpError(400, "A valid email address is required.");
    }

    const token = randomToken(32);
    const now = nowTs();
    const ttl = Number.parseInt(env.MAGIC_LINK_TTL_SECONDS, 10);
    await insertMagicLinkChallenge(env.DB, {
      challengeId: crypto.randomUUID(),
      email,
      hashedToken: await sha256Hex(token),
      expiresAt: now + ttl,
      createdAt: now,
      ipHash: await sha256Hex(request.headers.get("cf-connecting-ip") ?? "unknown"),
      userAgentHash: await sha256Hex(request.headers.get("user-agent") ?? "unknown"),
    });

    ctx.waitUntil(
      env.SYNC_EMAIL_OUTBOX.send({
        type: "magic_link",
        email,
        verify_url: `${baseUrl(env)}/auth/verify?token=${token}`,
        token,
        created_at: now,
      }),
    );
    ctx.waitUntil(audit(env, { action: "magic_link_started", request, metadata: { email } }));
    return jsonResponse({ challenge_sent: true }, 202);
  }

  if (path === "/v1/auth/magic-link/verify") {
    requireMethod(request, "POST");
    const body = await readJson<MagicLinkVerifyRequest>(request);
    const hashedToken = await sha256Hex(body.token.trim());
    const challenge = await (await import("./lib/db")).consumeMagicLinkChallenge(
      env.DB,
      hashedToken,
      nowTs(),
    );
    const session = await finishMagicLinkSignin(env, challenge.email);
    ctx.waitUntil(
      audit(env, {
        userId: session.user.userId,
        action: "magic_link_verified",
        request,
        metadata: { email: challenge.email },
      }),
    );
    return jsonResponse({
      session: {
        access_token: session.accessToken,
        refresh_token: session.refreshToken,
        user: {
          user_id: session.user.userId,
          primary_email: session.user.primaryEmail,
          created_at: nowTs(),
          updated_at: nowTs(),
          disabled_at: null,
        },
      },
    });
  }

  if (path === "/v1/auth/refresh") {
    requireMethod(request, "POST");
    const body = await readJson<RefreshSessionRequest>(request);
    const session = await rotateSession(env, body.refresh_token);
    return jsonResponse({
      session: {
        access_token: session.accessToken,
        refresh_token: session.refreshToken,
        user: {
          user_id: session.user.userId,
          primary_email: session.user.primaryEmail,
          created_at: nowTs(),
          updated_at: nowTs(),
          disabled_at: null,
        },
      },
    });
  }

  if (path === "/v1/auth/logout") {
    requireMethod(request, "POST");
    const body = await readJson<LogoutRequest>(request);
    await revokeRefreshToken(env, body.refresh_token);
    return jsonResponse({ ok: true });
  }

  const user = await authenticateRequest(request, env);

  if (path === "/v1/vaults" && request.method === "GET") {
    const vaults = await listVaultsForUser(env.DB, user.userId);
    return jsonResponse({ vaults: vaults.map(summaryFromRow) });
  }

  if (path === "/v1/vaults" && request.method === "POST") {
    const body = await readJson<CreateVaultRequest>(request);
    if (body.vault_id !== body.bootstrap_snapshot.header.vault_id) {
      throw new HttpError(400, "Bootstrap snapshot vault_id must match the request vault_id.");
    }

    const now = nowTs();
    const snapshotId = crypto.randomUUID();
    const objectKey = snapshotObjectKey(body.vault_id, snapshotId);
    const payloadSha256 = await jsonPayloadDigest(body.bootstrap_snapshot);
    const payload = JSON.stringify(body.bootstrap_snapshot);
    await env.SYNC_BLOBS.put(objectKey, payload, {
      httpMetadata: { contentType: "application/json" },
    });

    const vault = await createVaultForOwner(env.DB, {
      vaultId: body.vault_id,
      ownerUserId: user.userId,
      displayName: body.display_name,
      snapshotId,
      logicalClock: body.bootstrap_snapshot.header.last_logical_clock,
      now,
    });

    await insertSnapshotManifest(env.DB, {
      snapshot_id: snapshotId,
      vault_id: body.vault_id,
      stream_seq_at_snapshot: 0,
      logical_clock_at_snapshot: body.bootstrap_snapshot.header.last_logical_clock,
      r2_object_key: objectKey,
      payload_sha256: payloadSha256,
      compressed_size: payload.length,
      created_at: now,
    });

    ctx.waitUntil(
      audit(env, {
        userId: user.userId,
        vaultId: body.vault_id,
        action: "vault_created",
        targetId: body.vault_id,
        request,
        metadata: { display_name: body.display_name, snapshot_id: snapshotId },
      }),
    );
    return jsonResponse(
      {
        vault: summaryFromRow(vault),
        cursor: {
          vault_id: vault.vault_id,
          stream_seq: 0,
          logical_clock: vault.current_logical_clock,
        },
      },
      201,
    );
  }

  const vaultMatch = path.match(/^\/v1\/vaults\/([^/]+)$/);
  if (vaultMatch && request.method === "GET") {
    const vault = await getVaultForUser(env.DB, vaultMatch[1], user.userId);
    return jsonResponse({ vault: summaryFromRow(vault) });
  }

  const bootstrapMatch = path.match(/^\/v1\/vaults\/([^/]+)\/bootstrap$/);
  if (bootstrapMatch && request.method === "GET") {
    const vaultId = bootstrapMatch[1];
    const vault = await getVaultForUser(env.DB, vaultId, user.userId);
    const snapshotManifest = await getLatestSnapshotManifest(env.DB, vaultId);
    const snapshotObject = await env.SYNC_BLOBS.get(snapshotManifest.r2_object_key);
    if (!snapshotObject) {
      throw new HttpError(500, "Latest snapshot payload is missing from R2.");
    }
    const snapshot = await snapshotObject.json<unknown>();
    const commits = await listCommitsAfterSeq(env.DB, vaultId, snapshotManifest.stream_seq_at_snapshot, 200);
    const envelopes = await Promise.all(commits.map((row) => loadCommitEnvelope(env, row)));
    return jsonResponse({
      vault: summaryFromRow(vault),
      snapshot,
      commits_after_snapshot: envelopes,
      cursor: {
        vault_id: vault.vault_id,
        stream_seq: vault.current_stream_seq,
        logical_clock: vault.current_logical_clock,
      },
    });
  }

  const snapshotMatch = path.match(/^\/v1\/vaults\/([^/]+)\/snapshot$/);
  if (snapshotMatch && request.method === "PUT") {
    const vaultId = snapshotMatch[1];
    const vault = await getVaultForUser(env.DB, vaultId, user.userId);
    const body = await readJson<SnapshotUploadRequest>(request);
    if (body.snapshot.header.vault_id !== vaultId) {
      throw new HttpError(400, "Snapshot header vault_id must match the route vault_id.");
    }
    const now = nowTs();
    const snapshotId = crypto.randomUUID();
    const objectKey = snapshotObjectKey(vaultId, snapshotId);
    const payloadSha256 = await jsonPayloadDigest(body.snapshot);
    const payload = JSON.stringify(body.snapshot);
    await env.SYNC_BLOBS.put(objectKey, payload, {
      httpMetadata: { contentType: "application/json" },
    });
    await insertSnapshotManifest(env.DB, {
      snapshot_id: snapshotId,
      vault_id: vaultId,
      stream_seq_at_snapshot: Math.max(body.base_stream_seq, vault.current_stream_seq),
      logical_clock_at_snapshot: body.snapshot.header.last_logical_clock,
      r2_object_key: objectKey,
      payload_sha256: payloadSha256,
      compressed_size: payload.length,
      created_at: now,
    });
    await (await import("./lib/db")).setLatestSnapshot(env.DB, {
      vaultId,
      snapshotId,
      logicalClock: body.snapshot.header.last_logical_clock,
      now,
    });
    ctx.waitUntil(
      audit(env, {
        userId: user.userId,
        vaultId,
        action: "snapshot_uploaded",
        request,
        targetId: snapshotId,
        metadata: { base_stream_seq: body.base_stream_seq, idempotency_key: body.idempotency_key },
      }),
    );
    return jsonResponse({
      snapshot_id: snapshotId,
      stream_seq_at_snapshot: Math.max(body.base_stream_seq, vault.current_stream_seq),
      logical_clock_at_snapshot: body.snapshot.header.last_logical_clock,
    });
  }

  const commitMatch = path.match(/^\/v1\/vaults\/([^/]+)\/commits$/);
  if (commitMatch && request.method === "POST") {
    const vaultId = commitMatch[1];
    const vault = await getVaultForUser(env.DB, vaultId, user.userId);
    const body = await readJson<CommitUploadRequest>(request);
    const stub = env.VAULT_AUTHORITY.get(env.VAULT_AUTHORITY.idFromName(vaultId));
    const upstream = await stub.fetch("https://vault-authority.internal/append-commit", {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify({
        user_id: user.userId,
        vault,
        upload: body,
      }),
    });
    ctx.waitUntil(
      audit(env, {
        userId: user.userId,
        vaultId,
        action: "commit_uploaded",
        request,
        metadata: {
          author_device_id: body.author_device_id,
          idempotency_key: body.idempotency_key,
          base_logical_clock: body.base_logical_clock,
        },
      }),
    );
    return upstream;
  }

  if (commitMatch && request.method === "GET") {
    const vaultId = commitMatch[1];
    const vault = await getVaultForUser(env.DB, vaultId, user.userId);
    const afterSeq = Number.parseInt(url.searchParams.get("after_seq") ?? "0", 10);
    const limit = Math.max(1, Math.min(200, Number.parseInt(url.searchParams.get("limit") ?? "100", 10)));
    const commits = await listCommitsAfterSeq(env.DB, vaultId, afterSeq, limit);
    return jsonResponse({
      commits: await Promise.all(commits.map((row) => loadCommitEnvelope(env, row))),
      next_after_seq: commits.length > 0 ? commits[commits.length - 1].stream_seq : afterSeq,
      head_stream_seq: vault.current_stream_seq,
      head_logical_clock: vault.current_logical_clock,
    });
  }

  const eventsMatch = path.match(/^\/v1\/vaults\/([^/]+)\/events$/);
  if (eventsMatch && request.method === "GET") {
    const vaultId = eventsMatch[1];
    await getVaultForUser(env.DB, vaultId, user.userId);
    const stub = env.VAULT_AUTHORITY.get(env.VAULT_AUTHORITY.idFromName(vaultId));
    return stub.fetch("https://vault-authority.internal/events", request);
  }

  if (path === "/v1/rooms" && request.method === "POST") {
    const body = await readJson<CreateRoomRequest>(request);
    await getVaultForUser(env.DB, body.vault_id, user.userId);
    const roomId = crypto.randomUUID();
    await createRoom(env.DB, {
      roomId,
      vaultId: body.vault_id,
      creatorUserId: user.userId,
      roomName: body.room_name,
      roomKind: body.kind,
      sourceHostId: body.source_host_id ?? null,
      createdAt: nowTs(),
    });
    ctx.waitUntil(
      audit(env, {
        userId: user.userId,
        vaultId: body.vault_id,
        action: "room_created",
        request,
        targetId: roomId,
        metadata: body,
      }),
    );
    return jsonResponse(
      {
        room_id: roomId,
        vault_id: body.vault_id,
        kind: body.kind,
        created_at: nowTs(),
      },
      201,
    );
  }

  const joinRoomMatch = path.match(/^\/v1\/rooms\/([^/]+)\/join$/);
  if (joinRoomMatch && request.method === "POST") {
    const room = await (await import("./lib/db")).getRoomForUser(env.DB, joinRoomMatch[1], user.userId);
    return jsonResponse({
      room_id: room.room_id,
      participant_id: crypto.randomUUID(),
    });
  }

  const roomTokenMatch = path.match(/^\/v1\/rooms\/([^/]+)\/(publish-token|subscribe-token)$/);
  if (roomTokenMatch && request.method === "POST") {
    const room = await (await import("./lib/db")).getRoomForUser(env.DB, roomTokenMatch[1], user.userId);
    const expiresAt = nowTs() + 300;
    const token = await signToken(
      {
        sub: user.userId,
        room_id: room.room_id,
        vault_id: room.vault_id,
        kind: room.room_kind,
        scope: roomTokenMatch[2] === "publish-token" ? "publish" : "subscribe",
        exp: expiresAt,
      },
      env.SYNC_SIGNING_KEY,
    );
    return jsonResponse({
      room_id: room.room_id,
      token,
      expires_at: expiresAt,
      moq_origin: env.MOQ_ORIGIN,
    });
  }

  return jsonResponse({ error: "Not found." }, 404);
}

async function handleQueue(
  batch: MessageBatch<unknown>,
  env: Env,
  ctx: ExecutionContext,
): Promise<void> {
  for (const message of batch.messages) {
    console.log(
      JSON.stringify({
        level: "info",
        msg: "queue_message_received",
        queue: batch.queue,
        body: message.body,
      }),
    );

    if (batch.queue === "seance-sync-jobs") {
      ctx.waitUntil(
        audit(env, {
          action: "job_queue_message_processed",
          request: new Request("https://internal.queue/jobs"),
          metadata: { body: message.body },
        }),
      );
    }

    message.ack();
  }
}

export default {
  fetch(request, env, ctx) {
    return withErrorBoundary(() => handleFetch(request, env, ctx));
  },
  queue(batch, env, ctx) {
    return handleQueue(batch, env, ctx);
  },
} satisfies ExportedHandler<Env>;
