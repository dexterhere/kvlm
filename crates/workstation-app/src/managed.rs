// SPDX-License-Identifier: GPL-3.0-or-later
//! The managed llama.cpp runtime as the interface sees it: install, profiles, one supervised
//! process. Everything here runs on Tokio; the interface only sees typed messages.
//!
//! Nothing starts at launch: the supervisor, its lock and the orphan check are created on the
//! first request, and nothing here touches the network except an explicit build listing or
//! install.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, atomic::AtomicBool},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::watch;
use workstation_core::{
    db::{HistoryStore, default_database_path, profiles::InstallRow},
    runtime::{RuntimeLifecycleState, profile::LaunchProfile},
};
use workstation_runtimes::{
    llamacpp::{
        client::{LlamaClient, Props},
        install::{Installer, Progress, remove_install},
        release::{Backend, Candidate, detect_backends},
    },
    supervisor::{StartSpec, Status, Supervisor, record::Reclaim},
};

use crate::bridge::BoxFuture;

/// Lines returned by one log read.
const LOG_LINES: usize = 500;
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub installs: Vec<InstallRow>,
    pub profiles: Vec<LaunchProfile>,
    /// Builds worth offering on this machine (reads loader files; runs nothing).
    pub backends: Vec<Backend>,
    /// Why supervision is unavailable (for example another instance manages the runtime).
    pub supervisor_error: Option<String>,
}

/// The supervisor's status plus what the running server reports about itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    pub status: Status,
    /// `Busy` is derived here from the server's own counters, never invented.
    pub busy: Option<bool>,
    pub props: Option<Props>,
}

impl Live {
    pub fn state(&self) -> RuntimeLifecycleState {
        if self.status.state == RuntimeLifecycleState::Ready && self.busy == Some(true) {
            RuntimeLifecycleState::Busy
        } else {
            self.status.state
        }
    }
}

/// Requests from the interface. Ids correlate a request with its answer so stale answers
/// can be dropped.
#[derive(Debug)]
pub enum ManagedCommand {
    Load {
        id: u64,
    },
    /// Explicit network listing of verified builds for these backends.
    Builds {
        id: u64,
        backends: Vec<Backend>,
    },
    Install {
        id: u64,
        candidate: Box<Candidate>,
    },
    CancelInstall,
    RemoveInstall {
        id: u64,
        dir_name: String,
    },
    SaveProfile {
        id: u64,
        profile: Box<LaunchProfile>,
    },
    DeleteProfile {
        id: u64,
        name: String,
    },
    Start {
        id: u64,
        profile: String,
        dir_name: String,
    },
    Stop {
        id: u64,
    },
    Restart {
        id: u64,
    },
    Logs {
        id: u64,
    },
}

#[derive(Debug)]
pub enum ManagedEvent {
    /// Answer to Load, RemoveInstall, SaveProfile and DeleteProfile.
    Snapshot {
        id: u64,
        result: Result<Box<Snapshot>, String>,
    },
    Builds {
        id: u64,
        result: Result<Vec<Candidate>, String>,
    },
    /// Latest reading only; older ones are dropped, never queued.
    InstallProgress(Progress),
    Installed {
        id: u64,
        result: Result<Box<Snapshot>, String>,
    },
    /// Answer to Start, Stop and Restart; the new state arrives as `Live`.
    Action {
        id: u64,
        result: Result<(), String>,
    },
    Live(Box<Live>),
    Logs {
        id: u64,
        lines: Vec<String>,
    },
}

pub trait ManagedBackend: Send + Sync + 'static {
    fn snapshot(&self) -> BoxFuture<Result<Snapshot, String>>;
    /// Explicit network listing of verified builds.
    fn builds(&self, backends: Vec<Backend>) -> BoxFuture<Result<Vec<Candidate>, String>>;
    fn install(
        &self,
        candidate: Candidate,
        cancel: Arc<AtomicBool>,
        progress: Box<dyn FnMut(Progress) + Send>,
    ) -> BoxFuture<Result<Snapshot, String>>;
    fn remove_install(&self, dir_name: String) -> BoxFuture<Result<Snapshot, String>>;
    fn save_profile(&self, profile: LaunchProfile) -> BoxFuture<Result<Snapshot, String>>;
    fn delete_profile(&self, name: String) -> BoxFuture<Result<Snapshot, String>>;
    fn start(&self, profile: String, dir_name: String) -> BoxFuture<Result<(), String>>;
    fn stop(&self) -> BoxFuture<Result<(), String>>;
    fn restart(&self) -> BoxFuture<Result<(), String>>;
    fn logs(&self) -> BoxFuture<Vec<String>>;
    /// Latest status; `None` until the supervisor exists. Coalesces by construction.
    fn live(&self) -> watch::Receiver<Option<Live>>;
    /// Stop the child and end supervision. Called once, at exit.
    fn shutdown(&self) -> BoxFuture<()>;
}

static LIVE: OnceLock<Arc<LiveManaged>> = OnceLock::new();

/// The one production backend, shared by the bridge and the exit path.
pub fn live() -> Arc<LiveManaged> {
    Arc::clone(LIVE.get_or_init(|| Arc::new(LiveManaged::new())))
}

/// Stop a managed child before the process exits. Blocks the caller (never a GTK callback
/// while the interface is in use), bounded by the stop grace plus a margin.
pub fn shutdown_blocking(runtime: &tokio::runtime::Handle) {
    if let Some(host) = LIVE.get() {
        let host = Arc::clone(host);
        let _ = runtime.block_on(tokio::time::timeout(
            Duration::from_secs(15),
            host.shutdown(),
        ));
    }
}

enum Sup {
    Unstarted,
    Up(Supervisor),
    Failed(String),
    Closed,
}

struct Core {
    sup: tokio::sync::Mutex<Sup>,
    logs: Mutex<Option<workstation_runtimes::supervisor::log::LogBuffer>>,
    live_tx: watch::Sender<Option<Live>>,
    live_rx: watch::Receiver<Option<Live>>,
    /// The build directory the current process runs from, so it is never removed.
    running_dir: Mutex<Option<String>>,
    forwarder: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct Paths {
    install_root: PathBuf,
    record: PathBuf,
}

fn paths() -> Result<Paths, String> {
    use std::os::unix::fs::DirBuilderExt;
    let data = default_database_path()?
        .parent()
        .map(PathBuf::from)
        .ok_or("The application data directory is unavailable.")?;
    let runtimes = data.join("runtimes");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&runtimes)
        .map_err(|_| "Cannot create the runtimes directory.".to_string())?;
    Ok(Paths {
        install_root: runtimes.join("llamacpp"),
        record: runtimes.join("llamacpp.record"),
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| "A background task failed. Retry.".to_string())?
}

fn open_store() -> Result<HistoryStore, String> {
    HistoryStore::open(&default_database_path()?)
}

fn read_snapshot() -> Result<Snapshot, String> {
    let store = open_store()?;
    Ok(Snapshot {
        installs: store.list_installs()?,
        profiles: store.list_profiles()?,
        backends: detect_backends(),
        supervisor_error: None,
    })
}

impl Core {
    fn new() -> Self {
        let (live_tx, live_rx) = watch::channel(None);
        Self {
            sup: tokio::sync::Mutex::new(Sup::Unstarted),
            logs: Mutex::new(None),
            live_tx,
            live_rx,
            running_dir: Mutex::new(None),
            forwarder: Mutex::new(None),
        }
    }

    /// Creates the supervisor on first use and checks for a leftover child of a crashed
    /// run (local files only). Returns the guard with the supervisor inside.
    async fn up(&self) -> Result<tokio::sync::MutexGuard<'_, Sup>, String> {
        let mut guard = self.sup.lock().await;
        match &*guard {
            Sup::Up(_) => return Ok(guard),
            Sup::Failed(message) => return Err(message.clone()),
            Sup::Closed => return Err("The application is shutting down.".into()),
            Sup::Unstarted => {}
        }
        let created = async {
            let paths = blocking(paths).await?;
            let supervisor = Supervisor::spawn(paths.record.clone())
                .await
                .map_err(|e| e.to_string())?;
            let reclaim = supervisor.reclaim_orphan(&paths.install_root).await;
            Ok::<_, String>((supervisor, reclaim))
        }
        .await;
        match created {
            Ok((supervisor, reclaim)) => {
                let logs = supervisor.logs();
                match reclaim {
                    Reclaim::Reclaimed => logs.push(
                        true,
                        "Stopped a llama-server left over from an earlier run.".into(),
                    ),
                    Reclaim::Skipped(why) => {
                        logs.push(true, format!("Left a recorded process alone: {why}."))
                    }
                    Reclaim::Nothing => {}
                }
                *self.logs.lock().unwrap_or_else(|e| e.into_inner()) = Some(logs);
                let task = tokio::spawn(forward(supervisor.subscribe(), self.live_tx.clone()));
                *self.forwarder.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
                *guard = Sup::Up(supervisor);
                Ok(guard)
            }
            Err(message) => {
                // A busy lock (another instance) can clear; do not cache that failure.
                if !message.contains("Another instance") {
                    *guard = Sup::Failed(message.clone());
                }
                Err(message)
            }
        }
    }
}

/// Copies supervisor status into the shared watch, adding the server's own busy flag and
/// properties while it is ready. Polls only while a process is live.
async fn forward(mut status: watch::Receiver<Status>, out: watch::Sender<Option<Live>>) {
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut props: Option<(String, Props)> = None;
    loop {
        let current = status.borrow_and_update().clone();
        let ready = current.state == RuntimeLifecycleState::Ready;
        let client = current
            .port
            .filter(|_| ready)
            .and_then(|port| LlamaClient::new(port).ok());
        let (busy, shown) = match (client, current.generation.as_deref()) {
            (Some(client), Some(generation)) => {
                if props.as_ref().is_none_or(|(g, _)| g != generation) {
                    props = client
                        .props()
                        .await
                        .ok()
                        .map(|p| (generation.to_owned(), p));
                }
                (client.busy().await, props.as_ref().map(|(_, p)| p.clone()))
            }
            _ => {
                props = None;
                (None, None)
            }
        };
        out.send_if_modified(|slot| {
            let next = Live {
                status: current,
                busy,
                props: shown,
            };
            if slot.as_ref() == Some(&next) {
                return false;
            }
            *slot = Some(next);
            true
        });
        tokio::select! {
            changed = status.changed() => if changed.is_err() { break },
            _ = tick.tick(), if ready => {}
        }
    }
}

/// The production backend. Cheap to clone; all state is shared.
pub struct LiveManaged(Arc<Core>);

impl Default for LiveManaged {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveManaged {
    pub fn new() -> Self {
        Self(Arc::new(Core::new()))
    }
}

macro_rules! delegate {
    ($self:ident, $name:ident($($arg:ident),*)) => {{
        let core = Arc::clone(&$self.0);
        Box::pin(async move { core.$name($($arg),*).await })
    }};
}

impl ManagedBackend for LiveManaged {
    fn snapshot(&self) -> BoxFuture<Result<Snapshot, String>> {
        delegate!(self, snapshot())
    }
    fn builds(&self, backends: Vec<Backend>) -> BoxFuture<Result<Vec<Candidate>, String>> {
        Box::pin(async move { builds(backends).await })
    }
    fn install(
        &self,
        candidate: Candidate,
        cancel: Arc<AtomicBool>,
        progress: Box<dyn FnMut(Progress) + Send>,
    ) -> BoxFuture<Result<Snapshot, String>> {
        Box::pin(async move { install(candidate, cancel, progress).await })
    }
    fn remove_install(&self, dir_name: String) -> BoxFuture<Result<Snapshot, String>> {
        delegate!(self, remove_install(dir_name))
    }
    fn save_profile(&self, profile: LaunchProfile) -> BoxFuture<Result<Snapshot, String>> {
        Box::pin(async move {
            blocking(move || {
                open_store()?.save_profile(&profile, now_ms())?;
                read_snapshot()
            })
            .await
        })
    }
    fn delete_profile(&self, name: String) -> BoxFuture<Result<Snapshot, String>> {
        Box::pin(async move {
            blocking(move || {
                open_store()?.delete_profile(&name)?;
                read_snapshot()
            })
            .await
        })
    }
    fn start(&self, profile: String, dir_name: String) -> BoxFuture<Result<(), String>> {
        delegate!(self, start(profile, dir_name))
    }
    fn stop(&self) -> BoxFuture<Result<(), String>> {
        delegate!(self, stop())
    }
    fn restart(&self) -> BoxFuture<Result<(), String>> {
        delegate!(self, restart())
    }
    fn logs(&self) -> BoxFuture<Vec<String>> {
        let logs = self
            .0
            .logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Box::pin(async move {
            logs.map(|logs| logs.tail(LOG_LINES).into_iter().map(|l| l.text).collect())
                .unwrap_or_default()
        })
    }
    fn live(&self) -> watch::Receiver<Option<Live>> {
        self.0.live_rx.clone()
    }
    fn shutdown(&self) -> BoxFuture<()> {
        delegate!(self, shutdown())
    }
}

async fn builds(backends: Vec<Backend>) -> Result<Vec<Candidate>, String> {
    Installer::new()
        .map_err(|e| e.to_string())?
        .list_candidates(&backends)
        .await
        .map_err(|e| e.to_string())
}

async fn install(
    candidate: Candidate,
    cancel: Arc<AtomicBool>,
    progress: Box<dyn FnMut(Progress) + Send>,
) -> Result<Snapshot, String> {
    let paths = blocking(paths).await?;
    let installed = Installer::new()
        .map_err(|e| e.to_string())?
        .install(&paths.install_root, &candidate, &cancel, progress)
        .await
        .map_err(|e| e.to_string())?;
    blocking(move || {
        open_store()?.record_install(&InstallRow {
            version: installed.tag,
            backend: installed.backend.as_str().into(),
            dir_name: installed.dir_name,
            sha256: installed.sha256,
            installed_at_unix_ms: now_ms(),
        })?;
        read_snapshot()
    })
    .await
}

impl Core {
    /// Reads the saved state and starts supervision, which also stops a child left over
    /// from a crashed run. Local files only.
    async fn snapshot(&self) -> Result<Snapshot, String> {
        let supervisor_error = self.up().await.err();
        let mut snapshot = blocking(read_snapshot).await?;
        snapshot.supervisor_error = supervisor_error;
        Ok(snapshot)
    }

    async fn remove_install(&self, dir_name: String) -> Result<Snapshot, String> {
        let running = self
            .running_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let live = self.live_rx.borrow().as_ref().map(|l| l.status.state);
        // A live process may still be using the build; name it so it is never removed.
        let in_use = live
            .is_some_and(RuntimeLifecycleState::is_live)
            .then_some(running)
            .flatten();
        let paths = blocking(paths).await?;
        blocking(move || {
            let store = open_store()?;
            let row = store
                .list_installs()?
                .into_iter()
                .find(|r| r.dir_name == dir_name)
                .ok_or("That build is not one of the installed runtimes.")?;
            remove_install(&paths.install_root, &dir_name, in_use.as_deref())
                .map_err(|e| e.to_string())?;
            store.delete_install(&row.version, &row.backend)?;
            read_snapshot()
        })
        .await
    }

    async fn start(&self, profile: String, dir_name: String) -> Result<(), String> {
        let (profile, row) = blocking(move || {
            let store = open_store()?;
            let profile = store
                .get_profile(&profile)?
                .ok_or("That launch profile no longer exists.")?;
            let row = store
                .list_installs()?
                .into_iter()
                .find(|r| r.dir_name == dir_name)
                .ok_or("That runtime build is not installed.")?;
            Ok((profile, row))
        })
        .await?;
        let paths = blocking(paths).await?;
        let guard = self.up().await?;
        let Sup::Up(supervisor) = &*guard else {
            return Err("The runtime supervisor is not available.".into());
        };
        let mut spec = StartSpec::new(paths.install_root, row.dir_name.clone(), profile);
        spec.auto_restart = true;
        supervisor.start(spec).await.map_err(|e| e.to_string())?;
        *self.running_dir.lock().unwrap_or_else(|e| e.into_inner()) = Some(row.dir_name);
        Ok(())
    }

    async fn stop(&self) -> Result<(), String> {
        let guard = self.up().await?;
        let Sup::Up(supervisor) = &*guard else {
            return Ok(());
        };
        supervisor.stop().await.map_err(|e| e.to_string())
    }

    async fn restart(&self) -> Result<(), String> {
        let guard = self.up().await?;
        let Sup::Up(supervisor) = &*guard else {
            return Err("The runtime supervisor is not available.".into());
        };
        supervisor.restart().await.map_err(|e| e.to_string())
    }

    async fn shutdown(&self) {
        let taken = std::mem::replace(&mut *self.sup.lock().await, Sup::Closed);
        if let Sup::Up(supervisor) = taken {
            supervisor.shutdown().await;
        }
        if let Some(task) = self
            .forwarder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use workstation_runtimes::supervisor::Status;

    /// An in-memory backend: nothing touches the filesystem, a process or the network.
    pub(crate) struct FakeManaged {
        pub calls: Mutex<Vec<String>>,
        snapshot: Mutex<Snapshot>,
        live_tx: watch::Sender<Option<Live>>,
        live_rx: watch::Receiver<Option<Live>>,
        /// An install that only ends when cancelled.
        pub slow_install: bool,
    }

    impl Default for FakeManaged {
        fn default() -> Self {
            let (live_tx, live_rx) = watch::channel(None);
            Self {
                calls: Mutex::default(),
                snapshot: Mutex::new(Snapshot {
                    installs: Vec::new(),
                    profiles: Vec::new(),
                    backends: vec![Backend::Cpu],
                    supervisor_error: None,
                }),
                live_tx,
                live_rx,
                slow_install: false,
            }
        }
    }

    impl FakeManaged {
        pub(crate) fn slow() -> Self {
            Self {
                slow_install: true,
                ..Self::default()
            }
        }
        fn note(&self, call: impl Into<String>) {
            self.calls.lock().unwrap().push(call.into());
        }
        fn set_state(&self, state: RuntimeLifecycleState) {
            let status = Status {
                state,
                generation: Some("abcdef01".into()),
                pid: Some(1),
                port: Some(8081),
                failure: None,
                restarts: 0,
            };
            self.live_tx.send_replace(Some(Live {
                status,
                busy: None,
                props: None,
            }));
        }
        fn current(&self) -> Snapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    impl ManagedBackend for FakeManaged {
        fn snapshot(&self) -> BoxFuture<Result<Snapshot, String>> {
            self.note("snapshot");
            let snapshot = self.current();
            Box::pin(async move { Ok(snapshot) })
        }
        fn builds(&self, _: Vec<Backend>) -> BoxFuture<Result<Vec<Candidate>, String>> {
            self.note("builds");
            Box::pin(async { Ok(Vec::new()) })
        }
        fn install(
            &self,
            candidate: Candidate,
            cancel: Arc<AtomicBool>,
            mut progress: Box<dyn FnMut(Progress) + Send>,
        ) -> BoxFuture<Result<Snapshot, String>> {
            self.note(format!("install {}", candidate.dir_name()));
            let (slow, snapshot) = (self.slow_install, self.current());
            Box::pin(async move {
                for done in [10, 20, 30] {
                    progress(Progress {
                        archive: 0,
                        archives: 1,
                        done,
                        total: 100,
                    });
                }
                if slow {
                    loop {
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err("The install was cancelled.".into());
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
                Ok(snapshot)
            })
        }
        fn remove_install(&self, dir_name: String) -> BoxFuture<Result<Snapshot, String>> {
            self.note(format!("remove {dir_name}"));
            let snapshot = self.current();
            Box::pin(async move { Ok(snapshot) })
        }
        fn save_profile(&self, profile: LaunchProfile) -> BoxFuture<Result<Snapshot, String>> {
            self.note(format!("save {}", profile.name));
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.profiles.retain(|p| p.name != profile.name);
            snapshot.profiles.push(profile);
            let copy = snapshot.clone();
            Box::pin(async move { Ok(copy) })
        }
        fn delete_profile(&self, name: String) -> BoxFuture<Result<Snapshot, String>> {
            self.note(format!("delete {name}"));
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.profiles.retain(|p| p.name != name);
            let copy = snapshot.clone();
            Box::pin(async move { Ok(copy) })
        }
        fn start(&self, profile: String, dir_name: String) -> BoxFuture<Result<(), String>> {
            self.note(format!("start {profile} {dir_name}"));
            let result = if profile == "missing" {
                Err("That launch profile no longer exists.".to_owned())
            } else {
                self.set_state(RuntimeLifecycleState::Ready);
                Ok(())
            };
            Box::pin(async move { result })
        }
        fn stop(&self) -> BoxFuture<Result<(), String>> {
            self.note("stop");
            self.set_state(RuntimeLifecycleState::Stopped);
            Box::pin(async { Ok(()) })
        }
        fn restart(&self) -> BoxFuture<Result<(), String>> {
            self.note("restart");
            Box::pin(async { Ok(()) })
        }
        fn logs(&self) -> BoxFuture<Vec<String>> {
            Box::pin(async { vec!["line one".into(), "line two".into()] })
        }
        fn live(&self) -> watch::Receiver<Option<Live>> {
            self.live_rx.clone()
        }
        fn shutdown(&self) -> BoxFuture<()> {
            self.note("shutdown");
            Box::pin(async {})
        }
    }
}
