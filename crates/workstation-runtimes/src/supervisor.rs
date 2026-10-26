// SPDX-License-Identifier: GPL-3.0-or-later
//! Supervision of the one managed `llama-server` process.
//!
//! One Tokio task owns the child and a small state machine. The rest of the application
//! talks to it through a handle: commands in, a `watch` status out, and a bounded log
//! buffer. The task signals only the child it spawned (never a pid read from anywhere) and
//! only while that child is un-reaped, and it always ends a stop in `Stopped` or `Failed`.
//!
//! Readiness is an identity check, not a liveness check: `/health` must answer 200 **and**
//! `/v1/models` must list this launch's random alias **and** the child must still be alive.

pub mod log;
pub mod record;

use crate::llamacpp::install::prepare_root;
use log::LogBuffer;
use reqwest::Client;
use rustix::process::{Pid, Signal, WaitIdOptions, kill_process_group, waitid};
use std::{
    fs::{self, File},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep_until},
};
use workstation_core::runtime::{
    RuntimeLifecycleState,
    profile::{self, LaunchProfile, valid_name},
    random_hex,
};

pub const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(180);
pub const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(5);
const STARTING_POLL: Duration = Duration::from_millis(500);
const READY_POLL: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const UNRESPONSIVE_AFTER: u32 = 6;
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(600);
const FAILURE_LINES: usize = 50;
const MAX_PROBE_BODY: usize = 64 * 1024;
/// Environment passed to the child. Everything else, including `LLAMA_ARG_*` overrides and
/// Hugging Face tokens, is dropped.
const ENV_ALLOW: [&str; 8] = [
    "HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "LANG",
    "LC_ALL",
    "CUDA_VISIBLE_DEVICES",
];

#[derive(Clone, Debug)]
pub struct StartSpec {
    /// `runtimes/llamacpp`, the directory `llamacpp::install` publishes into.
    pub install_root: PathBuf,
    /// The installed build to run, e.g. `b11905-cpu`.
    pub dir_name: String,
    pub profile: LaunchProfile,
    pub start_timeout: Duration,
    pub stop_grace: Duration,
    /// Restart after an unexpected exit: at most three in ten minutes, with backoff.
    pub auto_restart: bool,
}

impl StartSpec {
    pub fn new(install_root: PathBuf, dir_name: String, profile: LaunchProfile) -> Self {
        Self {
            install_root,
            dir_name,
            profile,
            start_timeout: DEFAULT_START_TIMEOUT,
            stop_grace: DEFAULT_STOP_GRACE,
            auto_restart: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    InvalidProfile,
    BinaryMissing,
    ModelUnreadable,
    PortInUse,
    SpawnFailed,
    ExitedEarly,
    StartTimeout,
    Crashed,
    Unresponsive,
    StuckOnStop,
}

impl FailureKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::InvalidProfile => "The launch profile is not valid.",
            Self::BinaryMissing => "The installed llama-server was not found or is not usable.",
            Self::ModelUnreadable => "The model file is missing or cannot be read.",
            Self::PortInUse => {
                "The port is already in use. Choose another port or stop what is using it."
            }
            Self::SpawnFailed => "llama-server could not be started.",
            Self::ExitedEarly => "llama-server exited while starting.",
            Self::StartTimeout => "llama-server did not become ready in time.",
            Self::Crashed => "llama-server stopped unexpectedly.",
            Self::Unresponsive => "llama-server stopped answering and was stopped.",
            Self::StuckOnStop => "llama-server did not exit when asked to stop.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub exit_code: Option<i32>,
    /// The newest log lines at the time of failure.
    pub last_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: RuntimeLifecycleState,
    pub generation: Option<String>,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub failure: Option<Failure>,
    /// Automatic restarts performed since the last explicit start.
    pub restarts: u32,
}

impl Status {
    fn stopped() -> Self {
        Self {
            state: RuntimeLifecycleState::Stopped,
            generation: None,
            pid: None,
            port: None,
            failure: None,
            restarts: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupervisorError {
    /// A process is already running or starting.
    AlreadyRunning,
    /// Refused before anything was spawned; nothing is running.
    Refused(FailureKind),
    /// The supervisor task has ended.
    Gone,
    /// Another application instance already supervises the runtime.
    Busy,
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("A llama-server is already running."),
            Self::Refused(kind) => f.write_str(kind.describe()),
            Self::Gone => f.write_str("The runtime supervisor is not running."),
            Self::Busy => f.write_str("Another instance of the application manages the runtime."),
        }
    }
}
impl std::error::Error for SupervisorError {}

enum Cmd {
    Start(Box<StartSpec>, oneshot::Sender<Result<(), SupervisorError>>),
    Stop(oneshot::Sender<()>),
    Restart(oneshot::Sender<Result<(), SupervisorError>>),
}

/// The handle. Dropping it stops the child (the task sees the channel close).
pub struct Supervisor {
    tx: mpsc::Sender<Cmd>,
    status: watch::Receiver<Status>,
    logs: LogBuffer,
    task: Option<JoinHandle<()>>,
    lock: std::sync::Arc<record::ProcessLock>,
    record_path: PathBuf,
}

impl Supervisor {
    /// `record_path` is the private file kept while a child runs (see `record`). Fails with
    /// `Busy` when another instance holds it. Call `reclaim_orphan` before the first start.
    pub async fn spawn(record_path: PathBuf) -> Result<Self, SupervisorError> {
        let for_lock = record_path.clone();
        let lock = tokio::task::spawn_blocking(move || record::ProcessLock::acquire(&for_lock))
            .await
            .ok()
            .flatten()
            .ok_or(SupervisorError::Busy)?;
        let lock = std::sync::Arc::new(lock);
        let (tx, rx) = mpsc::channel(8);
        let (status_tx, status) = watch::channel(Status::stopped());
        let logs = LogBuffer::default();
        let task = tokio::spawn(actor(
            rx,
            status_tx,
            logs.clone(),
            record_path.clone(),
            std::sync::Arc::clone(&lock),
        ));
        Ok(Self {
            tx,
            status,
            logs,
            task: Some(task),
            lock,
            record_path,
        })
    }

    /// Terminate a recorded orphan left by a crashed run of the application, when provably
    /// ours (see `record::reclaim`). Call once, before the first start.
    pub async fn reclaim_orphan(&self, install_root: &Path) -> record::Reclaim {
        record::reclaim(
            &self.lock,
            &self.record_path,
            install_root,
            DEFAULT_STOP_GRACE,
        )
        .await
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    pub fn logs(&self) -> LogBuffer {
        self.logs.clone()
    }

    /// Returns once the process is spawned (or refused); readiness arrives through `status`.
    pub async fn start(&self, spec: StartSpec) -> Result<(), SupervisorError> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Cmd::Start(Box::new(spec), reply))
            .await
            .map_err(|_| SupervisorError::Gone)?;
        answer.await.map_err(|_| SupervisorError::Gone)?
    }

    /// Returns when the process has been stopped and reaped.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Cmd::Stop(reply))
            .await
            .map_err(|_| SupervisorError::Gone)?;
        answer.await.map_err(|_| SupervisorError::Gone)
    }

    pub async fn restart(&self) -> Result<(), SupervisorError> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Cmd::Restart(reply))
            .await
            .map_err(|_| SupervisorError::Gone)?;
        answer.await.map_err(|_| SupervisorError::Gone)?
    }

    /// Stop the child and end the task; use at application exit.
    pub async fn shutdown(mut self) {
        let _ = self.stop().await;
        if let Some(task) = self.task.take() {
            drop(self.tx);
            let _ = task.await;
        }
    }
}

struct Run {
    child: Child,
    pid: Pid,
    alias: String,
    port: u16,
    spec: StartSpec,
    started: Instant,
    ready: bool,
    failed_probes: u32,
    probe: Option<JoinHandle<()>>,
    pumps: Vec<JoinHandle<()>>,
    next_probe: Instant,
    /// A stop could not reap the child. It stays owned (and recorded) until it exits.
    stuck: bool,
}

/// Exited but not yet reaped: the leader stays a zombie, which keeps its pid and process
/// group number from being reused while the group is cleaned up.
fn exited(pid: Pid) -> bool {
    matches!(
        waitid(
            rustix::process::WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT
        ),
        Ok(Some(_))
    )
}

enum Health {
    Ready,
    Loading,
    Down,
}

struct State {
    _lock: std::sync::Arc<record::ProcessLock>,
    status_tx: watch::Sender<Status>,
    logs: LogBuffer,
    record_path: PathBuf,
    client: Client,
    run: Option<Run>,
    last_spec: Option<StartSpec>,
    restarts: Vec<Instant>,
    restart_at: Option<Instant>,
}

impl State {
    fn publish(&self, state: RuntimeLifecycleState, failure: Option<Failure>) {
        let (generation, pid, port) = self.run.as_ref().map_or((None, None, None), |r| {
            (
                Some(r.alias.clone()),
                Some(r.pid.as_raw_pid() as u32),
                Some(r.port),
            )
        });
        let restarts = self.status_tx.borrow().restarts;
        let _ = self.status_tx.send(Status {
            state,
            generation,
            pid,
            port,
            failure,
            restarts,
        });
    }

    fn fail(&mut self, kind: FailureKind, exit_code: Option<i32>) {
        let last_lines = self
            .logs
            .tail(FAILURE_LINES)
            .into_iter()
            .map(|l| l.text)
            .collect();
        self.publish(
            RuntimeLifecycleState::Failed,
            Some(Failure {
                kind,
                exit_code,
                last_lines,
            }),
        );
    }
}

async fn actor(
    mut rx: mpsc::Receiver<Cmd>,
    status_tx: watch::Sender<Status>,
    logs: LogBuffer,
    record_path: PathBuf,
    lock: std::sync::Arc<record::ProcessLock>,
) {
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()
        .expect("static client configuration");
    let mut state = State {
        _lock: lock,
        status_tx,
        logs,
        record_path,
        client,
        run: None,
        last_spec: None,
        restarts: Vec::new(),
        restart_at: None,
    };
    let (probe_tx, mut probe_rx) = mpsc::channel::<(String, Health)>(1);
    loop {
        let live = state.run.as_ref().filter(|r| !r.stuck);
        let next_probe = live.map(|r| r.next_probe);
        let deadline = live
            .filter(|r| !r.ready)
            .map(|r| r.started + r.spec.start_timeout);
        tokio::select! {
            command = rx.recv() => match command {
                None => {
                    stop_run(&mut state).await;
                    // A child that could not be reaped stays supervised for a while longer.
                    for _ in 0..30 {
                        if state.run.is_none() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        stop_run(&mut state).await;
                    }
                    break;
                }
                Some(Cmd::Start(spec, reply)) => {
                    let _ = reply.send(start(&mut state, *spec, true).await);
                }
                Some(Cmd::Stop(reply)) => {
                    state.restart_at = None;
                    stop_run(&mut state).await;
                    let _ = reply.send(());
                }
                Some(Cmd::Restart(reply)) => {
                    state.restart_at = None;
                    let spec = state.last_spec.clone();
                    stop_run(&mut state).await;
                    let result = match spec {
                        Some(spec) => start(&mut state, spec, true).await,
                        None => Err(SupervisorError::Refused(FailureKind::InvalidProfile)),
                    };
                    let _ = reply.send(result);
                }
            },
            () = wait_child(&state.run) => on_exit(&mut state).await,
            Some((alias, health)) = probe_rx.recv() => on_probe(&mut state, alias, health).await,
            () = sleep_until_opt(next_probe), if next_probe.is_some() => {
                schedule_probe(&mut state, &probe_tx);
            }
            () = sleep_until_opt(deadline), if deadline.is_some() => {
                stop_run(&mut state).await;
                if state.run.is_none() {
                    state.fail(FailureKind::StartTimeout, None);
                }
            }
            () = sleep_until_opt(state.restart_at), if state.restart_at.is_some() => {
                state.restart_at = None;
                if let Some(spec) = state.last_spec.clone() {
                    let _ = start(&mut state, spec, false).await;
                }
            }
        }
    }
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Resolves when the current child has exited (not yet reaped). Polled, because reaping
/// before the group is cleaned up would free its pid for reuse.
async fn wait_child(run: &Option<Run>) {
    let Some(run) = run else {
        return std::future::pending().await;
    };
    while !exited(run.pid) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Everything that touches the filesystem before a spawn; runs on a blocking thread.
fn preflight(
    spec: &StartSpec,
    alias: &str,
) -> Result<(PathBuf, PathBuf, Vec<String>, u16), FailureKind> {
    let mut profile = spec.profile.clone();
    // The model must be a readable regular file; the child gets its resolved path.
    match fs::canonicalize(&profile.model_path) {
        Ok(path) if path.is_file() && File::open(&path).is_ok() => {
            profile.model_path = path.to_string_lossy().into_owned();
        }
        _ => return Err(FailureKind::ModelUnreadable),
    }
    let args = profile
        .arguments(alias)
        .map_err(|_| FailureKind::InvalidProfile)?;
    let (program, dir) =
        resolve_program(&spec.install_root, &spec.dir_name).ok_or(FailureKind::BinaryMissing)?;
    // A conflict is reported, never resolved by signalling whatever holds the port.
    TcpListener::bind((profile::LOOPBACK, profile.port)).map_err(|_| FailureKind::PortInUse)?;
    Ok((program, dir, args, profile.port))
}

/// Validate, pre-flight and spawn. On success the state is `Starting`. An explicit start
/// (from the user) also resets the automatic-restart budget; a rejected one changes nothing.
async fn start(state: &mut State, spec: StartSpec, explicit: bool) -> Result<(), SupervisorError> {
    if state.run.is_some() {
        return Err(SupervisorError::AlreadyRunning);
    }
    if explicit {
        state.restarts.clear();
        state.status_tx.send_modify(|s| s.restarts = 0);
    }
    state.last_spec = Some(spec.clone());
    state.restart_at = None;
    state.logs.clear();
    let refuse = |state: &mut State, kind: FailureKind| {
        state.fail(kind, None);
        Err(SupervisorError::Refused(kind))
    };
    let Ok(alias) = random_hex(8) else {
        return refuse(state, FailureKind::SpawnFailed);
    };
    let checked = {
        let (spec, alias) = (spec.clone(), alias.clone());
        tokio::task::spawn_blocking(move || preflight(&spec, &alias)).await
    };
    let (program, dir, args, port) = match checked {
        Ok(Ok(ok)) => ok,
        Ok(Err(kind)) => return refuse(state, kind),
        Err(_) => return refuse(state, FailureKind::SpawnFailed),
    };
    let mut command = Command::new(&program);
    command
        .args(&args)
        .env_clear()
        .env("LD_LIBRARY_PATH", &dir)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    for name in ENV_ALLOW {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let Ok(mut child) = command.spawn() else {
        return refuse(state, FailureKind::SpawnFailed);
    };
    let Some(raw) = child.id() else {
        return refuse(state, FailureKind::SpawnFailed);
    };
    let pid = i32::try_from(raw)
        .ok()
        .and_then(Pid::from_raw)
        .expect("a live child has a positive pid");
    let mut pumps = Vec::new();
    if let Some(out) = child.stdout.take() {
        pumps.push(tokio::spawn(log::pump(out, state.logs.clone(), false)));
    }
    if let Some(err) = child.stderr.take() {
        pumps.push(tokio::spawn(log::pump(err, state.logs.clone(), true)));
    }
    let mut run = Run {
        child,
        pid,
        alias: alias.clone(),
        port,
        spec,
        started: Instant::now(),
        ready: false,
        failed_probes: 0,
        probe: None,
        pumps,
        next_probe: Instant::now() + STARTING_POLL,
        stuck: false,
    };
    // Recovery depends on this record: without it the child is torn down rather than left
    // running where a crash of the application could orphan it. A child that is already gone
    // has no start time to record and needs none.
    if let Some(ticks) = record::start_ticks(raw) {
        let (path, record) = (
            state.record_path.clone(),
            record::Record {
                pid: raw,
                start_ticks: ticks,
                exe: program,
                generation: alias,
            },
        );
        let saved = tokio::task::spawn_blocking(move || record::write(&path, &record)).await;
        if !matches!(saved, Ok(Ok(()))) {
            if teardown(&mut run).await {
                finish(&mut run, &state.record_path).await;
                return refuse(state, FailureKind::SpawnFailed);
            }
            // Cannot reap it: keep owning it (and its record, if any) rather than lose it.
            run.stuck = true;
            state.run = Some(run);
            state.fail(FailureKind::StuckOnStop, None);
            return Err(SupervisorError::Refused(FailureKind::StuckOnStop));
        }
    }
    state.run = Some(run);
    state.publish(RuntimeLifecycleState::Starting, None);
    Ok(())
}

/// TERM the group, wait out the grace period, then KILL it whatever the leader did, and
/// reap. The leader is only reaped at the end, so its pid and group stay ours throughout.
/// Returns false when the child could not be reaped.
async fn teardown(run: &mut Run) -> bool {
    let _ = kill_process_group(run.pid, Signal::TERM);
    let deadline = Instant::now() + run.spec.stop_grace;
    while !exited(run.pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Descendants that ignored TERM die here even if the leader already left.
    let _ = kill_process_group(run.pid, Signal::KILL);
    tokio::time::timeout(REAP_TIMEOUT, run.child.wait())
        .await
        .is_ok()
}

/// `llama-server` inside a build directory this application published: real directory,
/// real executable file, no links.
fn resolve_program(root: &Path, dir_name: &str) -> Option<(PathBuf, PathBuf)> {
    if !valid_name(dir_name) || dir_name.starts_with('.') || prepare_root(root).is_err() {
        return None;
    }
    let dir = root.join(dir_name);
    if !fs::symlink_metadata(&dir).ok()?.is_dir() {
        return None;
    }
    let program = dir.join("llama-server");
    let meta = fs::symlink_metadata(&program).ok()?;
    (meta.is_file() && meta.permissions().mode() & 0o100 != 0).then_some((program, dir))
}

fn schedule_probe(state: &mut State, probe_tx: &mpsc::Sender<(String, Health)>) {
    let Some(run) = state.run.as_mut() else {
        return;
    };
    run.next_probe = Instant::now() + if run.ready { READY_POLL } else { STARTING_POLL };
    if run.probe.as_ref().is_some_and(|p| !p.is_finished()) {
        return;
    }
    let (client, port, alias, tx) = (
        state.client.clone(),
        run.port,
        run.alias.clone(),
        probe_tx.clone(),
    );
    run.probe = Some(tokio::spawn(async move {
        let health = probe(&client, port, &alias).await;
        let _ = tx.send((alias, health)).await;
    }));
}

async fn probe(client: &Client, port: u16, alias: &str) -> Health {
    let base = format!("http://127.0.0.1:{port}");
    match client.get(format!("{base}/health")).send().await {
        Ok(r) if r.status().as_u16() == 200 => {}
        Ok(r) if r.status().as_u16() == 503 => return Health::Loading,
        _ => return Health::Down,
    }
    let Ok(mut response) = client.get(format!("{base}/v1/models")).send().await else {
        return Health::Down;
    };
    if response.status().as_u16() != 200 {
        return Health::Loading;
    }
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if body.len() + chunk.len() > MAX_PROBE_BODY {
            return Health::Down;
        }
        body.extend_from_slice(&chunk);
    }
    let listed = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("data")?
                .as_array()?
                .iter()
                .any(|m| m.get("id").and_then(|i| i.as_str()) == Some(alias))
                .then_some(())
        });
    if listed.is_some() {
        Health::Ready
    } else {
        Health::Down
    }
}

async fn on_probe(state: &mut State, alias: String, health: Health) {
    let Some(run) = state.run.as_mut() else {
        return;
    };
    if run.alias != alias || run.stuck {
        return; // a probe from an earlier launch, or for a child already being stopped
    }
    match health {
        Health::Ready => {
            if exited(run.pid) {
                return; // on_exit follows; a dead child is never Ready
            }
            run.failed_probes = 0;
            if !run.ready {
                run.ready = true;
                state.publish(RuntimeLifecycleState::Ready, None);
            }
        }
        Health::Loading if !run.ready => {}
        Health::Loading | Health::Down => {
            run.failed_probes += 1;
            if run.ready && run.failed_probes >= UNRESPONSIVE_AFTER {
                stop_run(state).await;
                if state.run.is_none() {
                    state.fail(FailureKind::Unresponsive, None);
                }
            }
        }
    }
}

async fn on_exit(state: &mut State) {
    let Some(mut run) = state.run.take() else {
        return;
    };
    // Clean up anything the leader left behind, then reap; the leader's pid is still ours.
    let _ = kill_process_group(run.pid, Signal::KILL);
    let status = tokio::time::timeout(REAP_TIMEOUT, run.child.wait()).await;
    let code = status.ok().and_then(|s| s.ok()).and_then(|s| s.code());
    let (was_ready, was_stuck) = (run.ready, run.stuck);
    finish(&mut run, &state.record_path).await;
    if was_stuck {
        // The user asked for this stop; it has now completed.
        state.publish(RuntimeLifecycleState::Stopped, None);
        return;
    }
    let kind = if was_ready {
        FailureKind::Crashed
    } else {
        FailureKind::ExitedEarly
    };
    state.fail(kind, code);
    if !state.last_spec.as_ref().is_some_and(|s| s.auto_restart) {
        return;
    }
    let now = Instant::now();
    state
        .restarts
        .retain(|t| now.duration_since(*t) < RESTART_WINDOW);
    if state.restarts.len() < MAX_RESTARTS {
        let backoff = Duration::from_secs(1 << state.restarts.len());
        state.restarts.push(now);
        state.restart_at = Some(now + backoff);
        let count = u32::try_from(state.restarts.len()).unwrap_or(u32::MAX);
        state.status_tx.send_modify(|s| s.restarts = count);
    }
}

/// Stop the current child, if any. Ends in `Stopped`, or in `Failed(StuckOnStop)` with the
/// child still owned and recorded when it cannot be reaped; a later stop retries.
async fn stop_run(state: &mut State) {
    if state.run.is_none() {
        if state.status_tx.borrow().state != RuntimeLifecycleState::Failed {
            state.publish(RuntimeLifecycleState::Stopped, None);
        }
        return;
    }
    state.publish(RuntimeLifecycleState::Stopping, None);
    let mut run = state.run.take().expect("checked above");
    if let Some(probe) = run.probe.take() {
        probe.abort();
    }
    if teardown(&mut run).await {
        finish(&mut run, &state.record_path).await;
        state.publish(RuntimeLifecycleState::Stopped, None);
    } else {
        run.stuck = true;
        state.run = Some(run);
        state.fail(FailureKind::StuckOnStop, None);
    }
}

/// Release what a finished run holds: the probe, the log readers (given a moment to drain
/// the last lines), and the on-disk record.
async fn finish(run: &mut Run, record_path: &Path) {
    if let Some(probe) = run.probe.take() {
        probe.abort();
    }
    for mut pump in run.pumps.drain(..) {
        if tokio::time::timeout(Duration::from_secs(1), &mut pump)
            .await
            .is_err()
        {
            pump.abort();
        }
    }
    record::remove(record_path);
}
