// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use workstation_core::db::{
    HistoryStore, PartialMetrics, RunCompletion, RunMetrics, RunOutcome, RunSnapshot,
    WorkloadSnapshot,
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
    let before = fs::read(&path.0).unwrap();
    assert!(HistoryStore::open(&path.0).is_err());
    assert_eq!(fs::read(&path.0).unwrap(), before);
}
