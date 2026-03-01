//! Database migration system for PIChain.
//!
//! Tracks a `DB_VERSION` key in CF_METADATA and applies forward-only migrations
//! when the binary is newer than the on-disk schema. Migrations run inside the
//! existing RocksDB instance and are idempotent where possible.

use crate::db::PiChainDB;
use crate::StorageError;
use tracing::{info, warn};

/// Key in CF_METADATA that stores the current database schema version.
const DB_VERSION_KEY: &[u8] = b"__db_version__";

/// Current schema version — bump this when adding a new migration.
const CURRENT_VERSION: u32 = 2;

/// Read the current database version. Returns 0 for databases created before
/// the migration system existed.
pub fn get_db_version(db: &PiChainDB) -> Result<u32, StorageError> {
    match db.get_metadata(DB_VERSION_KEY)? {
        Some(bytes) => {
            if bytes.len() != 4 {
                return Err(StorageError::Serialization(
                    "DB_VERSION is not 4 bytes".into(),
                ));
            }
            let arr: [u8; 4] = bytes
                .try_into()
                .map_err(|_| StorageError::Serialization("DB_VERSION conversion failed".into()))?;
            Ok(u32::from_le_bytes(arr))
        }
        None => Ok(0), // Pre-migration database
    }
}

/// Write the database version.
fn set_db_version(db: &PiChainDB, version: u32) -> Result<(), StorageError> {
    db.put_metadata(DB_VERSION_KEY, &version.to_le_bytes())
}

/// Run all pending migrations to bring the database up to `CURRENT_VERSION`.
///
/// Call this right after opening the database, before any other reads.
/// Migrations are applied one-at-a-time in order and each version is
/// committed before advancing to the next.
pub fn migrate_to_latest(db: &PiChainDB) -> Result<(), StorageError> {
    let mut version = get_db_version(db)?;

    if version > CURRENT_VERSION {
        return Err(StorageError::IntegrityViolation(format!(
            "database version {version} is newer than binary version {CURRENT_VERSION} — \
             refusing to downgrade. Use the matching pichain binary."
        )));
    }

    if version == CURRENT_VERSION {
        info!(
            db_version = CURRENT_VERSION,
            "database schema is up to date"
        );
        return Ok(());
    }

    info!(
        from = version,
        to = CURRENT_VERSION,
        "running database migrations"
    );

    while version < CURRENT_VERSION {
        let next = version + 1;
        match next {
            1 => migrate_v0_to_v1(db)?,
            2 => migrate_v1_to_v2(db)?,
            other => {
                return Err(StorageError::IntegrityViolation(format!(
                    "no migration handler for version {other}"
                )));
            }
        }
        set_db_version(db, next)?;
        info!(version = next, "migration complete");
        version = next;
    }

    info!(
        version = CURRENT_VERSION,
        "all migrations applied successfully"
    );
    Ok(())
}

/// v0 → v1: Stamp the version marker on pre-existing databases.
/// No schema changes — just records that the DB has been seen by the migration system.
fn migrate_v0_to_v1(db: &PiChainDB) -> Result<(), StorageError> {
    // Check if this is an existing database with data
    match db.get_metadata(b"latest_height")? {
        Some(_) => {
            warn!("migrating existing database to version 1 (adding version marker)");
        }
        None => {
            info!("fresh database — stamping version 1");
        }
    }
    // The new column families (CF_TX_HISTORY, CF_EVENT_INDEX, CF_EVM_STATE) are
    // automatically created by RocksDB because we set `create_missing_column_families(true)`.
    // This migration just marks the database as v1-aware.
    Ok(())
}

/// v1 → v2: Verify that new column families exist (they're auto-created by the
/// `open()` call, but we log confirmation for operators).
fn migrate_v1_to_v2(db: &PiChainDB) -> Result<(), StorageError> {
    // CF_TX_HISTORY, CF_EVENT_INDEX, CF_EVM_STATE are created automatically
    // when the DB opens with create_missing_column_families(true) and they're
    // listed in ALL_CFS. This migration validates they're accessible.
    let cfs_to_check = ["tx_history", "event_index", "evm_state"];
    for cf_name in &cfs_to_check {
        // Try to access the CF — this will panic if it doesn't exist,
        // but it should exist because ALL_CFS includes it.
        if db.inner().cf_handle(cf_name).is_some() {
            info!(cf = cf_name, "column family verified");
        } else {
            warn!(
                cf = cf_name,
                "column family missing — may be created on next restart"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_starts_at_version_0() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        assert_eq!(get_db_version(&db).unwrap(), 0);
    }

    #[test]
    fn migrate_to_latest_on_fresh_db() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        migrate_to_latest(&db).unwrap();
        assert_eq!(get_db_version(&db).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        migrate_to_latest(&db).unwrap();
        migrate_to_latest(&db).unwrap(); // Second call is a no-op
        assert_eq!(get_db_version(&db).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn rejects_future_version() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        set_db_version(&db, 999).unwrap();
        let err = migrate_to_latest(&db).unwrap_err();
        assert!(err.to_string().contains("newer than binary"));
    }

    #[test]
    fn incremental_migration() {
        let (db, _tmp) = PiChainDB::open_temp().unwrap();
        // Simulate a v1 database
        set_db_version(&db, 1).unwrap();
        migrate_to_latest(&db).unwrap();
        assert_eq!(get_db_version(&db).unwrap(), CURRENT_VERSION);
    }
}
