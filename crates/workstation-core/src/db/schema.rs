// SPDX-License-Identifier: GPL-3.0-or-later
//! Initial atomic migration; unknown databases are never adopted or downgraded.

use rusqlite::Connection;
pub const APPLICATION_ID: i64 = 0x4b56_4c4d;
pub const VERSION: i64 = 1;

pub(super) fn initialize(connection: &mut Connection) -> Result<(), String> {
    connection
        .busy_timeout(std::time::Duration::from_secs(1))
        .map_err(|_| "Cannot configure history lock timeout.")?;
    connection
        .pragma_update(None, "trusted_schema", false)
        .map_err(|_| "Cannot secure history connection.")?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "History is locked or unreadable. Close other writers and retry.")?;
    let app: i64 = tx
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|_| "Cannot read history identity.")?;
    let version: i64 = tx
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|_| "Cannot read history version.")?;
    if app == 0 && version == 0 {
        let objects: i64 = tx
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "Cannot inspect history schema.")?;
        if objects != 0 {
            return Err(
                "History path contains an unrelated database. Choose a separate data directory."
                    .into(),
            );
        }
        tx.execute_batch("CREATE TABLE benchmark_runs (
            id INTEGER PRIMARY KEY,
            started_at_unix_ms INTEGER NOT NULL,
            snapshot_json TEXT NOT NULL CHECK(length(CAST(snapshot_json AS BLOB)) <= 262144),
            completion_json TEXT CHECK(completion_json IS NULL OR length(CAST(completion_json AS BLOB)) <= 262144)
        ) STRICT;") .map_err(|_| "Cannot create benchmark history schema.")?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|_| "Cannot set history identity.")?;
        tx.pragma_update(None, "user_version", VERSION)
            .map_err(|_| "Cannot set history version.")?;
    } else if app != APPLICATION_ID || version != VERSION {
        return Err("Unsupported or unrelated history database. Keep the file and use a compatible application version.".into());
    }
    tx.prepare(
        "SELECT id, started_at_unix_ms, snapshot_json, completion_json FROM benchmark_runs LIMIT 0",
    )
    .map_err(|_| "History schema is incompatible. Keep the file for recovery.")?;
    tx.commit()
        .map_err(|_| "Cannot commit history migration. Retry after freeing disk space.")?;
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .map_err(|_| "Cannot read history journal mode.")?;
    if journal != "wal" {
        let journal: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .map_err(|_| "Cannot enable history WAL. Retry on a local writable filesystem.")?;
        if journal != "wal" {
            return Err("History requires WAL on a local writable filesystem.".into());
        }
    }
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| "Cannot enable history foreign keys.")?;
    Ok(())
}
