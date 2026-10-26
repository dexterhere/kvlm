// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use workstation_core::{
    db::{
        HistoryStore,
        schema::{APPLICATION_ID, VERSION},
    },
    models::{
        catalog::{CatalogPage, RawRepoDetail, RawSibling, RepoSummary, normalize_release},
        release::{GatedAccess, ModelRelease},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);

const REV: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA: &str = "abababababababababababababababababababababababababababababababab";
const OTHER_SHA: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

struct TempDb(PathBuf);
impl TempDb {
    fn new() -> Self {
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(PathBuf::from(format!(
            "/tmp/kvlm-catalog-{stamp}-{nonce}/history.db"
        )))
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn page(repo: &str, cursor: Option<&str>) -> CatalogPage {
    CatalogPage {
        repos: vec![RepoSummary {
            repo_id: repo.into(),
            downloads: Some(5),
            likes: None,
            gated: GatedAccess::Open,
        }],
        next_cursor: cursor.map(str::to_owned),
    }
}

fn release_with(size: Option<u64>, sha: Option<&str>) -> ModelRelease {
    normalize_release(RawRepoDetail {
        repo_id: "o/n-GGUF".into(),
        revision: REV.into(),
        license: None,
        gated: GatedAccess::Open,
        base_model: None,
        siblings: vec![RawSibling {
            filename: "m-Q4_K_M.gguf".into(),
            size,
            lfs_sha256: sha.map(str::to_owned),
        }],
    })
    .unwrap()
}

fn release() -> ModelRelease {
    release_with(Some(10), Some(SHA))
}

fn pragma_i64(connection: &Connection, name: &str) -> i64 {
    connection
        .pragma_query_value(None, name, |row| row.get(0))
        .unwrap()
}

#[test]
fn fresh_database_has_every_v2_table_and_version() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    drop(store);

    let connection = Connection::open(&path.0).unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), VERSION);
    assert_eq!(pragma_i64(&connection, "application_id"), APPLICATION_ID);

    let mut statement = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table'")
        .unwrap();
    let tables: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in [
        "benchmark_runs",
        "catalog_queries",
        "catalog_releases",
        "artifact_metadata",
        "local_file_digests",
        "downloads",
    ] {
        assert!(tables.iter().any(|name| name == table), "missing {table}");
    }
}

#[test]
fn v1_database_upgrades_in_place_and_keeps_runs() {
    let path = TempDb::new();
    let dir = path.0.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();

    {
        let connection = Connection::open(&path.0).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=DELETE;
                 CREATE TABLE benchmark_runs (
                    id INTEGER PRIMARY KEY,
                    started_at_unix_ms INTEGER NOT NULL,
                    snapshot_json TEXT NOT NULL,
                    completion_json TEXT
                 ) STRICT;
                 INSERT INTO benchmark_runs(started_at_unix_ms, snapshot_json, completion_json)
                    VALUES (1, '{}', NULL);",
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
    }
    fs::set_permissions(&path.0, fs::Permissions::from_mode(0o600)).unwrap();

    let store = HistoryStore::open(&path.0).unwrap();
    drop(store);

    let connection = Connection::open(&path.0).unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), VERSION);
    let count: i64 = connection
        .query_row("SELECT count(*) FROM benchmark_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let snapshot: String = connection
        .query_row("SELECT snapshot_json FROM benchmark_runs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(snapshot.as_bytes(), b"{}");
    let releases: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='catalog_releases'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(releases, 1);
    drop(connection);

    let again = HistoryStore::open(&path.0).unwrap();
    drop(again);
    let connection = Connection::open(&path.0).unwrap();
    assert_eq!(pragma_i64(&connection, "user_version"), VERSION);
}

#[test]
fn future_version_is_refused_unchanged() {
    let path = TempDb::new();
    let dir = path.0.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    {
        let connection = Connection::open(&path.0).unwrap();
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", VERSION + 1)
            .unwrap();
    }
    fs::set_permissions(&path.0, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(&path.0).unwrap();

    assert!(HistoryStore::open(&path.0).is_err());

    assert_eq!(fs::read(&path.0).unwrap(), before);
}

#[test]
fn damaged_v2_missing_table_is_refused() {
    let path = TempDb::new();
    drop(HistoryStore::open(&path.0).unwrap());

    {
        let connection = Connection::open(&path.0).unwrap();
        connection.execute_batch("DROP TABLE downloads;").unwrap();
    }

    assert!(HistoryStore::open(&path.0).is_err());
}

#[test]
fn page_cache_round_trips_and_replaces_same_key() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let first = page("a/b", Some("c"));

    assert_eq!(store.cached_catalog_page("q").unwrap(), None);
    store.cache_catalog_page("q", &first, 100).unwrap();
    assert_eq!(
        store.cached_catalog_page("q").unwrap(),
        Some((first.clone(), 100))
    );

    let second = page("x/y", None);
    store.cache_catalog_page("q", &second, 200).unwrap();
    assert_eq!(store.cached_catalog_page("q").unwrap(), Some((second, 200)));
    assert_eq!(store.cached_catalog_page("missing").unwrap(), None);
}

#[test]
fn page_cache_evicts_oldest_beyond_200_rows() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let body = page("a/b", None);

    for i in 0..205u64 {
        store
            .cache_catalog_page(&format!("k{i:03}"), &body, i + 1)
            .unwrap();
    }

    for i in 0..5 {
        assert_eq!(
            store.cached_catalog_page(&format!("k{i:03}")).unwrap(),
            None
        );
    }
    assert!(store.cached_catalog_page("k005").unwrap().is_some());
    assert!(store.cached_catalog_page("k204").unwrap().is_some());
}

#[test]
fn release_cache_round_trips() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let original = release();

    store.cache_release(&original, 42).unwrap();
    assert_eq!(
        store.cached_release("o/n-GGUF", REV).unwrap(),
        Some((original, 42))
    );
}

#[test]
fn release_cache_refuses_changed_checksum_or_size_for_same_revision() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let original = release();
    store.cache_release(&original, 1).unwrap();

    let changed_size = release_with(Some(11), Some(SHA));
    assert!(store.cache_release(&changed_size, 2).is_err());

    let changed_sha = release_with(Some(10), Some(OTHER_SHA));
    assert!(store.cache_release(&changed_sha, 3).is_err());

    assert_eq!(
        store.cached_release("o/n-GGUF", REV).unwrap(),
        Some((original.clone(), 1))
    );

    store.cache_release(&original, 4).unwrap();
    assert_eq!(
        store.cached_release("o/n-GGUF", REV).unwrap(),
        Some((original, 4))
    );
}

#[test]
fn release_cache_accepts_filling_in_an_unknown_size() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    store.cache_release(&release_with(None, None), 1).unwrap();

    let filled = release_with(Some(10), Some(SHA));
    store.cache_release(&filled, 2).unwrap();
    assert_eq!(
        store.cached_release("o/n-GGUF", REV).unwrap(),
        Some((filled, 2))
    );
}

#[test]
fn invalid_release_identity_is_refused() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let mut bad = release();
    bad.repo_id = "bad".into();

    assert!(store.cache_release(&bad, 1).is_err());
}

#[test]
fn oversized_page_key_is_refused() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let body = page("a/b", None);

    assert!(
        store
            .cache_catalog_page(&"k".repeat(513), &body, 1)
            .is_err()
    );
    assert!(store.cache_catalog_page("", &body, 1).is_err());
}
