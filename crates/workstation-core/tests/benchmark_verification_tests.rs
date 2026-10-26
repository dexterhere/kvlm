// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use workstation_core::{
    db::{
        HistoryStore, PartialMetrics, RunCompletion, RunMetrics, RunOutcome, RunSnapshot,
        WorkloadSnapshot,
    },
    hardware::{
        ComputeBackend, CpuFeatures, CpuProfile, GpuDeviceProfile, GpuVendor, HardwareProfile,
        MemoryPool,
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const HEX: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

struct TempDb(PathBuf);
impl TempDb {
    fn new() -> Self {
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(PathBuf::from(format!(
            "/tmp/kvlm-verification-{stamp}-{nonce}/history.db"
        )))
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn hardware() -> HardwareProfile {
    let mut profile = HardwareProfile {
        profile_hash: None,
        cpu: Some(CpuProfile {
            model: Some("Verification Test CPU".into()),
            physical_cores: Some(8),
            logical_cores: 16,
            features: Some(CpuFeatures {
                avx2: true,
                avx512: false,
                vnni: false,
                amx: false,
            }),
        }),
        system_ram: Some(MemoryPool {
            total_bytes: 64 * 1024 * 1024 * 1024,
            available_bytes: Some(48 * 1024 * 1024 * 1024),
        }),
        gpus: vec![GpuDeviceProfile {
            id: Some("pci:0000:01:00.0".into()),
            name: "Verification Test GPU".into(),
            vendor: GpuVendor::Nvidia,
            dedicated_vram: Some(MemoryPool {
                total_bytes: 12 * 1024 * 1024 * 1024,
                available_bytes: Some(10 * 1024 * 1024 * 1024),
            }),
            shared_gtt: None,
            supported_backends: vec![ComputeBackend::Cuda {
                compute_capability: (8, 6),
                driver_version: "test-driver".into(),
            }],
            unified_memory: Some(false),
        }],
        issues: vec![],
        captured_at_unix_ms: 1,
    };
    refresh_hash(&mut profile);
    profile
}

fn refresh_hash(profile: &mut HardwareProfile) {
    profile.profile_hash = profile.identity_hash();
}

fn digest() -> String {
    format!("sha256:{HEX}")
}

fn snapshot(model_digest: Option<String>, profile: Option<HardwareProfile>) -> RunSnapshot {
    RunSnapshot {
        started_at_unix_ms: 10,
        model_name: "verified model".into(),
        model_digest,
        model_size_bytes: Some(1),
        hardware: profile,
        hardware_notice: None,
        runtime_endpoint: "http://127.0.0.1:11434".into(),
        workload: WorkloadSnapshot {
            id: "quick".into(),
            prompt_sha256: "0".repeat(64),
            num_predict: 4,
            num_ctx: 32,
            seed: 1,
            temperature: 0,
            think: false,
        },
    }
}

fn metrics() -> RunMetrics {
    RunMetrics {
        first_response_ns: Some(2),
        elapsed_ns: 10,
        response_chunks: 3,
        eval_count: Some(4),
        eval_duration_ns: Some(5),
        load_duration_ns: None,
        done_reason: "stop".into(),
    }
}

fn completion(model_digest: Option<String>) -> RunCompletion {
    RunCompletion {
        finished_at_unix_ms: 20,
        runtime_version: Some("ollama-test".into()),
        model_digest,
        outcome: RunOutcome::Completed(metrics()),
        resources: None,
    }
}

fn record(
    store: &HistoryStore,
    model_digest: Option<String>,
    profile: Option<HardwareProfile>,
    result: RunCompletion,
) -> i64 {
    let id = store
        .start(&snapshot(model_digest, profile))
        .expect("start record");
    store.finish(id, &result).expect("finish record");
    id
}

fn read_json(path: &std::path::Path, id: i64, column: &str) -> String {
    assert!(matches!(column, "snapshot_json" | "completion_json"));
    Connection::open(path)
        .unwrap()
        .query_row(
            &format!("SELECT {column} FROM benchmark_runs WHERE id=?1"),
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

fn write_json(path: &std::path::Path, id: i64, column: &str, json: &str) {
    assert!(matches!(column, "snapshot_json" | "completion_json"));
    let connection = Connection::open(path).unwrap();
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    connection
        .execute(
            &format!("UPDATE benchmark_runs SET {column}=?1 WHERE id=?2"),
            rusqlite::params![json, id],
        )
        .unwrap();
}

#[test]
fn completed_record_verifies_after_restart_with_canonical_digest_variants() {
    let path = TempDb::new();
    let current = hardware();
    let stored = format!("sha256:{}", HEX.to_ascii_uppercase());
    let store = HistoryStore::open(&path.0).unwrap();
    let id = record(
        &store,
        Some(stored.clone()),
        Some(current.clone()),
        completion(Some(stored)),
    );
    drop(store);

    let reopened = HistoryStore::open(&path.0).unwrap();
    assert_eq!(
        reopened.verified_run(&HEX.to_ascii_uppercase(), &current),
        Ok(Some(id))
    );
    assert_eq!(reopened.verified_run(&digest(), &current), Ok(Some(id)));
    assert_eq!(
        current.profile_hash.as_deref().unwrap().split(':').next(),
        Some("hardware-v2")
    );
}

#[test]
fn exact_identity_search_is_not_limited_to_the_recent_hundred_rows() {
    let path = TempDb::new();
    let current = hardware();
    let store = HistoryStore::open(&path.0).unwrap();
    let verified = record(
        &store,
        Some(digest()),
        Some(current.clone()),
        completion(Some(digest())),
    );
    for index in 0..125 {
        let other = format!("sha256:{:064x}", index + 1);
        record(
            &store,
            Some(other.clone()),
            Some(current.clone()),
            completion(Some(other)),
        );
    }
    assert_eq!(store.recent().unwrap().len(), 100);
    assert_eq!(store.verified_run(&digest(), &current), Ok(Some(verified)));
}

#[test]
fn cancelled_timed_out_failed_and_unfinished_records_never_verify() {
    let path = TempDb::new();
    let current = hardware();
    let store = HistoryStore::open(&path.0).unwrap();
    for outcome in [
        RunOutcome::Cancelled(PartialMetrics {
            first_response_ns: Some(2),
            response_chunks: 3,
        }),
        RunOutcome::TimedOut(PartialMetrics {
            first_response_ns: Some(2),
            response_chunks: 3,
        }),
        RunOutcome::Failed {
            message: "test failure".into(),
            partial: PartialMetrics {
                first_response_ns: Some(2),
                response_chunks: 3,
            },
        },
    ] {
        let mut finished = completion(Some(digest()));
        finished.outcome = outcome;
        record(&store, Some(digest()), Some(current.clone()), finished);
    }
    store
        .start(&snapshot(Some(digest()), Some(current.clone())))
        .unwrap();
    assert_eq!(store.verified_run(&digest(), &current).unwrap(), None);
}

#[test]
fn incomplete_completed_metrics_and_invalid_stop_reasons_never_verify() {
    let path = TempDb::new();
    let current = hardware();
    let store = HistoryStore::open(&path.0).unwrap();
    let mut cases = vec![metrics(); 7];
    cases[0].response_chunks = 0;
    cases[1].elapsed_ns = 0;
    cases[2].first_response_ns = None;
    cases[3].first_response_ns = Some(cases[3].elapsed_ns + 1);
    cases[4].eval_count = Some(0);
    cases[5].eval_duration_ns = Some(0);
    cases[6].done_reason = "other".into();
    for run_metrics in cases {
        let mut finished = completion(Some(digest()));
        finished.outcome = RunOutcome::Completed(run_metrics);
        record(&store, Some(digest()), Some(current.clone()), finished);
    }
    assert_eq!(store.verified_run(&digest(), &current).unwrap(), None);

    let mut length_reason = completion(Some(digest()));
    if let RunOutcome::Completed(metrics) = &mut length_reason.outcome {
        metrics.done_reason = "length".into();
    }
    let length_id = record(&store, Some(digest()), Some(current.clone()), length_reason);
    assert_eq!(
        store.verified_run(&digest(), &current).unwrap(),
        Some(length_id)
    );
}

#[test]
fn snapshot_and_completion_digests_must_both_match() {
    let path = TempDb::new();
    let current = hardware();
    let store = HistoryStore::open(&path.0).unwrap();
    record(
        &store,
        Some(digest()),
        Some(current.clone()),
        completion(Some(format!("sha256:{}", "1".repeat(64)))),
    );
    record(
        &store,
        Some(format!("sha256:{}", "1".repeat(64))),
        Some(current.clone()),
        completion(Some(digest())),
    );
    assert_eq!(store.verified_run(&digest(), &current).unwrap(), None);
}

#[test]
fn missing_legacy_or_inconsistent_hardware_identity_never_verifies() {
    let path = TempDb::new();
    let current = hardware();
    let store = HistoryStore::open(&path.0).unwrap();

    let mut missing = current.clone();
    missing.profile_hash = None;
    record(
        &store,
        Some(digest()),
        Some(missing),
        completion(Some(digest())),
    );

    let mut legacy = current.clone();
    legacy.profile_hash = Some(HEX.into());
    record(
        &store,
        Some(digest()),
        Some(legacy),
        completion(Some(digest())),
    );

    let mut inconsistent = current.clone();
    inconsistent.gpus[0].name.push_str(" changed");
    inconsistent.profile_hash = current.profile_hash.clone();
    record(
        &store,
        Some(digest()),
        Some(inconsistent),
        completion(Some(digest())),
    );

    assert_eq!(store.verified_run(&digest(), &current).unwrap(), None);
    assert_eq!(store.recent().unwrap().len(), 3);
}

#[test]
fn changed_gpu_capacity_name_or_cpu_identity_does_not_match() {
    let path = TempDb::new();
    let original = hardware();
    let store = HistoryStore::open(&path.0).unwrap();
    record(
        &store,
        Some(digest()),
        Some(original.clone()),
        completion(Some(digest())),
    );

    let mut changed_capacity = original.clone();
    changed_capacity.gpus[0]
        .dedicated_vram
        .as_mut()
        .unwrap()
        .total_bytes += 1;
    refresh_hash(&mut changed_capacity);
    let mut changed_name = original.clone();
    changed_name.gpus[0].name.push_str(" revised");
    refresh_hash(&mut changed_name);
    let mut changed_cpu = original.clone();
    changed_cpu.cpu.as_mut().unwrap().model = Some("Different CPU".into());
    refresh_hash(&mut changed_cpu);
    for changed in [&changed_capacity, &changed_name, &changed_cpu] {
        assert_eq!(store.verified_run(&digest(), changed).unwrap(), None);
    }
}

#[test]
fn malformed_typed_or_oversized_matching_rows_fail_closed_without_deleting_data() {
    for corruption in ["invalid-digest-type", "malformed-json", "oversized"] {
        let path = TempDb::new();
        let current = hardware();
        let store = HistoryStore::open(&path.0).unwrap();
        let id = record(
            &store,
            Some(digest()),
            Some(current.clone()),
            completion(Some(digest())),
        );
        drop(store);

        let original = read_json(&path.0, id, "completion_json");
        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        let changed = match corruption {
            "invalid-digest-type" => {
                value["model_digest"] = serde_json::Value::from(7);
                serde_json::to_string(&value).unwrap()
            }
            "malformed-json" => "{".into(),
            _ => {
                value["padding"] = serde_json::Value::String("x".repeat(262_145));
                serde_json::to_string(&value).unwrap()
            }
        };
        write_json(&path.0, id, "completion_json", &changed);

        let store = HistoryStore::open(&path.0).unwrap();
        assert!(store.verified_run(&digest(), &current).is_err());
        drop(store);
        assert_eq!(read_json(&path.0, id, "completion_json"), changed);
        let count: i64 = Connection::open(&path.0)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM benchmark_runs WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
