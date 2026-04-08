# seance-sync

Cloudflare-first hosted sync authority for encrypted vault replication and future multiplayer transport.

## Scripts

```bash
npm install
npm run check
npm run dev
```

## Bindings

- `DB` - D1 metadata store
- `SYNC_BLOBS` - R2 bucket for opaque encrypted snapshots and commit payloads
- `SYNC_EMAIL_OUTBOX` - queue for magic-link email delivery
- `SYNC_JOBS` - queue for compaction and async sync jobs
- `VAULT_AUTHORITY` - per-vault Durable Object
- `ROOM_AUTHORITY` - realtime room Durable Object

## Local notes

- `SYNC_SIGNING_KEY` is configured in `wrangler.jsonc` as a placeholder for local scaffolding.
- Replace it with a proper secret before deploying.
- D1 migrations live in `migrations/d1/`.
