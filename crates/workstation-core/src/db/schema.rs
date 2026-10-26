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

/// Expected `name:type:notnull:pk-position` for every column of every table. CHECK
/// size caps are not verified here; they are enforced when the tables are created.
const SHAPES: &[(&str, &str)] = &[
    (
        "benchmark_runs",
        "id:INTEGER:0:1,started_at_unix_ms:INTEGER:1:0,snapshot_json:TEXT:1:0,completion_json:TEXT:0:0",
    ),
    (
        "catalog_queries",
        "query_key:TEXT:1:1,result_json:TEXT:1:0,fetched_at_unix_ms:INTEGER:1:0",
    ),
    (
        "catalog_releases",
        "repo_id:TEXT:1:1,revision:TEXT:1:2,release_json:TEXT:1:0,fetched_at_unix_ms:INTEGER:1:0",
    ),
    (
        "artifact_metadata",
        "repo_id:TEXT:1:1,revision:TEXT:1:2,filename:TEXT:1:3,gguf_json:TEXT:1:0,fetched_at_unix_ms:INTEGER:1:0",
    ),
    (
        "local_file_digests",
        "path:TEXT:1:1,dev:INTEGER:1:0,inode:INTEGER:1:0,size:INTEGER:1:0,mtime_ns:INTEGER:1:0,ctime_ns:INTEGER:1:0,sha256:TEXT:1:0,verified_at_unix_ms:INTEGER:1:0",
    ),
    (
        "downloads",
        "id:INTEGER:0:1,artifact_id:TEXT:1:0,repo_id:TEXT:1:0,revision:TEXT:1:0,filename:TEXT:1:0,expected_sha256:TEXT:1:0,expected_size:INTEGER:1:0,state:TEXT:1:0,bytes_done:INTEGER:1:0,failure_code:TEXT:0:0,created_at_unix_ms:INTEGER:1:0,updated_at_unix_ms:INTEGER:1:0",
    ),
];

/// A table must be STRICT and have exactly the expected columns, types, and keys.
fn table_matches(tx: &rusqlite::Transaction, table: &str, expected: &str) -> bool {
    let strict: Option<i64> = tx
        .query_row("SELECT strict FROM pragma_table_list(?1)", [table], |r| {
            r.get(0)
        })
        .ok();
    let columns: Result<Vec<String>, _> = tx
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")
        .and_then(|mut statement| {
            statement
                .query_map([table], |r| {
                    Ok(format!(
                        "{}:{}:{}:{}",
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?
                    ))
                })?
                .collect()
        });
    strict == Some(1) && columns.is_ok_and(|c| c.join(",") == expected)
}

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
    // Every table of the current version must match exactly, or the file is kept untouched.
    for (table, expected) in SHAPES {
        if !table_matches(&tx, table, expected) {
            return Err("History schema is incompatible. Keep the file for recovery.".into());
        }
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
