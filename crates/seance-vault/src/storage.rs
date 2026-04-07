use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    VaultError, VaultResult,
    kdf::KdfParams,
    model::{
        DeviceEnrollment, EncryptedRecord, RecordKind, RecoveryBundle, VAULT_SCHEMA_VERSION,
        VaultHeader,
    },
};

pub fn initialize_schema(conn: &Connection) -> VaultResult<()> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS vault_header (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            vault_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            cipher TEXT NOT NULL,
            recovery_salt BLOB NOT NULL,
            recovery_memory_kib INTEGER NOT NULL,
            recovery_iterations INTEGER NOT NULL,
            recovery_parallelism INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_logical_clock INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS recovery_bundles (
            bundle_id TEXT PRIMARY KEY,
            salt BLOB NOT NULL,
            memory_kib INTEGER NOT NULL,
            iterations INTEGER NOT NULL,
            parallelism INTEGER NOT NULL,
            wrapping_nonce BLOB NOT NULL,
            wrapped_master_key BLOB NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS device_enrollments (
            device_id TEXT PRIMARY KEY,
            device_name TEXT NOT NULL,
            wrapping_nonce BLOB NOT NULL,
            wrapped_master_key BLOB NOT NULL,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER NOT NULL,
            revoked_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS records (
            record_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            version INTEGER NOT NULL,
            logical_clock INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            deleted_at INTEGER,
            key_nonce BLOB NOT NULL,
            wrapped_record_key BLOB NOT NULL,
            payload_nonce BLOB NOT NULL,
            payload_ciphertext BLOB NOT NULL,
            last_synced_clock INTEGER,
            sync_state TEXT NOT NULL DEFAULT 'pending'
        );

        CREATE INDEX IF NOT EXISTS idx_records_kind_deleted
            ON records(kind, deleted_at, modified_at DESC);
        CREATE INDEX IF NOT EXISTS idx_records_sync
            ON records(sync_state, logical_clock);

        CREATE TABLE IF NOT EXISTS local_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;

    Ok(())
}

pub fn load_header(conn: &Connection) -> VaultResult<Option<VaultHeader>> {
    conn.query_row(
        "
        SELECT vault_id, schema_version, cipher, recovery_salt, recovery_memory_kib,
               recovery_iterations, recovery_parallelism, created_at, updated_at, last_logical_clock
        FROM vault_header
        WHERE singleton = 1
        ",
        [],
        |row| {
            Ok(VaultHeader {
                vault_id: row.get(0)?,
                schema_version: row.get(1)?,
                cipher: row.get(2)?,
                recovery_kdf: KdfParams {
                    salt: row.get(3)?,
                    memory_kib: row.get(4)?,
                    iterations: row.get(5)?,
                    parallelism: row.get(6)?,
                },
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                last_logical_clock: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn insert_header(conn: &Connection, header: &VaultHeader) -> VaultResult<()> {
    conn.execute(
        "
        INSERT INTO vault_header (
            singleton, vault_id, schema_version, cipher, recovery_salt, recovery_memory_kib,
            recovery_iterations, recovery_parallelism, created_at, updated_at, last_logical_clock
        )
        VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ",
        params![
            header.vault_id,
            header.schema_version,
            header.cipher,
            header.recovery_kdf.salt,
            header.recovery_kdf.memory_kib,
            header.recovery_kdf.iterations,
            header.recovery_kdf.parallelism,
            header.created_at,
            header.updated_at,
            header.last_logical_clock,
        ],
    )?;

    Ok(())
}

pub fn update_header_after_rotation(
    conn: &Connection,
    updated_at: i64,
    kdf: &KdfParams,
) -> VaultResult<()> {
    conn.execute(
        "
        UPDATE vault_header
        SET recovery_salt = ?1,
            recovery_memory_kib = ?2,
            recovery_iterations = ?3,
            recovery_parallelism = ?4,
            updated_at = ?5
        WHERE singleton = 1
        ",
        params![
            kdf.salt,
            kdf.memory_kib,
            kdf.iterations,
            kdf.parallelism,
            updated_at,
        ],
    )?;

    Ok(())
}

pub fn bump_logical_clock(conn: &Connection) -> VaultResult<u64> {
    let current: u64 = conn.query_row(
        "SELECT last_logical_clock FROM vault_header WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let next = current + 1;
    conn.execute(
        "UPDATE vault_header SET last_logical_clock = ?1, updated_at = ?2 WHERE singleton = 1",
        params![next, crate::now_ts()],
    )?;
    Ok(next)
}

pub fn insert_recovery_bundle(conn: &Connection, bundle: &RecoveryBundle) -> VaultResult<()> {
    conn.execute("DELETE FROM recovery_bundles", [])?;
    conn.execute(
        "
        INSERT INTO recovery_bundles (
            bundle_id, salt, memory_kib, iterations, parallelism,
            wrapping_nonce, wrapped_master_key, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ",
        params![
            bundle.bundle_id,
            bundle.params.salt,
            bundle.params.memory_kib,
            bundle.params.iterations,
            bundle.params.parallelism,
            bundle.wrapping_nonce,
            bundle.wrapped_master_key,
            bundle.created_at,
            bundle.updated_at,
        ],
    )?;
    Ok(())
}

pub fn load_recovery_bundle(conn: &Connection) -> VaultResult<Option<RecoveryBundle>> {
    conn.query_row(
        "
        SELECT bundle_id, salt, memory_kib, iterations, parallelism, wrapping_nonce,
               wrapped_master_key, created_at, updated_at
        FROM recovery_bundles
        LIMIT 1
        ",
        [],
        |row| {
            Ok(RecoveryBundle {
                bundle_id: row.get(0)?,
                params: KdfParams {
                    salt: row.get(1)?,
                    memory_kib: row.get(2)?,
                    iterations: row.get(3)?,
                    parallelism: row.get(4)?,
                },
                wrapping_nonce: row.get(5)?,
                wrapped_master_key: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn upsert_device_enrollment(
    conn: &Connection,
    enrollment: &DeviceEnrollment,
) -> VaultResult<()> {
    conn.execute(
        "
        INSERT INTO device_enrollments (
            device_id, device_name, wrapping_nonce, wrapped_master_key,
            created_at, last_used_at, revoked_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(device_id) DO UPDATE SET
            device_name = excluded.device_name,
            wrapping_nonce = excluded.wrapping_nonce,
            wrapped_master_key = excluded.wrapped_master_key,
            last_used_at = excluded.last_used_at,
            revoked_at = excluded.revoked_at
        ",
        params![
            enrollment.device_id,
            enrollment.device_name,
            enrollment.wrapping_nonce,
            enrollment.wrapped_master_key,
            enrollment.created_at,
            enrollment.last_used_at,
            enrollment.revoked_at,
        ],
    )?;
    Ok(())
}

pub fn load_device_enrollment(
    conn: &Connection,
    device_id: &str,
) -> VaultResult<Option<DeviceEnrollment>> {
    conn.query_row(
        "
        SELECT device_id, device_name, wrapping_nonce, wrapped_master_key,
               created_at, last_used_at, revoked_at
        FROM device_enrollments
        WHERE device_id = ?1
        ",
        params![device_id],
        device_enrollment_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_device_last_used(
    conn: &Connection,
    device_id: &str,
    timestamp: i64,
) -> VaultResult<()> {
    conn.execute(
        "
        UPDATE device_enrollments
        SET last_used_at = ?2
        WHERE device_id = ?1
        ",
        params![device_id, timestamp],
    )?;
    Ok(())
}

pub fn set_local_state(conn: &Connection, key: &str, value: &str) -> VaultResult<()> {
    conn.execute(
        "
        INSERT INTO local_state (key, value)
        VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        ",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_local_state(conn: &Connection, key: &str) -> VaultResult<Option<String>> {
    conn.query_row(
        "SELECT value FROM local_state WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn delete_local_state(conn: &Connection, key: &str) -> VaultResult<()> {
    conn.execute("DELETE FROM local_state WHERE key = ?1", params![key])?;
    Ok(())
}

pub fn upsert_record(conn: &Connection, record: &EncryptedRecord) -> VaultResult<()> {
    conn.execute(
        "
        INSERT INTO records (
            record_id, kind, version, logical_clock, modified_at, deleted_at,
            key_nonce, wrapped_record_key, payload_nonce, payload_ciphertext,
            last_synced_clock, sync_state
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, 'pending')
        ON CONFLICT(record_id) DO UPDATE SET
            kind = excluded.kind,
            version = excluded.version,
            logical_clock = excluded.logical_clock,
            modified_at = excluded.modified_at,
            deleted_at = excluded.deleted_at,
            key_nonce = excluded.key_nonce,
            wrapped_record_key = excluded.wrapped_record_key,
            payload_nonce = excluded.payload_nonce,
            payload_ciphertext = excluded.payload_ciphertext,
            sync_state = 'pending'
        ",
        params![
            record.record_id,
            record.kind.as_str(),
            record.version,
            record.logical_clock,
            record.modified_at,
            record.deleted_at,
            record.key_nonce,
            record.wrapped_record_key,
            record.payload_nonce,
            record.payload_ciphertext,
        ],
    )?;
    Ok(())
}

pub fn list_records_by_kind(
    conn: &Connection,
    kind: RecordKind,
) -> VaultResult<Vec<EncryptedRecord>> {
    let mut stmt = conn.prepare(
        "
        SELECT record_id, kind, version, logical_clock, modified_at, deleted_at,
               key_nonce, wrapped_record_key, payload_nonce, payload_ciphertext
        FROM records
        WHERE kind = ?1 AND deleted_at IS NULL
        ORDER BY modified_at DESC
        ",
    )?;

    let rows = stmt.query_map(params![kind.as_str()], encrypted_record_from_row)?;
    let records = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(records)
}

pub fn load_record(
    conn: &Connection,
    record_id: &str,
    kind: RecordKind,
) -> VaultResult<Option<EncryptedRecord>> {
    conn.query_row(
        "
        SELECT record_id, kind, version, logical_clock, modified_at, deleted_at,
               key_nonce, wrapped_record_key, payload_nonce, payload_ciphertext
        FROM records
        WHERE record_id = ?1 AND kind = ?2
        ",
        params![record_id, kind.as_str()],
        encrypted_record_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn verify_header_integrity(header: &VaultHeader) -> VaultResult<()> {
    if header.schema_version != VAULT_SCHEMA_VERSION {
        return Err(VaultError::UnsupportedSchemaVersion {
            version: header.schema_version,
        });
    }
    if header.vault_id.is_empty() {
        return Err(VaultError::CorruptVault(
            "vault header is missing the vault id".into(),
        ));
    }
    if header.recovery_kdf.salt.is_empty() {
        return Err(VaultError::CorruptVault(
            "vault header is missing the recovery salt".into(),
        ));
    }
    Ok(())
}

fn encrypted_record_from_row(row: &Row<'_>) -> rusqlite::Result<EncryptedRecord> {
    let kind_raw: String = row.get(1)?;
    let kind = kind_raw.parse::<RecordKind>().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(VaultError::CorruptVault(format!(
                "unsupported record kind stored in sqlite: {kind_raw}"
            ))),
        )
    })?;

    Ok(EncryptedRecord {
        record_id: row.get(0)?,
        kind,
        version: row.get(2)?,
        logical_clock: row.get(3)?,
        modified_at: row.get(4)?,
        deleted_at: row.get(5)?,
        key_nonce: row.get(6)?,
        wrapped_record_key: row.get(7)?,
        payload_nonce: row.get(8)?,
        payload_ciphertext: row.get(9)?,
    })
}

fn device_enrollment_from_row(row: &Row<'_>) -> rusqlite::Result<DeviceEnrollment> {
    Ok(DeviceEnrollment {
        device_id: row.get(0)?,
        device_name: row.get(1)?,
        wrapping_nonce: row.get(2)?,
        wrapped_master_key: row.get(3)?,
        created_at: row.get(4)?,
        last_used_at: row.get(5)?,
        revoked_at: row.get(6)?,
    })
}
