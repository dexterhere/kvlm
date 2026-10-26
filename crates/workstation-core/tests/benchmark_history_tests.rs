// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, TransactionBehavior};
use workstation_core::db::{
    HistoryStore, PartialMetrics, RunCompletion, RunMetrics, RunOutcome, RunSnapshot,
    WorkloadSnapshot,
    schema::{APPLICATION_ID, VERSION},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempDb(PathBuf);
impl TempDb {
    fn new() -> Self {
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!("/tmp/kvlm-history-{stamp}-{nonce}/history.db"));
        Self(path)
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn snapshot(name: &str) -> RunSnapshot {
    RunSnapshot {
        started_at_unix_ms: 1,
        model_name: name.into(),
        model_digest: Some("digest".into()),
        model_size_bytes: Some(u64::MAX),
        hardware: None,
        hardware_notice: Some("unknown".into()),
        runtime_endpoint: "http://127.0.0.1:11434".into(),
        workload: WorkloadSnapshot {
            id: "quick".into(),
            prompt_sha256: "abc".into(),
            num_predict: 1,
            num_ctx: 2,
            seed: 3,
            temperature: 0,
            think: false,
        },
    }
}
fn completed() -> RunCompletion {
    RunCompletion {
        finished_at_unix_ms: 2,
        resources: None,
        runtime_version: Some("test".into()),
        model_digest: Some("digest".into()),
        outcome: RunOutcome::Completed(RunMetrics {
            first_response_ns: Some(u64::MAX),
            elapsed_ns: u64::MAX,
            response_chunks: u64::MAX,
            eval_count: Some(u64::MAX),
            eval_duration_ns: Some(u64::MAX),
            load_duration_ns: Some(u64::MAX),
            done_reason: "stop".into(),
        }),
    }
}

#[test]
fn restart_preserves_completed_cancelled_timed_out_failed_and_unfinished_records() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let complete = store.start(&snapshot("complete")).unwrap();
    store.finish(complete, &completed()).unwrap();
    for (name, outcome) in [
        (
            "cancelled",
            RunOutcome::Cancelled(PartialMetrics {
                first_response_ns: None,
                response_chunks: 2,
            }),
        ),
        (
            "timed",
            RunOutcome::TimedOut(PartialMetrics {
                first_response_ns: Some(3),
                response_chunks: 4,
            }),
        ),
        (
            "failed",
            RunOutcome::Failed {
                message: "safe message".into(),
                partial: PartialMetrics {
                    first_response_ns: None,
                    response_chunks: 0,
                },
            },
        ),
    ] {
        let id = store.start(&snapshot(name)).unwrap();
        store
            .finish(
                id,
                &RunCompletion {
                    finished_at_unix_ms: 2,
                    resources: None,
                    runtime_version: None,
                    model_digest: None,
                    outcome,
                },
            )
            .unwrap();
    }
    store.start(&snapshot("unfinished")).unwrap();
    drop(store);
    let entries = HistoryStore::open(&path.0).unwrap().recent().unwrap();
    assert_eq!(entries.len(), 5);
    assert!(
        entries
            .iter()
            .any(|entry| entry.snapshot.model_name == "unfinished" && entry.completion.is_none())
    );
    let complete = entries
        .iter()
        .find(|entry| entry.snapshot.model_name == "complete")
        .unwrap();
    match &complete.completion.as_ref().unwrap().outcome {
        RunOutcome::Completed(metrics) => assert_eq!(metrics.elapsed_ns, u64::MAX),
        _ => panic!("wrong outcome"),
    }
}

#[test]
fn completion_is_immutable_and_identical_retry_is_safe() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let original = snapshot("name '); DROP TABLE benchmark_runs; --");
    let id = store.start(&original).unwrap();
    let finish = completed();
    store.finish(id, &finish).unwrap();
    store.finish(id, &finish).unwrap();
    let different = RunCompletion {
        finished_at_unix_ms: 3,
        ..completed()
    };
    assert!(store.finish(id, &different).is_err());
    let entries = store.recent().unwrap();
    assert_eq!(entries[0].snapshot.model_name, original.model_name);
    assert_eq!(
        entries[0].completion.as_ref().unwrap().finished_at_unix_ms,
        2
    );
}

#[test]
fn recent_is_newest_first_and_limited_to_one_hundred() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    for number in 0..101 {
        store.start(&snapshot(&number.to_string())).unwrap();
    }
    let entries = store.recent().unwrap();
    assert_eq!(entries.len(), 100);
    assert_eq!(entries[0].snapshot.model_name, "100");
    assert_eq!(entries[99].snapshot.model_name, "1");
}

#[test]
fn incompatible_or_corrupt_existing_database_is_preserved() {
    let path = TempDb::new();
    fs::create_dir_all(path.0.parent().unwrap()).unwrap();
    fs::write(&path.0, b"not sqlite").unwrap();
    fs::set_permissions(&path.0, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(&path.0).unwrap();
    assert!(HistoryStore::open(&path.0).is_err());
    assert_eq!(fs::read(&path.0).unwrap(), before);
}

#[test]
fn unknown_identity_version_and_unversioned_data_are_rejected_without_changes() {
    for (application_id, version) in [(1, VERSION), (APPLICATION_ID, VERSION + 1), (0, 0)] {
        let path = TempDb::new();
        fs::create_dir_all(path.0.parent().unwrap()).unwrap();
        let connection = Connection::open(&path.0).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE marker(value TEXT);")
            .unwrap();
        connection
            .execute("INSERT INTO marker VALUES ('keep me')", [])
            .unwrap();
        connection
            .pragma_update(None, "application_id", application_id)
            .unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        drop(connection);
        fs::set_permissions(&path.0, fs::Permissions::from_mode(0o600)).unwrap();

        assert!(HistoryStore::open(&path.0).is_err());

        let connection = Connection::open(&path.0).unwrap();
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        let value: String = connection
            .query_row("SELECT value FROM marker", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "delete");
        assert_eq!(value, "keep me");
    }
}

#[test]
fn known_schema_with_missing_columns_is_rejected() {
    let path = TempDb::new();
    fs::create_dir_all(path.0.parent().unwrap()).unwrap();
    let connection = Connection::open(&path.0).unwrap();
    connection
        .execute("CREATE TABLE benchmark_runs(id INTEGER PRIMARY KEY)", [])
        .unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    connection
        .pragma_update(None, "user_version", VERSION)
        .unwrap();
    drop(connection);
    fs::set_permissions(&path.0, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(HistoryStore::open(&path.0).is_err());
}

#[test]
fn writer_lock_times_out_quickly_and_open_succeeds_after_release() {
    let path = TempDb::new();
    drop(HistoryStore::open(&path.0).unwrap());
    let mut connection = Connection::open(&path.0).unwrap();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();

    let started = Instant::now();
    assert!(HistoryStore::open(&path.0).is_err());
    assert!(started.elapsed() < Duration::from_secs(3));

    drop(transaction);
    drop(connection);
    let store = HistoryStore::open(&path.0).unwrap();
    store.start(&snapshot("after lock")).unwrap();
}

#[test]
fn invalid_snapshot_json_errors_without_removing_rows() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let invalid = store.start(&snapshot("invalid")).unwrap();
    store.start(&snapshot("valid")).unwrap();
    drop(store);
    let connection = Connection::open(&path.0).unwrap();
    connection
        .execute(
            "UPDATE benchmark_runs SET snapshot_json='{' WHERE id=?1",
            [invalid],
        )
        .unwrap();
    drop(connection);

    let store = HistoryStore::open(&path.0).unwrap();
    assert!(store.recent().is_err());
    drop(store);
    let connection = Connection::open(&path.0).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM benchmark_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn storage_permissions_are_private_and_leaf_symlinks_are_rejected() {
    let path = TempDb::new();
    drop(HistoryStore::open(&path.0).unwrap());
    assert_eq!(
        fs::metadata(path.0.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path.0).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let before = fs::read(&path.0).unwrap();
    fs::set_permissions(&path.0, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(HistoryStore::open(&path.0).is_err());
    assert_eq!(fs::read(&path.0).unwrap(), before);
    assert_eq!(
        fs::metadata(&path.0).unwrap().permissions().mode() & 0o777,
        0o644
    );
    fs::set_permissions(&path.0, fs::Permissions::from_mode(0o600)).unwrap();
    drop(HistoryStore::open(&path.0).unwrap());

    let parent = path.0.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o751)).unwrap();
    drop(HistoryStore::open(&parent.join("second.db")).unwrap());
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o751
    );

    let target = parent.join("target.db");
    let link = parent.join("link.db");
    fs::write(&target, b"unchanged").unwrap();
    symlink(&target, &link).unwrap();
    assert!(HistoryStore::open(&link).is_err());
    assert_eq!(fs::read(target).unwrap(), b"unchanged");
}

#[test]
fn oversized_records_and_shared_writable_directories_are_rejected() {
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    assert!(store.start(&snapshot(&"x".repeat(262_144))).is_err());
    assert!(store.recent().unwrap().is_empty());
    drop(store);
    let parent = path.0.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(HistoryStore::open(&path.0).is_err());
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn legacy_completion_bytes_stay_unchanged_and_new_resources_survive_restart() {
    use workstation_core::hardware::resources::{GpuMemoryPeak, ResourceSummary};
    let path = TempDb::new();
    let store = HistoryStore::open(&path.0).unwrap();
    let legacy_id = store.start(&snapshot("legacy")).unwrap();
    let measured_id = store.start(&snapshot("measured")).unwrap();
    drop(store);
    let legacy = r#"{"finished_at_unix_ms":2,"runtime_version":"old","model_digest":null,"outcome":{"Cancelled":{"first_response_ns":null,"response_chunks":0}}}"#;
    let connection = Connection::open(&path.0).unwrap();
    connection
        .execute(
            "UPDATE benchmark_runs SET completion_json=?1 WHERE id=?2",
            rusqlite::params![legacy, legacy_id],
        )
        .unwrap();
    drop(connection);
    let store = HistoryStore::open(&path.0).unwrap();
    let entries = store.recent().unwrap();
    assert!(
        entries
            .iter()
            .find(|r| r.id == legacy_id)
            .unwrap()
            .completion
            .as_ref()
            .unwrap()
            .resources
            .is_none()
    );
    let summary = ResourceSummary {
        interval_ms: 100,
        window_ms: 1000,
        samples: 9,
        system_ram_samples: 7,
        peak_system_ram_nonavailable_bytes: Some(u64::MAX),
        gpus: vec![GpuMemoryPeak {
            id: "pci:0000:03:00.0".into(),
            name: "fixture".into(),
            vram_samples: 3,
            peak_vram_used_bytes: Some(0),
            gtt_samples: 0,
            peak_gtt_used_bytes: None,
        }],
        probe_failed: false,
        probe_in_flight: true,
        limit_reached: false,
    };
    let mut outcome = completed();
    outcome.resources = Some(summary.clone());
    store.finish(measured_id, &outcome).unwrap();
    drop(store);
    let store = HistoryStore::open(&path.0).unwrap();
    let entries = store.recent().unwrap();
    assert_eq!(
        entries[0].completion.as_ref().unwrap().resources,
        Some(summary)
    );
    drop(store);
    let connection = Connection::open(&path.0).unwrap();
    let preserved: String = connection
        .query_row(
            "SELECT completion_json FROM benchmark_runs WHERE id=?1",
            [legacy_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(preserved, legacy);
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, VERSION,
        "additive JSON metadata needs no structural database migration"
    );
}
