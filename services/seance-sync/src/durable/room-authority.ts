import { DurableObject } from "cloudflare:workers";

import { HttpError, jsonResponse } from "../lib/http";
import { nowTs, randomToken } from "../lib/crypto";

export class RoomAuthority extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/presence" && request.method === "GET") {
      return this.openPresenceSocket(request);
    }
    if (url.pathname === "/announce" && request.method === "POST") {
      const payload = await request.json<Record<string, unknown>>();
      this.broadcast({
        ...payload,
        emitted_at: nowTs(),
        event_id: randomToken(8),
      });
      return jsonResponse({ ok: true });
    }
    return jsonResponse({ error: "Not found." }, 404);
  }

  private openPresenceSocket(request: Request): Response {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      throw new HttpError(426, "Upgrade to WebSocket is required.");
    }

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    this.ctx.acceptWebSocket(server);
    server.send(JSON.stringify({ type: "presence_connected", connected_at: nowTs() }));
    return new Response(null, { status: 101, webSocket: client });
  }

  private broadcast(payload: Record<string, unknown>): void {
    const serialized = JSON.stringify(payload);
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.send(serialized);
      } catch {}
    }
  }
}

