// SPDX-License-Identifier: GPL-3.0-or-later
//! The Benchmark Lab as the interface sees it: a typed command and event pair on its own
//! channels, one run at a time. A run only starts from an explicit `Run` the interface sends
//! after a successful `Check`; nothing runs at launch and nothing leaves the machine.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::Read,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};
use workstation_bench::lab::{
    self, Db, Progress, Workload,
    llama::{ColdLoaded, LlamaTarget},
};
use workstation_core::{
    compare::{Comparison, compare, compare_follow_up},
    db::experiments::Listing,
    diagnostics::{self, Context},
    experiments::{
        Engine, Experiment, MAX_PASSES, NewExperiment, Ownership, PassKind, State, Variable,
        follow_up::{requested_settings, with_variable},
    },
    hardware::HardwareProfile,
    models::gguf::inspect_file,
};
use workstation_runtimes::llamacpp::client::LlamaClient;

use crate::{
    bridge::BoxFuture,
    managed::{self, Expect, LiveManaged, blocking, open_store},
};

/// What the user asked for. Everything else comes from the running managed server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabRequest {
    pub passes: u32,
    pub cold: bool,
    /// A controlled change of an earlier experiment: its id and the one setting to change.
    pub follow_up: Option<(i64, Variable)>,
    /// The server a Check looked at. Run refuses to act on a different one.
    pub server: Option<String>,
}

/// The answer to a Check: what Run would do, and the server it was worked out against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub description: String,
    pub server: String,
}

#[derive(Debug)]
pub enum LabCommand {
    List {
        id: u64,
    },
    Load {
        id: u64,
        experiment: i64,
    },
    Delete {
        experiment: i64,
    },
    Check {
        id: u64,
        request: LabRequest,
    },
    Run {
        id: u64,
        request: LabRequest,
        cancel: Arc<AtomicBool>,
    },
    Compare {
        id: u64,
        a: i64,
        b: i64,
    },
}

/// A finished run: the stored experiment and anything the user should know besides it.
#[derive(Clone, Debug)]
pub struct RunDone {
    pub experiment: Box<Experiment>,
    pub notice: Option<String>,
}

#[derive(Debug)]
pub enum LabEvent {
    Listed {
        id: u64,
        result: Result<Listing, String>,
    },
    Loaded {
        id: u64,
        result: Result<Option<Box<Experiment>>, String>,
    },
    Deleted {
        experiment: i64,
        result: Result<(), String>,
    },
    /// A plain-language statement of exactly what `Run` would do.
    Checked {
        id: u64,
        result: Result<Checked, String>,
    },
    Progress {
        id: u64,
        progress: Progress,
    },
    Finished {
        id: u64,
        result: Result<RunDone, String>,
    },
    Compared {
        id: u64,
        result: Result<Box<Comparison>, String>,
    },
}

pub trait LabBackend: Send + Sync + 'static {
    fn list(&self) -> BoxFuture<Result<Listing, String>>;
    fn load(&self, experiment: i64) -> BoxFuture<Result<Option<Experiment>, String>>;
    fn delete(&self, experiment: i64) -> BoxFuture<Result<(), String>>;
    fn check(&self, request: LabRequest) -> BoxFuture<Result<Checked, String>>;
    fn run(
        &self,
        request: LabRequest,
        cancel: Arc<AtomicBool>,
        progress: watch::Sender<Progress>,
    ) -> BoxFuture<Result<RunDone, String>>;
}

/// Start the lab worker. Commands are handled concurrently except that only one `Run` may be
/// in flight; a second one is refused rather than queued.
pub fn start(
    runtime: &tokio::runtime::Handle,
    backend: Arc<dyn LabBackend>,
) -> (mpsc::Sender<LabCommand>, async_channel::Receiver<LabEvent>) {
    let (commands, mut inbox) = mpsc::channel::<LabCommand>(16);
    let (events, outbox) = async_channel::bounded::<LabEvent>(16);
    let running = Arc::new(AtomicBool::new(false));
    runtime.spawn(async move {
        while let Some(command) = inbox.recv().await {
            let backend = backend.clone();
            let events = events.clone();
            match command {
                LabCommand::Run {
                    id,
                    request,
                    cancel,
                } => {
                    if running.swap(true, Ordering::AcqRel) {
                        let _ = events
                            .send(LabEvent::Finished {
                                id,
                                result: Err("A lab run is already in progress.".into()),
                            })
                            .await;
                        continue;
                    }
                    let running = running.clone();
                    tokio::spawn(async move {
                        let (progress, mut seen) = watch::channel(Progress {
                            phase: lab::Phase::Starting,
                            done: 0,
                            of: request.passes,
                        });
                        let forward = {
                            let events = events.clone();
                            tokio::spawn(async move {
                                while seen.changed().await.is_ok() {
                                    let progress = *seen.borrow_and_update();
                                    // A stale tick is dropped, never queued behind the result.
                                    let _ = events.try_send(LabEvent::Progress { id, progress });
                                }
                            })
                        };
                        // A panic in the backend must not leave the slot taken or the page
                        // waiting for a result that never comes.
                        let work =
                            tokio::spawn(
                                async move { backend.run(request, cancel, progress).await },
                            );
                        let result = work
                            .await
                            .unwrap_or_else(|_| Err("The lab run stopped unexpectedly.".into()));
                        forward.abort();
                        running.store(false, Ordering::Release);
                        let _ = events.send(LabEvent::Finished { id, result }).await;
                    });
                }
                other => {
                    tokio::spawn(async move {
                        let event = match other {
                            LabCommand::List { id } => LabEvent::Listed {
                                id,
                                result: backend.list().await,
                            },
                            LabCommand::Load { id, experiment } => LabEvent::Loaded {
                                id,
                                result: backend
                                    .load(experiment)
                                    .await
                                    .map(|found| found.map(Box::new)),
                            },
                            LabCommand::Delete { experiment } => LabEvent::Deleted {
                                experiment,
                                result: backend.delete(experiment).await,
                            },
                            LabCommand::Check { id, request } => LabEvent::Checked {
                                id,
                                result: backend.check(request).await,
                            },
                            LabCommand::Compare { id, a, b } => LabEvent::Compared {
                                id,
                                result: compare_stored(&*backend, a, b).await.map(Box::new),
                            },
                            LabCommand::Run { .. } => return,
                        };
                        let _ = events.send(event).await;
                    });
                }
            }
        }
    });
    (commands, outbox)
}

async fn compare_stored(backend: &dyn LabBackend, a: i64, b: i64) -> Result<Comparison, String> {
    let find = |id: i64| async move {
        backend
            .load(id)
            .await?
            .ok_or_else(|| "One of the experiments no longer exists.".to_string())
    };
    let (first, second) = (find(a).await?, find(b).await?);
    // The controlled rules apply when B declares A as its baseline.
    let result = if second.spec.baseline_id == Some(first.id) {
        compare_follow_up(&first, &second)
    } else {
        compare(&first, &second)
    };
    result.map_err(|refusal| refusal.to_string())
}

/// The production backend: the managed llama.cpp server and the local experiment store.
pub struct LiveLab {
    managed: Arc<LiveManaged>,
}

impl LiveLab {
    pub fn new() -> Self {
        Self {
            managed: managed::live(),
        }
    }
}

/// Identifies one running server: its profile, build and generation.
fn server_token(
    active: &managed::Active,
    status: &workstation_runtimes::supervisor::Status,
) -> String {
    format!(
        "{}|{}|{}",
        active.profile.name,
        active.install.dir_name,
        status.generation.as_deref().unwrap_or("")
    )
}

/// Records experiments a previous process left running, once, before the first list or run.
/// The lab runs one experiment at a time in one application instance, so a record still
/// running at this point can only be a leftover.
fn recover_once(store: &workstation_core::db::HistoryStore) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) && store.recover_interrupted(lab::now_ms()).is_err() {
        DONE.store(false, Ordering::Release);
    }
}

/// What a request resolves to against the running server.
struct Prepared {
    /// Whether the run times a cold load. A follow-up repeats its baseline's cache state,
    /// because a comparison across cold and warm is refused.
    cold: bool,
    active: managed::Active,
    status: workstation_runtimes::supervisor::Status,
    /// The baseline experiment and the profile with its one declared change.
    follow_up: Option<(
        Experiment,
        Variable,
        workstation_core::runtime::profile::LaunchProfile,
    )>,
}

impl LiveLab {
    async fn prepare(&self, request: &LabRequest) -> Result<Prepared, String> {
        if !(1..=MAX_PASSES).contains(&request.passes) {
            return Err(format!("Passes must be between 1 and {MAX_PASSES}."));
        }
        let Some((active, status)) = self.managed.running().await else {
            return Err(
                "Start a managed llama.cpp server on the Runtimes page first. The lab only measures a server this application started.".into(),
            );
        };
        if let Some(server) = &request.server
            && *server != server_token(&active, &status)
        {
            return Err("The managed server changed since Check. Press Check again.".into());
        }
        let Some((baseline_id, variable)) = request.follow_up else {
            return Ok(Prepared {
                cold: request.cold,
                active,
                status,
                follow_up: None,
            });
        };
        let baseline = blocking(move || open_store()?.load_experiment(baseline_id))
            .await?
            .ok_or("The baseline experiment no longer exists.")?;
        if baseline.state != State::Completed
            || baseline.spec.engine != Engine::Llamacpp
            || baseline.spec.ownership != Ownership::Managed
        {
            return Err(
                "Only a completed run of a managed llama.cpp server can be changed.".into(),
            );
        }
        if requested_settings(&active.profile) != baseline.spec.requested {
            return Err("The running profile differs from the one the baseline used. Start that profile, then run the follow-up.".into());
        }
        let prompt_tokens = baseline
            .passes
            .iter()
            .filter(|p| p.kind == PassKind::Measured && p.state == State::Completed)
            .filter_map(|p| p.metrics.as_ref()?.prompt_tokens)
            .max();
        let derived = with_variable(
            &active.profile,
            variable,
            prompt_tokens,
            Workload::standard().max_tokens(),
        )?;
        Ok(Prepared {
            cold: baseline.spec.cold,
            active,
            status,
            follow_up: Some((baseline, variable, derived)),
        })
    }
}

impl LabBackend for LiveLab {
    fn list(&self) -> BoxFuture<Result<Listing, String>> {
        Box::pin(blocking(|| {
            let store = open_store()?;
            recover_once(&store);
            store.list_experiments(100)
        }))
    }

    fn load(&self, experiment: i64) -> BoxFuture<Result<Option<Experiment>, String>> {
        Box::pin(blocking(move || open_store()?.load_experiment(experiment)))
    }

    fn delete(&self, experiment: i64) -> BoxFuture<Result<(), String>> {
        Box::pin(blocking(move || {
            open_store()?.delete_experiment(experiment)
        }))
    }

    fn check(&self, request: LabRequest) -> BoxFuture<Result<Checked, String>> {
        let this = Self {
            managed: self.managed.clone(),
        };
        Box::pin(async move {
            let prepared = this.prepare(&request).await?;
            let profile = &prepared.active.profile;
            let mut text = format!(
                "Runs {} measured pass{} of a fixed prompt against profile \u{201c}{}\u{201d} (llama.cpp {} {}), after one warm-up pass.",
                request.passes,
                if request.passes == 1 { "" } else { "es" },
                profile.name,
                prepared.active.install.version,
                prepared.active.install.backend,
            );
            if prepared.cold {
                text.push_str(
                    " The server is restarted once first so the model load is timed from cold.",
                );
            }
            if let Some((_, variable, _)) = &prepared.follow_up {
                text.push_str(&format!(
                    " The server is restarted with one setting changed ({}) and restored to this profile afterwards. The cache state stays that of the baseline ({}).",
                    variable.as_str(),
                    if prepared.cold { "cold" } else { "warm" }
                ));
            }
            Ok(Checked {
                description: text,
                server: server_token(&prepared.active, &prepared.status),
            })
        })
    }

    fn run(
        &self,
        request: LabRequest,
        cancel: Arc<AtomicBool>,
        progress: watch::Sender<Progress>,
    ) -> BoxFuture<Result<RunDone, String>> {
        let this = Self {
            managed: self.managed.clone(),
        };
        // The run future is large in debug builds; keep it off the stack.
        Box::pin(async move { this.run_checked(request, cancel, progress).await })
    }
}

impl LiveLab {
    async fn run_checked(
        &self,
        request: LabRequest,
        cancel: Arc<AtomicBool>,
        progress: watch::Sender<Progress>,
    ) -> Result<RunDone, String> {
        let prepared = self.prepare(&request).await?;
        let request = LabRequest {
            cold: prepared.cold,
            ..request
        };
        let Prepared {
            cold: _,
            active,
            status,
            follow_up,
        } = prepared;
        let hardware = blocking(|| Ok(HardwareProfile::detect())).await?;
        let model_path = active.profile.model_path.clone();
        let (model_sha256, layers) = blocking(move || model_facts(Path::new(&model_path))).await?;
        if cancel.load(Ordering::Relaxed) {
            return Err("The run was cancelled.".into());
        }
        let Some(alias) = status.generation.clone() else {
            return Err("The server is not ready.".into());
        };

        let run_profile = follow_up
            .as_ref()
            .map_or_else(|| active.profile.clone(), |(_, _, derived)| derived.clone());
        let target = if follow_up.is_some() {
            self.managed
                .switch(
                    run_profile.clone(),
                    active.install.clone(),
                    &Expect::Generation(alias),
                    &cancel,
                )
                .await
                .map(|r| (r.alias, r.pid, r.port))
        } else {
            match (status.pid, status.port) {
                (Some(pid), Some(port)) => Ok((alias, pid, port)),
                _ => Err("The server is not ready.".to_string()),
            }
        };
        let outcome = match target {
            Ok(target) => {
                // The measurement runs as its own task so that a panic in it cannot skip
                // putting the user's server back.
                let job = Measure {
                    managed: self.managed.clone(),
                    request: request.clone(),
                    active: active.clone(),
                    profile: run_profile.clone(),
                    hardware: hardware.clone(),
                    model_sha256,
                    target,
                    follow_up: follow_up.as_ref().map(|(b, v, _)| (b.id, *v)),
                    cancel,
                    progress,
                };
                tokio::spawn(job.run())
                    .await
                    .unwrap_or_else(|_| Err("The measurement stopped unexpectedly.".into()))
            }
            Err(error) => Err(error),
        };

        // A follow-up replaced the user's server; put theirs back whatever happened, unless
        // they have started something else since.
        let mut notice = None;
        if follow_up.is_some() {
            let never = AtomicBool::new(false);
            if let Err(error) = self
                .managed
                .switch(
                    active.profile.clone(),
                    active.install.clone(),
                    &Expect::Profile(Box::new(run_profile)),
                    &never,
                )
                .await
            {
                notice = Some(format!(
                    "The original profile could not be restarted afterwards: {error}"
                ));
            }
        }
        let (id, store_error) = match outcome {
            Ok(done) => done,
            Err(error) => {
                return Err(match notice {
                    Some(restore) => format!("{error} {restore}"),
                    None => error,
                });
            }
        };
        let ctx = Context {
            offloadable_layers: layers.map(|n| n.saturating_add(1)),
            unified_memory: unified_memory(&hardware),
        };
        let experiment = blocking(move || {
            let store = open_store()?;
            let stored = store
                .load_experiment(id)?
                .ok_or("The experiment was not saved.")?;
            store.save_findings(id, &diagnostics::evaluate(&stored, &ctx))?;
            store
                .load_experiment(id)?
                .ok_or_else(|| "The experiment was not saved.".to_string())
        })
        .await?;
        Ok(RunDone {
            experiment: Box::new(experiment),
            notice: notice.or(store_error),
        })
    }
}

/// Everything one measurement needs, owned so it can run as a task.
struct Measure {
    managed: Arc<LiveManaged>,
    request: LabRequest,
    active: managed::Active,
    profile: workstation_core::runtime::profile::LaunchProfile,
    hardware: HardwareProfile,
    model_sha256: String,
    /// Alias, process id and port of the server to measure.
    target: (String, u32, u16),
    follow_up: Option<(i64, Variable)>,
    cancel: Arc<AtomicBool>,
    progress: watch::Sender<Progress>,
}

impl Measure {
    async fn run(self) -> Result<(i64, Option<String>), String> {
        let Self {
            managed,
            request,
            active,
            profile,
            hardware,
            model_sha256,
            target: (alias, pid, port),
            follow_up,
            cancel,
            progress,
        } = self;
        let workload = Workload::standard();
        let spec = NewExperiment {
            engine: Engine::Llamacpp,
            ownership: Ownership::Managed,
            cold: request.cold,
            passes_requested: request.passes,
            workload_id: workload.id().into(),
            prompt_sha256: workload.prompt_sha256().into(),
            seed: workload.seed(),
            max_tokens: workload.max_tokens(),
            model_name: Path::new(&profile.model_path)
                .file_name()
                .map_or_else(|| "model".into(), |n| n.to_string_lossy().into_owned()),
            model_sha256: Some(model_sha256),
            runtime_version: Some(active.install.version.clone()),
            backend: Some(active.install.backend.clone()),
            hardware_hash: hardware.profile_hash.clone(),
            requested: requested_settings(&profile),
            variable: follow_up.map(|(_, v)| v),
            baseline_id: follow_up.map(|(id, _)| id),
        };
        let client = LlamaClient::new(port).map_err(|e| e.to_string())?;
        // Each restart is only allowed from the generation the previous one produced.
        let current = Arc::new(Mutex::new(alias.clone()));
        let mut target = LlamaTarget::new(
            client,
            alias,
            i32::try_from(pid).ok(),
            Box::new(move |cancel| {
                let managed = managed.clone();
                let current = current.clone();
                Box::pin(async move {
                    let expected = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let restarted = managed
                        .cold_restart(&Expect::Generation(expected), &cancel)
                        .await?;
                    *current.lock().unwrap_or_else(|e| e.into_inner()) = restarted.alias.clone();
                    Ok(ColdLoaded {
                        ready_in: restarted.ready_in,
                        pgid: i32::try_from(restarted.pid).ok(),
                        alias: restarted.alias,
                        port: restarted.port,
                    })
                })
            }),
        );
        let db = Db::new(
            blocking(|| {
                let store = open_store()?;
                recover_once(&store);
                Ok(store)
            })
            .await?,
        );
        let outcome = Box::pin(lab::run(
            db,
            &hardware,
            &mut target,
            spec,
            &workload,
            cancel,
            &progress,
        ))
        .await;
        match outcome.experiment_id {
            Some(id) => Ok((id, outcome.store_error)),
            None => Err(outcome
                .failure
                .unwrap_or_else(|| "The run could not start.".into())),
        }
    }
}

/// Shared-memory machines are where spill between pools is not meaningful; unknown stays
/// unknown, so a machine with no GPU entry is not claimed to be either.
fn unified_memory(hardware: &HardwareProfile) -> Option<bool> {
    let reports: Vec<Option<bool>> = hardware.gpus.iter().map(|g| g.unified_memory).collect();
    if reports.contains(&Some(true)) {
        Some(true)
    } else if !reports.is_empty() && reports.iter().all(|r| *r == Some(false)) {
        Some(false)
    } else {
        None
    }
}

type ModelFacts = (String, Option<u32>);

/// File digest (cached by path, size and modification time) and the model's block count.
fn model_facts(path: &Path) -> Result<ModelFacts, String> {
    static CACHE: Mutex<Option<(u64, String)>> = Mutex::new(None);
    // Look before opening: opening a FIFO would block this worker for good.
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Err("The model path is not a regular file.".into());
    }
    let file = std::fs::File::open(path).map_err(|_| "The model file cannot be opened.")?;
    let meta = file
        .metadata()
        .map_err(|_| "The model file cannot be read.")?;
    if !meta.is_file() {
        return Err("The model path is not a regular file.".into());
    }
    let mut key = DefaultHasher::new();
    path.hash(&mut key);
    meta.len().hash(&mut key);
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
        .hash(&mut key);
    let key = key.finish();
    let cached = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(k, _)| *k == key)
        .map(|(_, sha)| sha.clone());
    let sha = match cached {
        Some(sha) => sha,
        None => {
            let mut hasher = Sha256::new();
            let mut buffer = vec![0u8; 1 << 20];
            let mut reader = file;
            loop {
                let n = reader
                    .read(&mut buffer)
                    .map_err(|_| "The model file cannot be read.")?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            let sha = format!("{:x}", hasher.finalize());
            *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, sha.clone()));
            sha
        }
    };
    let layers = inspect_file(path)
        .ok()
        .and_then(|m| m.attention)
        .and_then(|a| a.block_count);
    Ok((sha, layers))
}

pub fn live() -> Arc<dyn LabBackend> {
    Arc::new(LiveLab::new())
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::time::Duration;

    /// In-memory backend: nothing touches a process, the store or the network.
    #[derive(Default)]
    pub(crate) struct FakeLab {
        pub calls: Mutex<Vec<String>>,
        pub listing: Mutex<Listing>,
        pub stored: Mutex<Vec<Experiment>>,
        pub check_error: Mutex<Option<String>>,
        /// A run that only ends when cancelled.
        pub hang: bool,
        /// A run whose backend panics.
        pub panic: bool,
    }

    impl LabBackend for FakeLab {
        fn list(&self) -> BoxFuture<Result<Listing, String>> {
            self.calls.lock().unwrap().push("list".into());
            let listing = self.listing.lock().unwrap().clone();
            Box::pin(async move { Ok(listing) })
        }
        fn load(&self, experiment: i64) -> BoxFuture<Result<Option<Experiment>, String>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("load {experiment}"));
            let found = self
                .stored
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.id == experiment)
                .cloned();
            Box::pin(async move { Ok(found) })
        }
        fn delete(&self, experiment: i64) -> BoxFuture<Result<(), String>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("delete {experiment}"));
            self.stored.lock().unwrap().retain(|e| e.id != experiment);
            Box::pin(async { Ok(()) })
        }
        fn check(&self, request: LabRequest) -> BoxFuture<Result<Checked, String>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("check {}", request.passes));
            let error = self.check_error.lock().unwrap().clone();
            Box::pin(async move {
                match error {
                    Some(e) => Err(e),
                    None => Ok(Checked {
                        description: format!("Runs {} passes.", request.passes),
                        server: "fake".into(),
                    }),
                }
            })
        }
        fn run(
            &self,
            request: LabRequest,
            cancel: Arc<AtomicBool>,
            progress: watch::Sender<Progress>,
        ) -> BoxFuture<Result<RunDone, String>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("run {}", request.passes));
            let (hang, panic) = (self.hang, self.panic);
            let stored = self.stored.lock().unwrap().first().cloned();
            Box::pin(async move {
                assert!(!panic, "injected backend panic");
                progress.send_replace(Progress {
                    phase: lab::Phase::Measured,
                    done: 1,
                    of: request.passes,
                });
                if hang {
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return Err("The run was cancelled.".into());
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
                stored
                    .map(|e| RunDone {
                        experiment: Box::new(e),
                        notice: None,
                    })
                    .ok_or_else(|| "No fixture.".to_string())
            })
        }
    }
}

#[cfg(test)]
mod acceptance;
#[cfg(test)]
mod tests;
