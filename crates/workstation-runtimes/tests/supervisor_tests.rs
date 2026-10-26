// SPDX-License-Identifier: GPL-3.0-or-later
//! Supervisor behaviour against a fake `llama-server` (see tests/support). No network beyond
//! loopback, no model, no GPU.

use rustix::process::{Pid, test_kill_process, test_kill_process_group};
use std::{
    fs,
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{sync::watch, time::timeout};
use workstation_core::runtime::{
    RuntimeLifecycleState as S,
    profile::{CacheType, LaunchProfile},
};
use workstation_runtimes::{
    llamacpp::install::prepare_root,
    supervisor::{
        FailureKind, StartSpec, Status, Supervisor, SupervisorError,
        log::{MAX_LINE, MAX_LINES},
        record::{self, Reclaim, Record},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const FAKE: &str = env!("CARGO_BIN_EXE_fake-llama-server");
const WAIT: Duration = Duration::from_secs(20);

struct Env {
    dir: PathBuf,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = PathBuf::from(format!(
            "{}/kvlm-sup-{}-{}",
            // Same filesystem as the fake binary, so it can be hard-linked into place.
            env!("CARGO_TARGET_TMPDIR"),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = dir.join("llamacpp");
        prepare_root(&root).unwrap();
        let build = root.join("b1-cpu");
        fs::create_dir(&build).unwrap();
        // A hard link is never opened for writing, so a concurrent fork cannot make exec
        // fail with ETXTBSY.
        fs::hard_link(FAKE, build.join("llama-server")).unwrap();
        Self { dir, root }
    }

    fn model(&self, mode: &str) -> String {
        let path = self.dir.join(format!("{mode}.gguf"));
        fs::write(&path, format!("{mode}\n")).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn record(&self) -> PathBuf {
        self.dir.join("managed.json")
    }

    fn spec(&self, mode: &str) -> StartSpec {
        let mut spec = StartSpec::new(
            self.root.clone(),
            "b1-cpu".into(),
            LaunchProfile {
                name: "t".into(),
                model_path: self.model(mode),
                port: free_port(),
                context_length: 2048,
                gpu_layers: 0,
                cache_type_k: CacheType::F16,
                cache_type_v: CacheType::F16,
                flash_attention: false,
                ubatch_size: 512,
                extra: vec![],
            },
        );
        spec.start_timeout = Duration::from_secs(15);
        spec.stop_grace = Duration::from_secs(2);
        spec
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A port nobody in this test process holds: a shared counter, so parallel tests never
/// hand out the same one. Occupied ports are skipped by a connect probe, never a bind: a
/// probe listener can leak into a concurrently forked child and make the real bind fail.
fn free_port() -> u16 {
    static NEXT_PORT: AtomicU64 = AtomicU64::new(0);
    // Below the kernel's ephemeral range (32768+), so outgoing test connections never hold it.
    let base = 12_000 + (std::process::id() as u64 % 15_000);
    loop {
        let port = (base + NEXT_PORT.fetch_add(1, Ordering::Relaxed)) as u16;
        if TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200))
            .is_err()
        {
            return port;
        }
    }
}

async fn until(rx: &mut watch::Receiver<Status>, ok: impl Fn(&Status) -> bool) -> Status {
    timeout(WAIT, async {
        loop {
            {
                let status = rx.borrow_and_update();
                if ok(&status) {
                    return status.clone();
                }
            }
            rx.changed().await.expect("supervisor alive");
        }
    })
    .await
    .expect("state reached in time")
}

fn alive(pid: u32) -> bool {
    Pid::from_raw(pid as i32).is_some_and(|p| test_kill_process(p).is_ok())
}

async fn gone(pid: u32) {
    for _ in 0..200 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} is still alive");
}

#[tokio::test]
async fn a_ready_server_is_identified_logged_recorded_and_stopped() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("serve")).await.unwrap();
    let ready = until(&mut rx, |s| s.state == S::Ready).await;
    let (pid, generation) = (ready.pid.unwrap(), ready.generation.clone().unwrap());
    assert_eq!(generation.len(), 16);
    assert!(alive(pid));
    let saved = record::read(&env.record()).expect("record while running");
    assert_eq!((saved.pid, saved.generation), (pid, generation));
    assert!(
        sup.logs()
            .tail(20)
            .iter()
            .any(|l| l.text.contains("host=Some(\"127.0.0.1\")")),
        "the child is always bound to loopback"
    );
    sup.stop().await.unwrap();
    assert_eq!(sup.status().state, S::Stopped);
    gone(pid).await;
    assert!(record::read(&env.record()).is_none());
}

#[tokio::test]
async fn states_move_in_order_through_start_and_stop() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    let initial = rx.borrow().state;
    let seen = tokio::spawn(async move {
        let mut states = vec![initial];
        while rx.changed().await.is_ok() {
            let state = rx.borrow().state;
            if states.last() != Some(&state) {
                states.push(state);
            }
            if state == S::Stopped && states.len() > 1 {
                break;
            }
        }
        states
    });
    sup.start(env.spec("serve")).await.unwrap();
    let mut rx2 = sup.subscribe();
    until(&mut rx2, |s| s.state == S::Ready).await;
    sup.stop().await.unwrap();
    let states = timeout(WAIT, seen).await.unwrap().unwrap();
    // `Stopping` is published and replaced within one wake-up, so a slow watcher may skip it.
    let states: Vec<_> = states.into_iter().filter(|s| *s != S::Stopping).collect();
    assert_eq!(states, [S::Stopped, S::Starting, S::Ready, S::Stopped]);
}

#[tokio::test]
async fn startup_failures_carry_the_exit_code_and_last_lines() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("exit_early")).await.unwrap();
    let failed = until(&mut rx, |s| s.state == S::Failed).await;
    let failure = failed.failure.unwrap();
    assert_eq!(failure.kind, FailureKind::ExitedEarly);
    assert_eq!(failure.exit_code, Some(3));
    assert!(failure.last_lines.iter().any(|l| l.contains("boom")));
    assert!(record::read(&env.record()).is_none());
}

#[tokio::test]
async fn refusals_spawn_nothing_and_name_the_reason() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();

    let holder = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut spec = env.spec("serve");
    spec.profile.port = holder.local_addr().unwrap().port();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::PortInUse))
    );
    drop(holder);

    let mut spec = env.spec("serve");
    spec.profile.model_path = env.dir.join("missing.gguf").to_string_lossy().into_owned();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::ModelUnreadable))
    );
    let mut spec = env.spec("serve");
    spec.profile.model_path = env.dir.to_string_lossy().into_owned();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::ModelUnreadable))
    );

    let mut spec = env.spec("serve");
    spec.dir_name = "nope".into();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::BinaryMissing))
    );
    let mut spec = env.spec("serve");
    spec.dir_name = "../b1-cpu".into();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::BinaryMissing))
    );

    let mut spec = env.spec("serve");
    spec.profile.port = 80;
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::InvalidProfile))
    );
    assert!(sup.status().pid.is_none());
}

#[tokio::test]
async fn a_symlinked_build_directory_or_binary_is_not_run() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    std::os::unix::fs::symlink(env.root.join("b1-cpu"), env.root.join("linked")).unwrap();
    let mut spec = env.spec("serve");
    spec.dir_name = "linked".into();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::BinaryMissing))
    );
    fs::create_dir(env.root.join("evil")).unwrap();
    std::os::unix::fs::symlink(
        env.root.join("b1-cpu/llama-server"),
        env.root.join("evil/llama-server"),
    )
    .unwrap();
    let mut spec = env.spec("serve");
    spec.dir_name = "evil".into();
    assert_eq!(
        sup.start(spec).await,
        Err(SupervisorError::Refused(FailureKind::BinaryMissing))
    );
}

#[tokio::test]
async fn a_second_start_is_refused_and_changes_nothing() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("serve")).await.unwrap();
    let ready = until(&mut rx, |s| s.state == S::Ready).await;
    assert_eq!(
        sup.start(env.spec("serve")).await,
        Err(SupervisorError::AlreadyRunning)
    );
    assert_eq!(sup.status().pid, ready.pid);
    sup.shutdown().await;
}

#[tokio::test]
async fn a_server_that_never_loads_times_out_and_is_killed() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    let mut spec = env.spec("loading");
    spec.start_timeout = Duration::from_millis(1500);
    sup.start(spec).await.unwrap();
    let starting = until(&mut rx, |s| s.state == S::Starting).await;
    let pid = starting.pid.unwrap();
    let failed = until(&mut rx, |s| s.state == S::Failed).await;
    assert_eq!(failed.failure.unwrap().kind, FailureKind::StartTimeout);
    gone(pid).await;
}

#[tokio::test]
async fn another_listener_answering_health_is_not_ready() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    let mut spec = env.spec("wrong_alias");
    spec.start_timeout = Duration::from_millis(2500);
    sup.start(spec).await.unwrap();
    let failed = until(&mut rx, |s| s.state == S::Failed || s.state == S::Ready).await;
    assert_eq!(
        failed.state,
        S::Failed,
        "a wrong alias must never look Ready"
    );
    assert_eq!(failed.failure.unwrap().kind, FailureKind::StartTimeout);
}

#[tokio::test]
async fn a_crash_after_ready_is_reported_with_its_exit_code() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("crash_after 4000")).await.unwrap();
    until(&mut rx, |s| s.state == S::Ready).await;
    let failed = until(&mut rx, |s| s.state == S::Failed).await;
    let failure = failed.failure.unwrap();
    assert_eq!(
        (failure.kind, failure.exit_code),
        (FailureKind::Crashed, Some(9))
    );
    assert!(record::read(&env.record()).is_none());
}

#[tokio::test]
async fn auto_restart_brings_a_crashed_server_back_with_a_new_identity() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    let mut spec = env.spec("crash_after 4000");
    spec.auto_restart = true;
    sup.start(spec).await.unwrap();
    let first = until(&mut rx, |s| s.state == S::Ready).await;
    until(&mut rx, |s| s.state == S::Failed).await;
    let second = until(&mut rx, |s| s.state == S::Ready && s.restarts == 1).await;
    assert_ne!(first.generation, second.generation);
    sup.shutdown().await;
}

#[tokio::test]
async fn restart_replaces_the_process() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("serve")).await.unwrap();
    let first = until(&mut rx, |s| s.state == S::Ready).await;
    sup.restart().await.unwrap();
    let second = until(&mut rx, |s| {
        s.state == S::Ready && s.generation != first.generation
    })
    .await;
    gone(first.pid.unwrap()).await;
    assert!(alive(second.pid.unwrap()));
    sup.shutdown().await;
    gone(second.pid.unwrap()).await;
}

#[tokio::test]
async fn a_child_that_ignores_sigterm_is_killed_with_its_whole_group() {
    let env = Env::new();
    let build = env.root.join("b1-cpu");
    fs::remove_file(build.join("llama-server")).unwrap();
    fs::write(
        build.join("llama-server"),
        "#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    fs::set_permissions(
        build.join("llama-server"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await; // let concurrent forks release the file
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    let mut spec = env.spec("serve");
    spec.stop_grace = Duration::from_millis(300);
    sup.start(spec).await.unwrap();
    let starting = until(&mut rx, |s| s.state == S::Starting).await;
    let pid = starting.pid.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await; // let the shell fork its sleep
    timeout(Duration::from_secs(10), sup.stop())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sup.status().state, S::Stopped);
    gone(pid).await;
    let group = Pid::from_raw(pid as i32).unwrap();
    for _ in 0..100 {
        if test_kill_process_group(group).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a member of the process group survived");
}

#[tokio::test]
async fn hostile_output_is_bounded_and_sanitized() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("noise")).await.unwrap();
    until(&mut rx, |s| s.state == S::Ready).await;
    let lines = sup.logs().tail(usize::MAX);
    assert!(lines.iter().any(|l| l.text.chars().count() == MAX_LINE + 1));
    assert!(
        lines
            .iter()
            .all(|l| l.text.chars().all(|c| c == '\t' || !c.is_control()))
    );
    sup.shutdown().await;
}

#[tokio::test]
async fn a_flooding_child_cannot_grow_memory_or_block_stop() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("flood")).await.unwrap();
    let ready = until(&mut rx, |s| s.state == S::Ready).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(sup.logs().tail(usize::MAX).len() <= MAX_LINES);
    timeout(Duration::from_secs(10), sup.stop())
        .await
        .unwrap()
        .unwrap();
    gone(ready.pid.unwrap()).await;
}

#[tokio::test]
async fn the_child_environment_is_cleared_to_an_allow_list() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("echo_env")).await.unwrap();
    until(&mut rx, |s| s.state == S::Ready).await;
    let allowed = [
        "HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "XDG_DATA_DIRS",
        "XDG_CONFIG_DIRS",
        "LANG",
        "LC_ALL",
        "CUDA_VISIBLE_DEVICES",
        "LD_LIBRARY_PATH",
    ];
    let mut seen = 0;
    for line in sup.logs().tail(usize::MAX) {
        if let Some(pair) = line.text.strip_prefix("env ") {
            seen += 1;
            let key = pair.split('=').next().unwrap();
            assert!(allowed.contains(&key), "unexpected variable {key}");
        }
    }
    assert!(seen >= 1, "LD_LIBRARY_PATH is always set");
    sup.shutdown().await;
}

#[tokio::test]
async fn dropping_the_handle_stops_the_child() {
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("serve")).await.unwrap();
    let pid = until(&mut rx, |s| s.state == S::Ready).await.pid.unwrap();
    drop(sup);
    gone(pid).await;
}

fn spawn_orphan(env: &Env, mode: &str) -> (std::process::Child, u32, Record) {
    use std::os::unix::process::CommandExt;
    let spec = env.spec(mode);
    let exe = env.root.join("b1-cpu/llama-server");
    let child = std::process::Command::new(&exe)
        .args(["--model", &spec.profile.model_path, "--port"])
        .arg(spec.profile.port.to_string())
        .args(["--alias", "deadbeefdeadbeef"])
        .env_clear()
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(150));
    let record = Record {
        pid,
        start_ticks: record::start_ticks(pid).unwrap(),
        exe,
        generation: "deadbeefdeadbeef".into(),
    };
    (child, pid, record)
}

#[tokio::test]
async fn an_orphan_from_a_crashed_run_is_reclaimed_only_when_provably_ours() {
    let env = Env::new();
    let grace = Duration::from_secs(2);
    let lock = record::ProcessLock::acquire(&env.record()).unwrap();

    assert_eq!(
        record::reclaim(&lock, &env.record(), &env.root, grace).await,
        Reclaim::Nothing
    );

    let (mut child, pid, good) = spawn_orphan(&env, "serve");
    // A reused pid (different start time) is never signalled.
    let mut stale = good.clone();
    stale.start_ticks += 1;
    record::write(&env.record(), &stale).unwrap();
    assert!(matches!(
        record::reclaim(&lock, &env.record(), &env.root, grace).await,
        Reclaim::Skipped(_)
    ));
    assert!(alive(pid));
    // A matching process whose executable is outside the install root is left alone.
    let outside = Env::new();
    record::write(&env.record(), &good).unwrap();
    assert!(matches!(
        record::reclaim(&lock, &env.record(), &outside.root, grace).await,
        Reclaim::Skipped(_)
    ));
    assert!(alive(pid));
    // A record that points at an unrelated executable is left alone.
    let mut wrong = good.clone();
    wrong.exe = PathBuf::from("/bin/sh");
    record::write(&env.record(), &wrong).unwrap();
    assert!(matches!(
        record::reclaim(&lock, &env.record(), Path::new("/bin"), grace).await,
        Reclaim::Skipped(_)
    ));
    assert!(alive(pid));

    record::write(&env.record(), &good).unwrap();
    assert_eq!(
        record::reclaim(&lock, &env.record(), &env.root, grace).await,
        Reclaim::Reclaimed
    );
    let _ = child.wait();
    gone(pid).await;
    assert!(record::read(&env.record()).is_none());
}

#[tokio::test]
async fn a_second_supervisor_cannot_share_the_record() {
    let env = Env::new();
    let first = Supervisor::spawn(env.record()).await.unwrap();
    assert!(matches!(
        Supervisor::spawn(env.record()).await,
        Err(SupervisorError::Busy)
    ));
    first.shutdown().await;
    Supervisor::spawn(env.record())
        .await
        .unwrap()
        .shutdown()
        .await;
}

#[tokio::test]
async fn descendants_are_killed_even_when_the_leader_exits_on_sigterm() {
    let env = Env::new();
    let build = env.root.join("b1-cpu");
    fs::remove_file(build.join("llama-server")).unwrap();
    fs::write(
        build.join("llama-server"),
        "#!/bin/sh\ntrap 'exit 0' TERM\n(trap '' TERM; while :; do sleep 1; done) &\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    fs::set_permissions(
        build.join("llama-server"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("serve")).await.unwrap();
    let pid = until(&mut rx, |s| s.state == S::Starting)
        .await
        .pid
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    sup.stop().await.unwrap();
    let group = Pid::from_raw(pid as i32).unwrap();
    for _ in 0..100 {
        if test_kill_process_group(group).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a descendant that ignored SIGTERM survived the stop");
}

#[tokio::test]
async fn a_child_that_cannot_be_recorded_is_not_left_running() {
    let env = Env::new();
    // A directory where the temporary record must be created makes the write fail.
    fs::create_dir(env.dir.join("managed.tmp")).unwrap();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let result = sup.start(env.spec("serve")).await;
    assert_eq!(
        result,
        Err(SupervisorError::Refused(FailureKind::SpawnFailed))
    );
    assert!(sup.status().pid.is_none());
    // Nothing is left running from that spawn.
    let out = std::process::Command::new("pgrep")
        .args([
            "-f",
            &format!("{}/llamacpp/b1-cpu/llama-server", env.dir.display()),
        ])
        .output();
    if let Ok(out) = out {
        assert!(out.stdout.is_empty(), "a child outlived a failed start");
    }
}

#[test]
fn a_fifo_in_place_of_the_record_does_not_hang_the_reader() {
    let env = Env::new();
    let path = env.record();
    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let made = std::process::Command::new("mkfifo").arg(&path).status();
    drop(c);
    if made.is_ok_and(|s| s.success()) {
        assert!(record::read(&path).is_none());
    }
}

#[tokio::test]
async fn a_managed_server_serves_a_streamed_completion_end_to_end() {
    use std::sync::atomic::AtomicBool;
    use workstation_runtimes::llamacpp::client::{ChatRequest, DEFAULT_DEADLINE, LlamaClient};
    let env = Env::new();
    let sup = Supervisor::spawn(env.record()).await.unwrap();
    let mut rx = sup.subscribe();
    sup.start(env.spec("serve")).await.unwrap();
    let ready = until(&mut rx, |s| s.state == S::Ready).await;
    let client = LlamaClient::new(ready.port.unwrap()).unwrap();
    assert_eq!(client.props().await.unwrap().n_ctx, Some(2048));
    assert_eq!(client.busy().await, Some(false));
    let alias = ready.generation.unwrap();
    let mut text = String::new();
    let end = client
        .stream_chat(
            &ChatRequest {
                model: &alias,
                prompt: "hi",
                max_tokens: 8,
                seed: 1,
                temperature: 0.0,
                cache_prompt: false,
            },
            DEFAULT_DEADLINE,
            &AtomicBool::new(false),
            |e| {
                if let workstation_runtimes::llamacpp::client::StreamEvent::Content {
                    text: t,
                    ..
                } = e
                {
                    text.push_str(t);
                }
            },
        )
        .await
        .unwrap();
    assert_eq!((text.as_str(), end.content_events), ("Hello from fake", 3));
    assert_eq!(end.usage.unwrap().completion_tokens, 3);
    sup.shutdown().await;
}
