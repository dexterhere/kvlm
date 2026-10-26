use super::*;
use std::{collections::VecDeque, fs, path::PathBuf, sync::atomic::AtomicU64};
use workstation_core::experiments::{Engine, Ownership, PassKind};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempDb(PathBuf);
impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "kvlm-lab-run-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.join("w.db"))
    }
    fn db(&self) -> Db {
        Db::new(HistoryStore::open(&self.0).unwrap())
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn hardware() -> HardwareProfile {
    HardwareProfile {
        profile_hash: Some("hw".into()),
        cpu: None,
        system_ram: None,
        gpus: vec![],
        issues: vec![],
        captured_at_unix_ms: 1,
    }
}

fn spec(cold: bool, passes: u32) -> NewExperiment {
    let w = Workload::standard();
    NewExperiment {
        engine: Engine::Llamacpp,
        ownership: Ownership::Managed,
        cold,
        passes_requested: passes,
        workload_id: w.id().into(),
        prompt_sha256: w.prompt_sha256().into(),
        seed: w.seed(),
        max_tokens: w.max_tokens(),
        model_name: "tiny".into(),
        model_sha256: None,
        runtime_version: None,
        backend: Some("cpu".into()),
        hardware_hash: Some("hw".into()),
        requested: Settings::default(),
        variable: None,
        baseline_id: None,
    }
}

fn output(tokens_per_s: f64) -> PassOutput {
    let mut stream = StreamRecord {
        reported_tokens: Some(12),
        ..StreamRecord::default()
    };
    for i in 0..12u64 {
        stream.push_content(Duration::from_millis(300 + i * 10));
    }
    PassOutput {
        stream,
        prompt_tokens: Some(30),
        prompt_ms: Some(60.0),
        generated_tokens: Some(12),
        predicted_ms: Some(12.0 / tokens_per_s * 1000.0),
    }
}

enum Step {
    Ok(PassOutput),
    Fail(&'static str),
    /// Never finishes; the test cancels it.
    Hang,
}

struct Fake {
    steps: VecDeque<Step>,
    generated: u32,
    cold_loads: u32,
    cold_result: Result<Duration, PassError>,
    on_hang: Option<Arc<AtomicBool>>,
}

impl Fake {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: steps.into(),
            generated: 0,
            cold_loads: 0,
            cold_result: Ok(Duration::from_millis(1500)),
            on_hang: None,
        }
    }
}

impl LabTarget for Fake {
    fn owned_pgid(&self) -> Option<i32> {
        None
    }
    async fn cold_load(&mut self, _cancel: &Arc<AtomicBool>) -> Result<Duration, PassError> {
        self.cold_loads += 1;
        self.cold_result.clone()
    }
    async fn generate(
        &mut self,
        _workload: &Workload,
        _cancel: &Arc<AtomicBool>,
    ) -> Result<PassOutput, PassError> {
        self.generated += 1;
        match self.steps.pop_front().expect("script ran out") {
            Step::Ok(out) => Ok(out),
            Step::Fail(m) => Err(PassError::Failed(m.into())),
            Step::Hang => {
                if let Some(flag) = &self.on_hang {
                    flag.store(true, Ordering::Relaxed);
                }
                std::future::pending().await
            }
        }
    }
    async fn observed(&mut self) -> Settings {
        Settings {
            context_length: Some(4096),
            ..Settings::default()
        }
    }
}

async fn go(db: &TempDb, fake: &mut Fake, spec: NewExperiment, cancel: Arc<AtomicBool>) -> Outcome {
    let (tx, _rx) = watch::channel(Progress::default());
    run(
        db.db(),
        &hardware(),
        fake,
        spec,
        &Workload::standard(),
        cancel,
        &tx,
    )
    .await
}

fn kinds(outcome: &Outcome) -> Vec<(PassKind, u32, State)> {
    outcome
        .passes
        .iter()
        .map(|p| (p.kind, p.ordinal, p.state))
        .collect()
}

#[tokio::test]
async fn the_protocol_runs_cold_then_warmup_then_measured_and_stores_everything() {
    let db = TempDb::new();
    let mut fake = Fake::new(vec![
        Step::Ok(output(10.0)),
        Step::Ok(output(40.0)),
        Step::Ok(output(44.0)),
        Step::Ok(output(42.0)),
    ]);
    let outcome = go(&db, &mut fake, spec(true, 3), Arc::default()).await;
    assert_eq!(outcome.state, State::Completed, "{:?}", outcome.failure);
    assert_eq!(
        kinds(&outcome),
        vec![
            (PassKind::ColdLoad, 0, State::Completed),
            (PassKind::Warmup, 0, State::Completed),
            (PassKind::Measured, 0, State::Completed),
            (PassKind::Measured, 1, State::Completed),
            (PassKind::Measured, 2, State::Completed),
        ]
    );
    assert_eq!((fake.cold_loads, fake.generated), (1, 4));
    assert_eq!(outcome.summary.measured_ok, 3);
    assert_eq!(outcome.summary.decode_tokens_per_s.unwrap().median, 42.0);
    assert_eq!(outcome.summary.load_ms.unwrap().median, 1500.0);
    assert_eq!(
        outcome.observed.as_ref().unwrap().context_length,
        Some(4096)
    );
    assert!(outcome.store_error.is_none());

    let stored = db
        .db()
        .call(move |s| s.load_experiment(outcome.experiment_id.unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, State::Completed);
    assert_eq!(stored.passes.len(), 5);
    assert_eq!(stored.summary.unwrap().measured_ok, 3);
    // 12 single-token events, 12 reported tokens, 11 intervals: ITL is known.
    assert!(stored.passes[2].metrics.as_ref().unwrap().itl.is_some());
}

#[tokio::test]
async fn without_cold_the_warmup_comes_first_and_no_load_is_timed() {
    let db = TempDb::new();
    let mut fake = Fake::new(vec![Step::Ok(output(10.0)), Step::Ok(output(40.0))]);
    let outcome = go(&db, &mut fake, spec(false, 1), Arc::default()).await;
    assert_eq!(kinds(&outcome)[0].0, PassKind::Warmup);
    assert_eq!(fake.cold_loads, 0);
    assert!(outcome.summary.load_ms.is_none());
}

#[tokio::test]
async fn one_failed_pass_is_counted_and_left_out_but_two_in_a_row_stop_the_run() {
    let db = TempDb::new();
    let mut fake = Fake::new(vec![
        Step::Ok(output(10.0)),
        Step::Ok(output(40.0)),
        Step::Fail("dropped"),
        Step::Ok(output(44.0)),
    ]);
    let outcome = go(&db, &mut fake, spec(false, 3), Arc::default()).await;
    assert_eq!(outcome.state, State::Completed);
    assert_eq!(
        (outcome.summary.measured_ok, outcome.summary.excluded),
        (2, 1)
    );
    assert_eq!(outcome.passes[2].failure.as_deref(), Some("dropped"));

    let db = TempDb::new();
    let mut fake = Fake::new(vec![
        Step::Ok(output(10.0)),
        Step::Fail("a"),
        Step::Fail("b"),
    ]);
    let outcome = go(&db, &mut fake, spec(false, 5), Arc::default()).await;
    assert_eq!(outcome.state, State::Failed);
    assert_eq!(fake.generated, 3, "later passes are not attempted");
    assert!(outcome.failure.unwrap().contains("in a row"));
}

#[tokio::test]
async fn a_failed_cold_load_or_warmup_ends_the_run_before_any_measured_pass() {
    let db = TempDb::new();
    let mut fake = Fake::new(vec![]);
    fake.cold_result = Err(PassError::Failed("did not become ready".into()));
    let outcome = go(&db, &mut fake, spec(true, 3), Arc::default()).await;
    assert_eq!(outcome.state, State::Failed);
    assert_eq!(outcome.failure.as_deref(), Some("did not become ready"));
    assert_eq!(fake.generated, 0);

    let db = TempDb::new();
    let mut fake = Fake::new(vec![Step::Fail("model missing")]);
    let outcome = go(&db, &mut fake, spec(false, 3), Arc::default()).await;
    assert_eq!(outcome.state, State::Failed);
    assert_eq!(outcome.passes.len(), 1);
}

#[tokio::test]
async fn cancelling_during_a_pass_stops_cleanly_and_leaves_no_running_pass() {
    let db = TempDb::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut fake = Fake::new(vec![Step::Ok(output(10.0)), Step::Hang]);
    fake.on_hang = Some(cancel.clone());
    let outcome = go(&db, &mut fake, spec(false, 3), cancel).await;
    assert_eq!(outcome.state, State::Cancelled);
    assert_eq!(fake.generated, 2);
    assert_eq!(outcome.passes.last().unwrap().state, State::Cancelled);
    let stored = db
        .db()
        .call(move |s| s.load_experiment(outcome.experiment_id.unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, State::Cancelled);
    assert!(stored.passes.iter().all(|p| p.state != State::Running));
    // The store accepts a new experiment afterwards.
    let mut again = Fake::new(vec![Step::Ok(output(1.0)), Step::Ok(output(1.0))]);
    let outcome = go(&db, &mut again, spec(false, 1), Arc::default()).await;
    assert_eq!(outcome.state, State::Completed);
}

#[tokio::test]
async fn cancelled_before_start_runs_nothing() {
    let db = TempDb::new();
    let mut fake = Fake::new(vec![]);
    let outcome = go(
        &db,
        &mut fake,
        spec(true, 3),
        Arc::new(AtomicBool::new(true)),
    )
    .await;
    assert_eq!(outcome.state, State::Cancelled);
    assert!(outcome.passes.is_empty());
    assert_eq!((fake.cold_loads, fake.generated), (0, 0));
}

#[tokio::test]
async fn an_experiment_that_misdescribes_the_workload_or_overlaps_another_is_refused() {
    let db = TempDb::new();
    let mut fake = Fake::new(vec![]);
    let mut wrong = spec(false, 1);
    wrong.seed += 1;
    let outcome = go(&db, &mut fake, wrong, Arc::default()).await;
    assert!(outcome.experiment_id.is_none());
    assert_eq!(fake.generated, 0);

    // A record left running blocks a second experiment before it touches the runtime.
    db.db()
        .call(|s| s.begin_experiment(&spec(false, 1), 1))
        .await
        .unwrap();
    let outcome = go(&db, &mut fake, spec(false, 1), Arc::default()).await;
    assert!(outcome.experiment_id.is_none());
    assert!(outcome.failure.unwrap().contains("still running"));
    assert_eq!(fake.generated, 0);
}

#[tokio::test]
async fn a_storage_failure_mid_run_is_reported_and_the_measurements_survive() {
    let db = TempDb::new();
    let handle = db.db();
    let mut fake = Fake::new(vec![Step::Ok(output(10.0)), Step::Ok(output(40.0))]);
    // Break the pass table after the experiment can be created.
    rusqlite::Connection::open(&db.0)
        .unwrap()
        .execute_batch("DROP TABLE experiment_passes")
        .unwrap();
    let (tx, _rx) = watch::channel(Progress::default());
    let outcome = Box::pin(run(
        handle,
        &hardware(),
        &mut fake,
        spec(false, 1),
        &Workload::standard(),
        Arc::default(),
        &tx,
    ))
    .await;
    assert!(outcome.store_error.is_some());
    assert_eq!(outcome.summary.measured_ok, 1, "result is still shown");
    assert_eq!(outcome.state, State::Completed);
}

#[tokio::test]
async fn the_guard_stops_a_pass_on_deadline_and_on_cancel() {
    let cancel = AtomicBool::new(false);
    let slow = guarded(&cancel, Duration::from_millis(20), async {
        std::future::pending::<Result<(), PassError>>().await
    })
    .await;
    assert!(matches!(slow, Err(PassError::Failed(m)) if m.contains("too long")));
    cancel.store(true, Ordering::Relaxed);
    let stopped = guarded(&cancel, Duration::from_secs(5), async {
        std::future::pending::<Result<(), PassError>>().await
    })
    .await;
    assert_eq!(stopped, Err(PassError::Cancelled));
}

#[test]
fn metrics_use_server_counters_and_say_why_itl_is_unknown() {
    let mut out = output(25.0);
    let metrics = metrics_from(&out, Duration::from_millis(900));
    assert_eq!(metrics.decode_tokens_per_s, Some(25.0));
    assert_eq!(metrics.prompt_tokens_per_s, Some(500.0));
    assert_eq!(metrics.ttft_ms, Some(300.0));
    assert!(metrics.itl.is_some() && metrics.itl_unknown_reason.is_none());
    assert_eq!(metrics.wall_ms, 900);

    out.stream.reported_tokens = Some(40);
    let metrics = metrics_from(&out, Duration::from_millis(900));
    assert!(metrics.itl.is_none());
    assert!(
        metrics
            .itl_unknown_reason
            .unwrap()
            .contains("12 stream events for 40")
    );
    out.generated_tokens = Some(0);
    assert!(
        metrics_from(&out, Duration::ZERO)
            .decode_tokens_per_s
            .is_none()
    );
}

#[tokio::test]
async fn a_result_that_lands_with_the_cancel_does_not_win() {
    let cancel = AtomicBool::new(true);
    let result = guarded(&cancel, Duration::from_secs(5), async {
        Ok::<_, PassError>(1)
    })
    .await;
    assert_eq!(result, Err(PassError::Cancelled));
}

#[tokio::test]
async fn a_pass_that_could_not_be_saved_is_not_counted_in_the_stored_summary() {
    let db = TempDb::new();
    let handle = db.db();
    // Saving a completed pass fails, so the store will close it as interrupted.
    rusqlite::Connection::open(&db.0)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_completed BEFORE UPDATE ON experiment_passes
             WHEN NEW.state = 'completed'
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    let mut fake = Fake::new(vec![Step::Ok(output(10.0)), Step::Ok(output(40.0))]);
    let (tx, _rx) = watch::channel(Progress::default());
    let outcome = Box::pin(run(
        handle,
        &hardware(),
        &mut fake,
        spec(false, 1),
        &Workload::standard(),
        Arc::default(),
        &tx,
    ))
    .await;
    assert!(outcome.store_error.is_some());
    assert_eq!(
        outcome.state,
        State::Completed,
        "the shown result is complete"
    );
    assert_eq!(outcome.summary.measured_ok, 1);
    let stored = db
        .db()
        .call(move |s| s.load_experiment(outcome.experiment_id.unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.summary.unwrap().measured_ok,
        0,
        "stored summary matches stored passes"
    );
    assert_eq!(stored.state, State::Failed);
    assert!(stored.passes.iter().all(|p| p.state == State::Interrupted));
}
