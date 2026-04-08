import { DurableObject } from "cloudflare:workers";

import { nowTs, randomToken } from "../lib/crypto";
import { type VaultSummaryRow } from "../lib/db";
import { HttpError, jsonResponse, readJson } from "../lib/http";
import { commitObjectKey, jsonPayloadDigest } from "../lib/r2";

type CommitUpload = {
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

type AppendCommitRequest = {
  user_id: string;
  vault: VaultSummaryRow;
  upload: CommitUpload;
};

export class VaultAuthority extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/append-commit" && request.method === "POST") {
      const body = await readJson<AppendCommitRequest>(request);
      return this.appendCommit(body);
    }
    if (url.pathname === "/events" && request.method === "GET") {
      return this.openEventStream(request);
    }
    return jsonResponse({ error: "Not found." }, 404);
  }

  private async appendCommit(body: AppendCommitRequest): Promise<Response> {
    const { vault, upload, user_id: userId } = body;
    if (upload.delta.vault_id !== vault.vault_id) {
      throw new HttpError(400, "Commit delta vault_id does not match the route vault.");
    }
    if (upload.delta.to_clock < upload.delta.from_clock) {
      throw new HttpError(400, "Commit delta to_clock must be greater than or equal to from_clock.");
    }
    if (upload.delta.records.length > 512) {
      throw new HttpError(413, "Commit delta exceeds the maximum record count.");
    }

    const existing = await this.env.DB
      .prepare(
        `SELECT stream_seq
           FROM vault_idempotency_keys
          WHERE vault_id = ?1
            AND device_id = ?2
            AND idempotency_key = ?3`,
      )
      .bind(vault.vault_id, upload.author_device_id, upload.idempotency_key)
      .first<{ stream_seq: number }>();

    if (existing) {
      return jsonResponse({
        accepted_stream_seq: existing.stream_seq,
        replication_cursor: {
          vault_id: vault.vault_id,
          stream_seq: vault.current_stream_seq,
          logical_clock: vault.current_logical_clock,
        },
        head_logical_clock: vault.current_logical_clock,
      });
    }

    const commitId = crypto.randomUUID();
    const createdAt = nowTs();
    const streamSeq = vault.current_stream_seq + 1;
    const resultLogicalClock = Math.max(
      vault.current_logical_clock,
      upload.delta.to_clock,
      ...upload.delta.records.map((record) => {
        const maybeClock = (record as { logical_clock?: number }).logical_clock;
        return typeof maybeClock === "number" ? maybeClock : 0;
      }),
    );
    const payloadSha256 = await jsonPayloadDigest(upload.delta);
    const objectKey = commitObjectKey(vault.vault_id, streamSeq, commitId);
    const serialized = JSON.stringify(upload.delta);

    await this.env.SYNC_BLOBS.put(objectKey, serialized, {
      httpMetadata: { contentType: "application/json" },
    });

    await this.env.DB.batch([
      this.env.DB
        .prepare(
          `INSERT INTO vault_commits (
             vault_id, stream_seq, commit_id, author_user_id, author_device_id,
             base_logical_clock, result_logical_clock, record_count,
             r2_object_key, payload_sha256, created_at
           ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)`,
        )
        .bind(
          vault.vault_id,
          streamSeq,
          commitId,
          userId,
          upload.author_device_id,
          upload.base_logical_clock,
          resultLogicalClock,
          upload.delta.records.length,
          objectKey,
          payloadSha256,
          createdAt,
        ),
      this.env.DB
        .prepare(
          `INSERT INTO vault_idempotency_keys (
             vault_id, device_id, idempotency_key, stream_seq, created_at
           ) VALUES (?1, ?2, ?3, ?4, ?5)`,
        )
        .bind(
          vault.vault_id,
          upload.author_device_id,
          upload.idempotency_key,
          streamSeq,
          createdAt,
        ),
      this.env.DB
        .prepare(
          `INSERT INTO vault_replicas (
             vault_id, device_id, device_name_hint, last_seen_stream_seq, last_seen_logical_clock,
             created_at, updated_at, revoked_at
           ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
           ON CONFLICT(vault_id, device_id) DO UPDATE SET
             last_seen_stream_seq = excluded.last_seen_stream_seq,
             last_seen_logical_clock = excluded.last_seen_logical_clock,
             updated_at = excluded.updated_at`,
        )
        .bind(
          vault.vault_id,
          upload.author_device_id,
          `device-${upload.author_device_id.slice(0, 8)}`,
          streamSeq,
          resultLogicalClock,
          createdAt,
          createdAt,
        ),
      this.env.DB
        .prepare(
          `UPDATE vaults
              SET current_stream_seq = ?1,
                  current_logical_clock = ?2,
                  updated_at = ?3
            WHERE vault_id = ?4`,
        )
        .bind(streamSeq, resultLogicalClock, createdAt, vault.vault_id),
    ]);

    this.broadcast({
      type: "commit_available",
      vault_id: vault.vault_id,
      stream_seq: streamSeq,
      logical_clock: resultLogicalClock,
      commit_id: commitId,
      created_at: createdAt,
      event_id: randomToken(8),
    });

    return jsonResponse({
      accepted_stream_seq: streamSeq,
      replication_cursor: {
        vault_id: vault.vault_id,
        stream_seq: streamSeq,
        logical_clock: resultLogicalClock,
      },
      head_logical_clock: resultLogicalClock,
    });
  }

  private openEventStream(request: Request): Response {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      throw new HttpError(426, "Upgrade to WebSocket is required.");
    }

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    this.ctx.acceptWebSocket(server);
    server.send(
      JSON.stringify({
        type: "connected",
        connected_at: nowTs(),
      }),
    );

    return new Response(null, {
      status: 101,
      webSocket: client,
    });
  }

  private broadcast(payload: Record<string, unknown>): void {
    const serialized = JSON.stringify(payload);
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.send(serialized);
      } catch (error) {
        console.warn(
          JSON.stringify({
            level: "warn",
            msg: "vault_event_broadcast_failed",
            error: error instanceof Error ? error.message : String(error),
          }),
        );
      }
    }
  }
}

