// SPDX-License-Identifier: GPL-3.0-or-later

use rusqlite::Connection;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use workstation_core::{
    db::{
        HistoryStore,
        schema::{APPLICATION_ID, VERSION},
    },
    experiments::{
        Engine, Finding, FindingStatus, NewExperiment, Ownership, PassKind, PassMetrics, Settings,
        State, Variable, summarize,
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kvlm-lab-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("w.db")
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn spec() -> NewExperiment {
    NewExperiment {
        engine: Engine::Llamacpp,
        ownership: Ownership::Managed,
        cold: true,
        passes_requested: 3,
        workload_id: "fixed-32".into(),
        prompt_sha256: "ab".repeat(32),
        seed: 42,
        max_tokens: 64,
        model_name: "tiny".into(),
        model_sha256: Some("cd".repeat(32)),
        runtime_version: Some("b11910".into()),
        backend: Some("cpu".into()),
        hardware_hash: Some("hw1".into()),
        requested: Settings {
            context_length: Some(4096),
            gpu_layers: Some(99),
            ..Settings::default()
        },
        variable: None,
        baseline_id: None,
    }
}

fn metrics(decode: f64) -> PassMetrics {
    PassMetrics {
        wall_ms: 900,
        ttft_ms: Some(80.0),
        decode_tokens_per_s: Some(decode),
        ..PassMetrics::default()
    }
}

fn finished(store: &HistoryStore, at: u64) -> i64 {
    let id = store.begin_experiment(&spec(), at).unwrap();
    store
        .finish_experiment(id, State::Completed, None, None, None, at + 1)
        .unwrap();
    id
}

#[test]
fn an_experiment_round_trips_with_passes_summary_and_findings() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    let id = store.begin_experiment(&spec(), 1_000).unwrap();
    for (kind, ordinal, decode) in [
        (PassKind::ColdLoad, 0, 0.0),
        (PassKind::Warmup, 0, 10.0),
        (PassKind::Measured, 0, 40.0),
        (PassKind::Measured, 1, 44.0),
    ] {
        let pass = store.begin_pass(id, kind, ordinal, 1_100).unwrap();
        store
            .finish_pass(pass, State::Completed, Some(&metrics(decode)), None, 1_200)
            .unwrap();
    }
    let failed = store.begin_pass(id, PassKind::Measured, 2, 1_300).unwrap();
    store
        .finish_pass(failed, State::Failed, None, Some("runtime stopped"), 1_400)
        .unwrap();

    let before = store.load_experiment(id).unwrap().unwrap();
    assert_eq!(before.state, State::Running);
    let summary = summarize(&before.passes);
    assert_eq!((summary.measured_ok, summary.excluded), (2, 1));
    let observed = Settings {
        context_length: Some(2048),
        ..Settings::default()
    };
    store
        .finish_experiment(
            id,
            State::Completed,
            Some(&observed),
            Some(&summary),
            None,
            2_000,
        )
        .unwrap();
    let finding = Finding {
        rule_id: "DIAG-03".into(),
        status: FindingStatus::NotEvaluated,
        warning: false,
        evidence: vec![],
        missing: vec!["prompt token count".into()],
        follow_up: None,
        message: "Not evaluated: the prompt token count is missing.".into(),
    };
    store
        .save_findings(id, std::slice::from_ref(&finding))
        .unwrap();

    let loaded = store.load_experiment(id).unwrap().unwrap();
    assert_eq!(loaded.state, State::Completed);
    assert_eq!(loaded.spec, spec());
    assert_eq!(loaded.observed.as_ref().unwrap().context_length, Some(2048));
    assert_eq!(loaded.summary.as_ref().unwrap().measured_ok, 2);
    assert_eq!(loaded.passes.len(), 5);
    assert_eq!(loaded.passes[4].failure.as_deref(), Some("runtime stopped"));
    assert_eq!(loaded.findings, vec![finding]);
    // Requested and observed stay apart.
    assert_eq!(
        loaded
            .spec
            .requested
            .differences(loaded.observed.as_ref().unwrap()),
        vec!["context_length"]
    );
}

#[test]
fn only_one_experiment_runs_at_a_time_and_results_are_final() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    let id = store.begin_experiment(&spec(), 1).unwrap();
    assert!(
        store
            .begin_experiment(&spec(), 2)
            .unwrap_err()
            .contains("still running")
    );
    let pass = store.begin_pass(id, PassKind::Warmup, 0, 3).unwrap();
    assert!(
        store.begin_pass(id, PassKind::Warmup, 0, 3).is_err(),
        "duplicate pass"
    );
    store
        .finish_pass(pass, State::Completed, None, None, 4)
        .unwrap();
    assert!(
        store
            .finish_pass(pass, State::Failed, None, None, 5)
            .is_err()
    );
    assert!(
        store
            .finish_experiment(id, State::Running, None, None, None, 5)
            .is_err()
    );
    store
        .finish_experiment(id, State::Cancelled, None, None, None, 6)
        .unwrap();
    assert!(
        store
            .finish_experiment(id, State::Completed, None, None, None, 7)
            .is_err()
    );
    assert!(
        store.begin_pass(id, PassKind::Measured, 0, 8).is_err(),
        "not running"
    );
    assert!(store.save_findings(999, &[]).is_err());
    store.begin_experiment(&spec(), 9).unwrap();
}

#[test]
fn a_run_left_running_is_marked_interrupted_and_stays_visible() {
    let dir = Dir::new();
    let first = {
        let store = HistoryStore::open(&dir.db()).unwrap();
        let id = store.begin_experiment(&spec(), 10).unwrap();
        store.begin_pass(id, PassKind::Warmup, 0, 11).unwrap();
        id
    };
    let store = HistoryStore::open(&dir.db()).unwrap();
    assert!(
        store.begin_experiment(&spec(), 20).is_err(),
        "stale run blocks until recovered"
    );
    assert_eq!(store.recover_interrupted(30).unwrap(), 1);
    let found = store.load_experiment(first).unwrap().unwrap();
    assert_eq!(found.state, State::Interrupted);
    assert_eq!(found.passes[0].state, State::Interrupted);
    assert_eq!(found.finished_at_unix_ms, Some(30));
    assert_eq!(store.recover_interrupted(40).unwrap(), 0);
    store.begin_experiment(&spec(), 50).unwrap();
}

#[test]
fn old_experiments_are_pruned_with_their_passes_and_a_baseline_link_is_emptied() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    let base = finished(&store, 1);
    let pass = store.begin_pass(
        {
            let id = store
                .begin_experiment(
                    &NewExperiment {
                        variable: Some(Variable::HalfContext),
                        baseline_id: Some(base),
                        ..spec()
                    },
                    5,
                )
                .unwrap();
            store
                .finish_experiment(id, State::Completed, None, None, None, 6)
                .unwrap();
            id
        },
        PassKind::Measured,
        0,
        7,
    );
    assert!(pass.is_err(), "finished experiments take no more passes");
    let follow_up = store.list_experiments(10).unwrap().rows[0].id;
    store.delete_experiment(base).unwrap();
    let row = &store.list_experiments(10).unwrap().rows[0];
    assert_eq!(
        (row.id, row.baseline_id),
        (follow_up, None),
        "link emptied, follow-up kept"
    );
    assert!(store.load_experiment(base).unwrap().is_none());

    for i in 0..205u64 {
        finished(&store, 100 + i * 10);
    }
    let listing = store.list_experiments(500).unwrap();
    assert_eq!(listing.rows.len(), 200);
    assert!(
        listing.rows.windows(2).all(|w| w[0].id > w[1].id),
        "newest first"
    );
    let connection = Connection::open(dir.db()).unwrap();
    let orphans: i64 = connection
        .query_row(
            "SELECT count(*) FROM experiment_passes WHERE experiment_id NOT IN (SELECT id FROM experiments)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
}

#[test]
fn deleting_removes_passes_and_findings_but_never_a_running_experiment() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    let id = store.begin_experiment(&spec(), 1).unwrap();
    assert!(store.delete_experiment(id).is_err());
    let pass = store.begin_pass(id, PassKind::Measured, 0, 2).unwrap();
    store
        .finish_pass(pass, State::Completed, Some(&metrics(1.0)), None, 3)
        .unwrap();
    store
        .finish_experiment(id, State::Completed, None, None, None, 4)
        .unwrap();
    store.delete_experiment(id).unwrap();
    let connection = Connection::open(dir.db()).unwrap();
    let left: i64 = connection
        .query_row("SELECT (SELECT count(*) FROM experiment_passes) + (SELECT count(*) FROM diagnostic_findings)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn a_v3_database_upgrades_and_keeps_history_and_profiles() {
    let dir = Dir::new();
    drop(HistoryStore::open(&dir.db()).unwrap());
    let connection = Connection::open(dir.db()).unwrap();
    connection
        .execute_batch(
            "INSERT INTO benchmark_runs(started_at_unix_ms,snapshot_json) VALUES (7,'{}');
             DROP TABLE diagnostic_findings; DROP TABLE experiment_passes; DROP TABLE experiments;",
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
    drop(connection);
    let store = HistoryStore::open(&dir.db()).unwrap();
    finished(&store, 1);
    drop(store);
    let connection = Connection::open(dir.db()).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    let app: i64 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap();
    let kept: i64 = connection
        .query_row("SELECT started_at_unix_ms FROM benchmark_runs", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!((version, app, kept), (VERSION, APPLICATION_ID, 7));
}

#[test]
fn a_damaged_row_is_counted_and_does_not_block_the_list() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    let good = finished(&store, 1);
    let bad = finished(&store, 10);
    drop(store);
    let connection = Connection::open(dir.db()).unwrap();
    connection
        .execute(
            "UPDATE experiments SET requested_json='not json' WHERE id=?1",
            [bad],
        )
        .unwrap();
    drop(connection);
    let store = HistoryStore::open(&dir.db()).unwrap();
    let listing = store.list_experiments(10).unwrap();
    assert_eq!((listing.rows.len(), listing.unreadable), (1, 1));
    assert_eq!(listing.rows[0].id, good);
    assert!(store.load_experiment(bad).is_err());
}

#[test]
fn schema_checks_reject_bad_values() {
    let dir = Dir::new();
    let store = HistoryStore::open(&dir.db()).unwrap();
    drop(store);
    let connection = Connection::open(dir.db()).unwrap();
    assert!(
        connection
            .execute(
                "INSERT INTO experiments(state,started_at_unix_ms,engine,ownership,cold,passes_requested,workload_id,prompt_sha256,seed,max_tokens,model_name,requested_json)
                 VALUES ('bogus',1,'llamacpp','managed',0,3,'w','a','1','1','m','{}')",
                [],
            )
            .is_err()
    );
}
