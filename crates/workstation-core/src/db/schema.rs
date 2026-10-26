// SPDX-License-Identifier: GPL-3.0-or-later
//! Ordered atomic migration (0→1, 1→2); unknown databases are never adopted or downgraded.

use rusqlite::Connection;
pub const APPLICATION_ID: i64 = 0x4b56_4c4d;
pub const VERSION: i64 = 2;

const STEP_0_TO_1: &str = "CREATE TABLE benchmark_runs (
    id INTEGER PRIMARY KEY,
    started_at_unix_ms INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL CHECK(length(CAST(snapshot_json AS BLOB)) <= 262144),
    completion_json TEXT CHECK(completion_json IS NULL OR length(CAST(completion_json AS BLOB)) <= 262144)
) STRICT;";

/// Model catalog cache, local file digests, and downloads. `downloads` copies what it
/// needs and has no foreign key, so evicting cached catalog rows never touches it.
const STEP_1_TO_2: &str = "
CREATE TABLE catalog_queries (
    query_key TEXT PRIMARY KEY,
    result_json TEXT NOT NULL CHECK(length(CAST(result_json AS BLOB)) <= 4194304),
    fetched_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE catalog_releases (
    repo_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    release_json TEXT NOT NULL CHECK(length(CAST(release_json AS BLOB)) <= 1048576),
    fetched_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (repo_id, revision)
) STRICT;
CREATE TABLE artifact_metadata (
    repo_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    filename TEXT NOT NULL,
    gguf_json TEXT NOT NULL CHECK(length(CAST(gguf_json AS BLOB)) <= 262144),
    fetched_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (repo_id, revision, filename)
) STRICT;
CREATE TABLE local_file_digests (
    path TEXT PRIMARY KEY,
    dev INTEGER NOT NULL,
    inode INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    ctime_ns INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    verified_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE downloads (
    id INTEGER PRIMARY KEY,
    artifact_id TEXT NOT NULL,
    repo_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    filename TEXT NOT NULL,
    expected_sha256 TEXT NOT NULL,
    expected_size INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','running','paused','verifying','completed','failed','cancelled')),
    bytes_done INTEGER NOT NULL,
    failure_code TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;";

const SHAPE_CHECKS: &[&str] = &[
    "SELECT id, started_at_unix_ms, snapshot_json, completion_json FROM benchmark_runs LIMIT 0",
    "SELECT query_key, result_json, fetched_at_unix_ms FROM catalog_queries LIMIT 0",
    "SELECT repo_id, revision, release_json, fetched_at_unix_ms FROM catalog_releases LIMIT 0",
    "SELECT repo_id, revision, filename, gguf_json, fetched_at_unix_ms FROM artifact_metadata LIMIT 0",
    "SELECT path, dev, inode, size, mtime_ns, ctime_ns, sha256, verified_at_unix_ms FROM local_file_digests LIMIT 0",
    "SELECT id, artifact_id, repo_id, revision, filename, expected_sha256, expected_size, state, bytes_done, failure_code, created_at_unix_ms, updated_at_unix_ms FROM downloads LIMIT 0",
];

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
        tx.execute_batch(STEP_0_TO_1)
            .map_err(|_| "Cannot create benchmark history schema.")?;
        tx.execute_batch(STEP_1_TO_2)
            .map_err(|_| "Cannot create model catalog schema.")?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|_| "Cannot set history identity.")?;
        tx.pragma_update(None, "user_version", VERSION)
            .map_err(|_| "Cannot set history version.")?;
    } else if app == APPLICATION_ID && version == 1 {
        tx.execute_batch(STEP_1_TO_2)
            .map_err(|_| "Cannot upgrade history to the model catalog schema.")?;
        tx.pragma_update(None, "user_version", VERSION)
            .map_err(|_| "Cannot set history version.")?;
    } else if app != APPLICATION_ID || version != VERSION {
        return Err("Unsupported or unrelated history database. Keep the file and use a compatible application version.".into());
    }
    // Every table of the current version must have its expected columns.
    for check in SHAPE_CHECKS {
        tx.prepare(check)
            .map_err(|_| "History schema is incompatible. Keep the file for recovery.")?;
    }
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
