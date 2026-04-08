# Vault Sync Runbook

## Purpose

Provide the operator procedures for running, validating, and recovering the hosted vault sync service.

## Scope

- day-0 bootstrap
- normal operations
- incident response
- rollback and recovery
- data integrity verification

## Assumptions

- service runs on Cloudflare Workers with D1, R2, Durable Objects, and Queues
- magic-link email delivery is async
- D1 is the metadata authority and R2 is the blob store

## Glossary

- **head**: latest hosted `stream_seq` and `logical_clock`
- **manifest**: D1 row pointing to an R2 payload
- **orphan blob**: R2 object without a committed manifest row

## Detection Signals

- elevated 5xx rate on `/v1/vaults/*`
- queue backlog growth
- missing snapshot or commit blobs during bootstrap or pull
- repeated auth failures on valid users
- rapidly growing orphan blob count
- stalled `stream_seq` growth despite active clients

## Day-0 Setup

1. Provision D1 database.
2. Provision R2 bucket for sync blobs.
3. Provision queues for email and background jobs.
4. Deploy Worker with Durable Object bindings and migrations.
5. Set production signing secret and any email provider secrets.
6. Apply D1 migrations.
7. Run a synthetic bootstrap and commit upload against a test vault.

## Preflight Checks

- D1 schema matches current migration set
- R2 bucket is writable
- Worker can read all required bindings
- queue consumers are attached
- magic-link queue messages are being acked
- Durable Object migrations are current

## Normal Operational Flows

### Bootstrap Validation

- create a test vault
- upload bootstrap snapshot
- fetch `/bootstrap`
- confirm snapshot returns and cursor head is `stream_seq = 0`

### Commit Validation

- upload a test delta with known idempotency key
- confirm `stream_seq` increments
- replay the same upload
- confirm no second commit manifest exists

### Realtime Validation

- connect to `/events`
- upload a commit
- confirm `commit_available` event arrives

## Incident Playbooks

### 1. Missing Snapshot Blob

Symptoms:

- `/bootstrap` returns 500
- D1 snapshot manifest exists but R2 object is missing

Operator steps:

1. identify affected `vault_id` and `snapshot_id`
2. confirm whether a newer snapshot exists
3. if newer snapshot exists, repoint latest snapshot pointer
4. if no snapshot exists, rebuild bootstrap from last known snapshot plus replayable commits
5. if rebuild succeeds, write new snapshot manifest and latest pointer
6. audit the incident

### 2. Missing Commit Blob

Symptoms:

- commit pull fails for specific `commit_id`

Operator steps:

1. identify `vault_id`, `stream_seq`, and `commit_id`
2. check whether R2 object exists under the manifest key
3. if missing and clients still have source state, re-upload reconstructed opaque delta
4. otherwise compact a new snapshot from last safe state and mark recovery boundary
5. notify affected users if replay continuity was broken

### 3. Queue Backlog Growth

Symptoms:

- email or job queue depth grows continuously

Operator steps:

1. inspect recent deploys and consumer logs
2. determine whether backlog is email or job queue specific
3. if consumer is unhealthy, roll back last deployment
4. if payloads are poison messages, isolate and dead-letter them
5. replay safe messages after fix

### 4. Auth Token Abuse

Symptoms:

- unusual refresh churn
- repeated magic-link verification attempts

Operator steps:

1. identify offending IP hashes and user ids
2. revoke affected sessions
3. disable affected user if needed
4. increase rate limiting for the path
5. rotate signing secret if compromise is suspected

## Rollback / Recovery

### Worker Rollback

- redeploy previous known-good Worker build
- verify bindings and Durable Object class names remain compatible
- re-run synthetic bootstrap and pull tests

### D1 Recovery

If D1 manifests are damaged but R2 data still exists:

1. inventory R2 keys for snapshots and commits
2. reconstruct manifests from naming convention and stored payload hashes
3. restore `vaults.current_stream_seq` from highest commit key per vault
4. restore `latest_snapshot_id` from newest valid snapshot per vault
5. verify hashes before accepting rebuild

### R2 Recovery

If R2 data is damaged but D1 manifests remain:

1. identify missing objects from manifest scans
2. locate replicas or offline backups with matching opaque payloads
3. restore blobs under original keys where possible
4. if restore is impossible, compact a fresh snapshot from last valid replay state

## Data Integrity Checks

Run periodically:

- every latest snapshot manifest points to an existing R2 object
- every commit manifest in the replay window points to an existing R2 object
- manifest payload hash matches current R2 object body
- vault head `stream_seq` equals max commit `stream_seq` for the vault
- vault head logical clock is not lower than any snapshot or commit logical clock

## Operator Commands / Tasks To Automate

- snapshot and commit manifest scan
- orphan R2 blob scan
- stale idempotency ledger cleanup
- expired magic-link cleanup
- revoked session cleanup
- room metadata cleanup for closed or stale rooms

## Post-Incident Review Checklist

- root cause recorded
- affected vault ids identified
- recovery steps documented
- any data loss or replay gaps measured
- monitoring or rate limits updated
- documentation updated if runbook was incomplete

