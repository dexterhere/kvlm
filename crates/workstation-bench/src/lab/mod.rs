// SPDX-License-Identifier: GPL-3.0-or-later
//! The Benchmark Lab protocol: optional cold load, one discarded warm-up, then N measured
//! passes of the same workload, each sampled for resources and written to the experiment
//! store as it finishes. The runner is generic over a target so the protocol is tested
//! without a runtime; `llama::LlamaTarget` is the managed llama.cpp implementation.

use crate::{
    metrics::{StreamRecord, tokens_per_second},
    sampler,
};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use workstation_core::{
    db::HistoryStore,
    experiments::{
        NewExperiment, Pass, PassKind, PassMetrics, Settings, State, Summary, summarize,
    },
    hardware::{
        HardwareProfile,
        load::{LoadAggregate, LoadProbe},
        resources::MemoryProbe,
    },
};

pub mod llama;

pub const LAB_WORKLOAD_ID: &str = "lab-fixed-v1";
pub const LAB_PROMPT: &str = "Count from one to ten in words, name three primary colors, and finish with one short sentence about the sea.";
pub const LAB_SEED: u32 = 42;
pub const LAB_MAX_TOKENS: u32 = 96;
/// One pass, cold load or warm-up, and the whole generation, may not run longer than this.
pub const PASS_DEADLINE: Duration = Duration::from_secs(180);
/// Two failed measured passes in a row end the experiment: the runtime is not coming back.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 2;
const OBSERVE_DEADLINE: Duration = Duration::from_secs(10);

/// What every pass asks the runtime to generate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workload {
    id: String,
    prompt: String,
    prompt_sha256: String,
    seed: u32,
    max_tokens: u32,
}

impl Workload {
    pub fn standard() -> Self {
        Self::new(LAB_WORKLOAD_ID, LAB_PROMPT, LAB_SEED, LAB_MAX_TOKENS)
    }

    pub fn new(id: &str, prompt: &str, seed: u32, max_tokens: u32) -> Self {
        Self {
            id: id.into(),
            prompt: prompt.into(),
            prompt_sha256: format!("{:x}", Sha256::digest(prompt.as_bytes())),
            seed,
            max_tokens,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    pub fn prompt_sha256(&self) -> &str {
        &self.prompt_sha256
    }
    pub fn seed(&self) -> u32 {
        self.seed
    }
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    /// An experiment record must describe exactly the workload that will run. The fields are
    /// private so the digest can never drift from the text.
    fn matches(&self, spec: &NewExperiment) -> bool {
        spec.workload_id == self.id
            && spec.prompt_sha256.eq_ignore_ascii_case(&self.prompt_sha256)
            && spec.seed == self.seed
            && spec.max_tokens == self.max_tokens
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassError {
    Cancelled,
    Failed(String),
}

/// What one generation produced, before it is turned into stored metrics.
#[derive(Clone, Debug, Default)]
pub struct PassOutput {
    pub stream: StreamRecord,
    pub prompt_tokens: Option<u64>,
    pub prompt_ms: Option<f64>,
    pub generated_tokens: Option<u64>,
    pub predicted_ms: Option<f64>,
}

pub trait LabTarget: Send {
    /// Process group of a runtime this application owns; `None` for an attached one.
    fn owned_pgid(&self) -> Option<i32>;
    /// Unload the model, load it again and report the time until the runtime is ready.
    /// Not dropped on cancel or deadline: a restart already handed to a supervisor cannot be
    /// recalled by dropping the future. The target must watch `cancel`, stop within
    /// [`PASS_DEADLINE`] and report how it ended.
    fn cold_load(
        &mut self,
        cancel: &Arc<AtomicBool>,
    ) -> impl Future<Output = Result<Duration, PassError>> + Send;
    fn generate(
        &mut self,
        workload: &Workload,
        cancel: &Arc<AtomicBool>,
    ) -> impl Future<Output = Result<PassOutput, PassError>> + Send;
    /// What the runtime reports it actually loaded; anything unreported stays `None`.
    fn observed(&mut self) -> impl Future<Output = Settings> + Send;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Starting,
    ColdLoad,
    Warmup,
    Measured,
    Finishing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// Measured passes finished (including failed ones).
    pub done: u32,
    pub of: u32,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    /// `None` when the experiment was refused before anything was recorded.
    pub experiment_id: Option<i64>,
    pub state: State,
    pub failure: Option<String>,
    pub passes: Vec<Pass>,
    pub summary: Summary,
    pub observed: Option<Settings>,
    /// The first storage error. The measurements above are still valid and shown.
    pub store_error: Option<String>,
}

impl Outcome {
    fn refused(message: String) -> Self {
        Self {
            experiment_id: None,
            state: State::Failed,
            failure: Some(message),
            passes: Vec::new(),
            summary: Summary::default(),
            observed: None,
            store_error: None,
        }
    }
}

/// The store from async code: every call runs on a blocking thread.
#[derive(Clone)]
pub struct Db(Arc<Mutex<HistoryStore>>);

impl Db {
    pub fn new(store: HistoryStore) -> Self {
        Self(Arc::new(Mutex::new(store)))
    }

    async fn call<R: Send + 'static>(
        &self,
        work: impl FnOnce(&HistoryStore) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let store = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            work(&guard)
        })
        .await
        .unwrap_or_else(|_| Err("The experiment store worker failed.".into()))
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u128::from(u64::MAX)) as u64)
}

/// Writes pass records, remembering only the first storage error so a database failure
/// after a long run never discards the measurements.
struct Recorder {
    db: Db,
    experiment: i64,
    first_error: Option<String>,
}

impl Recorder {
    fn note(&mut self, result: Result<(), String>) {
        if let Err(message) = result {
            self.first_error.get_or_insert(message);
        }
    }

    async fn begin(&mut self, kind: PassKind, ordinal: u32) -> Option<i64> {
        let experiment = self.experiment;
        match self
            .db
            .call(move |s| s.begin_pass(experiment, kind, ordinal, now_ms()))
            .await
        {
            Ok(id) => Some(id),
            Err(message) => {
                self.first_error.get_or_insert(message);
                None
            }
        }
    }

    async fn finish(
        &mut self,
        pass: Option<i64>,
        state: State,
        metrics: Option<PassMetrics>,
        failure: Option<String>,
    ) {
        let Some(pass) = pass else { return };
        let result = self
            .db
            .call(move |s| {
                s.finish_pass(pass, state, metrics.as_ref(), failure.as_deref(), now_ms())
            })
            .await;
        self.note(result);
    }
}

struct PassResult {
    state: State,
    metrics: Option<PassMetrics>,
    failure: Option<String>,
}

impl PassResult {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            state: State::Failed,
            metrics: None,
            failure: Some(message.into()),
        }
    }
    fn cancelled() -> Self {
        Self {
            state: State::Cancelled,
            metrics: None,
            failure: None,
        }
    }
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Run `work` until it ends, the deadline passes or the user cancels.
async fn guarded<R>(
    cancel: &AtomicBool,
    deadline: Duration,
    work: impl Future<Output = Result<R, PassError>>,
) -> Result<R, PassError> {
    let result = tokio::select! {
        biased;
        () = cancelled(cancel) => Err(PassError::Cancelled),
        r = tokio::time::timeout(deadline, work) => match r {
            Ok(result) => result,
            Err(_) => Err(PassError::Failed("The pass took too long and was stopped.".into())),
        },
    };
    // A result that arrived in the same instant as the user's cancel does not win.
    if result.is_ok() && cancel.load(Ordering::Relaxed) {
        return Err(PassError::Cancelled);
    }
    result
}

fn into_result(error: PassError) -> PassResult {
    match error {
        PassError::Cancelled => PassResult::cancelled(),
        PassError::Failed(message) => PassResult::failed(message),
    }
}

async fn cold_pass<T: LabTarget>(target: &mut T, cancel: &Arc<AtomicBool>) -> PassResult {
    let started = Instant::now();
    let loaded = target.cold_load(cancel).await.and_then(|loaded| {
        if cancel.load(Ordering::Relaxed) {
            Err(PassError::Cancelled)
        } else {
            Ok(loaded)
        }
    });
    match loaded {
        Ok(loaded) => PassResult {
            state: State::Completed,
            metrics: Some(PassMetrics {
                wall_ms: millis(started.elapsed()),
                load_ms: Some(loaded.as_secs_f64() * 1000.0),
                ..PassMetrics::default()
            }),
            failure: None,
        },
        Err(error) => into_result(error),
    }
}

async fn generate_pass<T: LabTarget>(
    target: &mut T,
    workload: &Workload,
    cancel: &Arc<AtomicBool>,
    hardware: &HardwareProfile,
) -> PassResult {
    let aggregate = Arc::new(Mutex::new(LoadAggregate::default()));
    let mut memory = MemoryProbe::new(hardware);
    let mut load = LoadProbe::new(hardware, target.owned_pgid());
    let sink = aggregate.clone();
    let probe = move || {
        let reading = memory.sample();
        let sample = load.sample();
        let at = sample.at;
        let mut aggregate = sink.lock().unwrap_or_else(|e| e.into_inner());
        aggregate.add(sample);
        for gpu in &reading.gpus {
            aggregate.add_gpu_memory(at, &gpu.id, gpu.vram_used_bytes);
        }
        reading
    };
    let started = Instant::now();
    let (result, _summary) = sampler::measure(
        guarded(cancel, PASS_DEADLINE, target.generate(workload, cancel)),
        probe,
    )
    .await;
    let wall = started.elapsed();
    let output = match result {
        Ok(output) => output,
        Err(error) => return into_result(error),
    };
    let mut metrics = metrics_from(&output, wall);
    aggregate
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .apply(&mut metrics);
    PassResult {
        state: State::Completed,
        metrics: Some(metrics),
        failure: None,
    }
}

fn metrics_from(output: &PassOutput, wall: Duration) -> PassMetrics {
    let (itl, itl_unknown_reason) = match output.stream.itl() {
        Ok(itl) => (Some(itl), None),
        Err(reason) => (None, Some(reason)),
    };
    PassMetrics {
        wall_ms: millis(wall),
        ttft_ms: output.stream.ttft_ms(),
        prompt_tokens: output.prompt_tokens,
        prompt_tokens_per_s: tokens_per_second(output.prompt_tokens, output.prompt_ms),
        generated_tokens: output.generated_tokens,
        decode_tokens_per_s: tokens_per_second(output.generated_tokens, output.predicted_ms),
        itl,
        itl_unknown_reason,
        ..PassMetrics::default()
    }
}

fn millis(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

/// Run one experiment to a terminal state. Nothing is executed unless the experiment record
/// was saved first; the caller has already obtained the user's Check + Run.
pub async fn run<T: LabTarget>(
    db: Db,
    hardware: &HardwareProfile,
    target: &mut T,
    spec: NewExperiment,
    workload: &Workload,
    cancel: Arc<AtomicBool>,
    progress: &watch::Sender<Progress>,
) -> Outcome {
    if !workload.matches(&spec) {
        return Outcome::refused(
            "The experiment does not describe the workload it would run.".into(),
        );
    }
    let measured_total = spec.passes_requested;
    let cold = spec.cold;
    let saved = spec.clone();
    let id = match db.call(move |s| s.begin_experiment(&saved, now_ms())).await {
        Ok(id) => id,
        Err(message) => return Outcome::refused(message),
    };
    let mut recorder = Recorder {
        db: db.clone(),
        experiment: id,
        first_error: None,
    };
    let mut passes: Vec<Pass> = Vec::new();
    let mut end = (State::Completed, None::<String>);
    let report = |phase, done| {
        progress.send_replace(Progress {
            phase,
            done,
            of: measured_total,
        });
    };

    let mut plan: Vec<(PassKind, u32)> = Vec::new();
    if cold {
        plan.push((PassKind::ColdLoad, 0));
    }
    plan.push((PassKind::Warmup, 0));
    plan.extend((0..measured_total).map(|i| (PassKind::Measured, i)));

    let mut consecutive_failures = 0;
    let mut measured_done = 0;
    for (kind, ordinal) in plan {
        if cancel.load(Ordering::Relaxed) {
            end = (State::Cancelled, None);
            break;
        }
        report(
            match kind {
                PassKind::ColdLoad => Phase::ColdLoad,
                PassKind::Warmup => Phase::Warmup,
                PassKind::Measured => Phase::Measured,
            },
            measured_done,
        );
        let pass_id = recorder.begin(kind, ordinal).await;
        let started = now_ms();
        let result = match kind {
            PassKind::ColdLoad => cold_pass(target, &cancel).await,
            // Boxed: this future carries two probes, which are large in a debug build.
            _ => Box::pin(generate_pass(target, workload, &cancel, hardware)).await,
        };
        recorder
            .finish(
                pass_id,
                result.state,
                result.metrics.clone(),
                result.failure.clone(),
            )
            .await;
        passes.push(Pass {
            id: pass_id.unwrap_or(-1),
            kind,
            ordinal,
            state: result.state,
            started_at_unix_ms: started,
            finished_at_unix_ms: Some(now_ms()),
            metrics: result.metrics,
            failure: result.failure.clone(),
        });
        if kind == PassKind::Measured {
            measured_done += 1;
        }
        match (result.state, kind) {
            (State::Cancelled, _) => {
                end = (State::Cancelled, None);
                break;
            }
            (State::Failed, PassKind::Measured) => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    end = (
                        State::Failed,
                        Some(
                            "Two measured passes failed in a row; the experiment was stopped."
                                .into(),
                        ),
                    );
                    break;
                }
            }
            (State::Failed, _) => {
                end = (State::Failed, result.failure);
                break;
            }
            (_, PassKind::Measured) => consecutive_failures = 0,
            _ => {}
        }
    }

    report(Phase::Finishing, measured_done);
    let observed = tokio::time::timeout(OBSERVE_DEADLINE, target.observed())
        .await
        .ok();
    let summary = summarize(&passes);
    let decide = |summary: &Summary| match end.0 {
        State::Completed if summary.measured_ok == 0 => State::Failed,
        other => other,
    };
    let state = decide(&summary);
    let failure = end.1.or_else(|| {
        (state == State::Failed && summary.measured_ok == 0)
            .then(|| "No measured pass completed.".to_string())
    });
    // After a storage failure some passes exist only in memory, and the store closes the
    // unsaved ones as interrupted. Summarize what was actually saved so the record agrees
    // with its own passes.
    let (stored_summary, stored_state) = if recorder.first_error.is_some() {
        match db.call(move |s| s.load_experiment(id)).await {
            Ok(Some(saved)) => {
                let saved = summarize(&saved.passes);
                let saved_state = decide(&saved);
                (Some(saved), saved_state)
            }
            _ => (None, state),
        }
    } else {
        (Some(summary.clone()), state)
    };
    let (stored_observed, stored_failure) = (observed.clone(), failure.clone());
    let finished = db
        .call(move |s| {
            s.finish_experiment(
                id,
                stored_state,
                stored_observed.as_ref(),
                stored_summary.as_ref(),
                stored_failure.as_deref(),
                now_ms(),
            )
        })
        .await;
    recorder.note(finished);
    Outcome {
        experiment_id: Some(id),
        state,
        failure,
        passes,
        summary,
        observed,
        store_error: recorder.first_error,
    }
}

#[cfg(test)]
mod tests;
