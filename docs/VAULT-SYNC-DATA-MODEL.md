# Vault Sync Data Model

## Purpose

Describe the persistent server-side data model for hosted vault sync and future multiplayer authorization.

## Scope

- D1 relational schema
- R2 object naming
- Durable Object ownership model
- state transitions for vaults and replicas

## Assumptions

- D1 is the control-plane source of truth
- R2 stores opaque encrypted payloads only
- Durable Objects coordinate writes but do not replace relational metadata
- shared-vault readiness exists in schema before owner-only sync is enabled

## Glossary

- **manifest**: relational row describing an R2 blob
- **replica**: device-level hosted sync participant
- **authority**: per-vault or per-room Durable Object responsible for serialized coordination

## D1 Entities

| Table | Purpose |
| --- | --- |
| `users` | stable user identity |
| `magic_link_challenges` | single-use login challenges |
| `sessions` | refresh-token backed login sessions |
| `vaults` | vault metadata and current hosted head |
| `vault_memberships` | role and lifecycle for vault access |
| `vault_replicas` | device/replica tracking |
| `vault_snapshot_manifests` | latest and historical snapshot manifests |
| `vault_commits` | append-only commit manifests |
| `vault_idempotency_keys` | safe commit retry ledger |
| `multiplayer_rooms` | room metadata linked to vault access |
| `audit_log` | security and operator audit trail |

## ER Diagram

```mermaid
erDiagram
    users ||--o{ sessions : owns
    users ||--o{ vault_memberships : participates_in
    users ||--o{ vaults : owns
    users ||--o{ vault_commits : authors
    users ||--o{ multiplayer_rooms : creates

    vaults ||--o{ vault_memberships : grants
    vaults ||--o{ vault_replicas : tracks
    vaults ||--o{ vault_snapshot_manifests : snapshots
    vaults ||--o{ vault_commits : commits
    vaults ||--o{ vault_idempotency_keys : dedupes
    vaults ||--o{ multiplayer_rooms : contains
    vaults ||--o{ audit_log : audits

    users {
        text user_id PK
        text primary_email
        int created_at
        int updated_at
        int disabled_at
    }
    sessions {
        text session_id PK
        text user_id FK
        text refresh_token_hash
        int issued_at
        int expires_at
        int revoked_at
    }
    magic_link_challenges {
        text challenge_id PK
        text email
        text hashed_token
        int expires_at
        int consumed_at
        int created_at
    }
    vaults {
        text vault_id PK
        text owner_user_id FK
        text display_name
        int created_at
        int updated_at
        int deleted_at
        int current_stream_seq
        int current_logical_clock
        text latest_snapshot_id
    }
    vault_memberships {
        text vault_id FK
        text user_id FK
        text role
        int invited_at
        int accepted_at
        int revoked_at
    }
    vault_replicas {
        text vault_id FK
        text device_id
        text device_name_hint
        int last_seen_stream_seq
        int last_seen_logical_clock
        int created_at
        int updated_at
        int revoked_at
    }
    vault_snapshot_manifests {
        text snapshot_id PK
        text vault_id FK
        int stream_seq_at_snapshot
        int logical_clock_at_snapshot
        text r2_object_key
        text payload_sha256
        int compressed_size
        int created_at
    }
    vault_commits {
        text vault_id FK
        int stream_seq
        text commit_id
        text author_user_id FK
        text author_device_id
        int base_logical_clock
        int result_logical_clock
        int record_count
        text r2_object_key
        text payload_sha256
        int created_at
    }
    vault_idempotency_keys {
        text vault_id FK
        text device_id
        text idempotency_key
        int stream_seq
        int created_at
    }
    multiplayer_rooms {
        text room_id PK
        text vault_id FK
        text creator_user_id FK
        text room_name
        text room_kind
        text source_host_id
        int created_at
    }
    audit_log {
        text audit_id PK
        text user_id FK
        text vault_id FK
        text action
        text target_id
        int occurred_at
        text ip_hash
        text metadata_json
    }
```

## R2 Object Naming

### Snapshots

```text
snapshots/<vault_id>/<snapshot_id>.json
```

### Commits

```text
commits/<vault_id>/<zero-padded-stream-seq>-<commit_id>.json
```

Properties:

- easy to inspect by vault
- stream order visible in key names
- append-only retention compatible
- safe to rebuild D1 manifests from R2 inventory if needed

## Durable Object Ownership Model

### VaultAuthority

Owns:

- commit append serialization
- `stream_seq` allocation
- notification fanout

Does not own:

- long-term manifests
- user identity
- room metadata

### RoomAuthority

Owns:

- room-local realtime coordination
- future presence state
- room-side event fanout

Does not own:

- vault membership source of truth
- commit ordering

## State Diagrams

### Vault Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Created
    Created --> Active: bootstrap snapshot uploaded
    Active --> Compacted: new snapshot written
    Compacted --> Active: new commits appended
    Active --> SoftDeleted: delete requested
    Compacted --> SoftDeleted: delete requested
    SoftDeleted --> Purged: cleanup completed
```

### Replica Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Registered
    Registered --> Active: bootstrap completed
    Active --> Idle: no recent sync activity
    Idle --> Active: pull or upload resumes
    Active --> Revoked: admin or owner revokes replica
    Idle --> Revoked: admin or owner revokes replica
```

## Head Tracking

Every vault keeps two hosted head values:

- `current_stream_seq`
- `current_logical_clock`

`current_stream_seq` is advanced only by accepted commits.

`current_logical_clock` is updated to the max of:

- current vault logical head
- incoming delta `to_clock`
- incoming record clocks
- uploaded snapshot header clock

## Data Visibility

### Visible in D1

- user email
- vault display name
- membership role
- device ids and last seen metadata
- room names
- audit metadata

### Visible in R2

- opaque encrypted snapshots
- opaque encrypted deltas

### Never visible server-side

- decrypted hosts
- decrypted credentials
- decrypted keys
- recovery passphrases
- master keys

