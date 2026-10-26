// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in live M2 acceptance run. It talks to api.github.com / github.com and runs the
//! installed `llama-server`. Tests never do either; this is run by hand to record evidence.
//!
//! ```text
//! cargo run --locked -p workstation-runtimes --example m2_acceptance -- <empty-work-dir> install [cpu|vulkan|cuda]
//! cargo run --locked -p workstation-runtimes --example m2_acceptance -- <work-dir> serve <model.gguf>
//! cargo run --locked -p workstation-runtimes --example m2_acceptance -- <work-dir> orphan-start <model.gguf>
//! cargo run --locked -p workstation-runtimes --example m2_acceptance -- <work-dir> orphan-reclaim
//! ```
//!
//! Everything lives under the work directory; the application's own data is not touched.
//! `serve` runs the supervisor against the installed `llama-server`: start, readiness, model
//! properties, a streamed completion, restart, a crash and automatic recovery, a port conflict
//! and a clean stop. `orphan-start` ends the process abruptly with the server still running, and
//! `orphan-reclaim` is the next launch finding and stopping it.

use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use workstation_core::runtime::{
    RuntimeLifecycleState as State,
    profile::{CacheType, LaunchProfile},
};
use workstation_runtimes::{
    llamacpp::{
        client::{ChatRequest, LlamaClient, StreamEvent},
        install::Installer,
        release::{Backend, detect_backends},
    },
    supervisor::{StartSpec, Status, Supervisor, record::Reclaim},
};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let work = PathBuf::from(args.next().expect("work directory"));
    let step = args.next().unwrap_or_default();
    let backend = args
        .next()
        .and_then(|b| Backend::parse(&b))
        .unwrap_or(Backend::Cpu);
    if !work.is_absolute() {
        panic!("work directory must be absolute");
    }
    match step.as_str() {
        "install" => install(&work, backend).await,
        "serve" => serve(&work, &model_arg()).await,
        "orphan-start" => orphan_start(&work, &model_arg()).await,
        "orphan-reclaim" => orphan_reclaim(&work).await,
        other => panic!("unknown step {other:?}"),
    }
}

async fn install(work: &std::path::Path, backend: Backend) {
    println!("backends offered on this machine: {:?}", detect_backends());
    let installer = Installer::new().expect("client");
    let found = installer
        .list_candidates(&[backend])
        .await
        .expect("release listing");
    let candidate = found.first().expect("no verified build for that backend");
    println!(
        "selected {} {} ({} archive(s), {} bytes)",
        candidate.tag,
        backend.as_str(),
        candidate.archives.len(),
        candidate.total_size()
    );
    let root = work.join("llamacpp");
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let installed = installer
        .install(&root, candidate, &cancel, |p| {
            println!(
                "  archive {}/{}: {}/{} bytes",
                p.archive + 1,
                p.archives,
                p.done,
                p.total
            );
        })
        .await
        .expect("install");
    println!(
        "installed {} sha256 {}",
        installed.dir_name, installed.sha256
    );
    println!(
        "server: {}",
        root.join(&installed.dir_name)
            .join("llama-server")
            .display()
    );
}

fn model_arg() -> PathBuf {
    std::env::args()
        .nth(3)
        .map(PathBuf::from)
        .expect("model path as the third argument")
}

/// The installed build directory name (the only entry that is not a staging directory).
fn installed_build(root: &Path) -> String {
    std::fs::read_dir(root)
        .expect("install root")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with('b') && !name.starts_with('.'))
        .expect("no installed build; run the install step first")
}

fn profile(model: &Path, port: u16) -> LaunchProfile {
    LaunchProfile {
        name: "acceptance".into(),
        model_path: model.to_string_lossy().into_owned(),
        port,
        context_length: 512,
        gpu_layers: 0,
        cache_type_k: CacheType::F16,
        cache_type_v: CacheType::F16,
        flash_attention: false,
        ubatch_size: 64,
        extra: vec![],
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

async fn wait_for(
    rx: &mut tokio::sync::watch::Receiver<Status>,
    what: &str,
    limit: Duration,
    ok: impl Fn(&Status) -> bool,
) -> Status {
    let started = Instant::now();
    let result = tokio::time::timeout(limit, async {
        loop {
            let status = rx.borrow_and_update().clone();
            println!(
                "  [{:>5.1}s] {:?} pid={:?} restarts={}",
                started.elapsed().as_secs_f32(),
                status.state,
                status.pid,
                status.restarts
            );
            if ok(&status) {
                return status;
            }
            rx.changed().await.expect("supervisor alive");
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn listeners(port: u16) {
    let out = std::process::Command::new("ss")
        .args(["-ltn", &format!("sport = :{port}")])
        .output();
    if let Ok(out) = out {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            println!("  ss: {line}");
        }
    }
}

async fn serve(work: &Path, model: &Path) {
    let root = work.join("llamacpp");
    let dir = installed_build(&root);
    let supervisor = Supervisor::spawn(work.join("supervisor.record"))
        .await
        .expect("supervisor");
    println!("orphan check: {:?}", supervisor.reclaim_orphan(&root).await);
    let mut rx = supervisor.subscribe();
    let port = free_port();
    let mut spec = StartSpec::new(root.clone(), dir.clone(), profile(model, port));
    spec.auto_restart = true;

    println!("== start {dir} on 127.0.0.1:{port}");
    supervisor.start(spec.clone()).await.expect("start");
    let ready = wait_for(&mut rx, "Ready", Duration::from_secs(120), |s| {
        s.state == State::Ready
    })
    .await;
    listeners(port);
    let client = LlamaClient::new(port).expect("client");
    let props = client.props().await.expect("props");
    println!(
        "props: n_ctx={:?} model={:?}",
        props.n_ctx, props.model_path
    );
    println!("slots: {:?}", client.slots().await.expect("slots"));
    println!("busy while idle: {:?}", client.busy().await);

    println!("== streamed completion");
    let alias = ready.generation.clone().expect("generation");
    let request = ChatRequest {
        model: &alias,
        prompt: "Once upon a time",
        max_tokens: 24,
        seed: 1,
        temperature: 0.0,
        cache_prompt: false,
    };
    let cancel = AtomicBool::new(false);
    let mut text = String::new();
    let end = client
        .stream_chat(&request, Duration::from_secs(60), &cancel, |event| {
            if let StreamEvent::Content { text: piece, .. } = event {
                text.push_str(piece);
            }
        })
        .await
        .expect("stream");
    println!("  text: {text:?}");
    println!(
        "  content events={} first={:?} elapsed={:?} finish={:?}",
        end.content_events, end.first_content_after, end.elapsed, end.finish_reason
    );
    println!("  usage={:?}\n  timings={:?}", end.usage, end.timings);
    assert!(end.content_events > 0, "no content streamed");

    println!("== restart");
    supervisor.restart().await.expect("restart");
    let again = wait_for(&mut rx, "Ready again", Duration::from_secs(120), |s| {
        s.state == State::Ready && s.generation != ready.generation
    })
    .await;
    assert_ne!(again.pid, ready.pid, "restart must start a new process");

    println!("== crash and automatic recovery (SIGKILL the server)");
    let pid = again.pid.expect("pid");
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
    let recovered = wait_for(&mut rx, "recovery", Duration::from_secs(180), |s| {
        s.state == State::Ready && s.pid != Some(pid) && s.restarts >= 1
    })
    .await;
    println!("  recovered with pid {:?}", recovered.pid);

    println!("== stop");
    let stopping = Instant::now();
    supervisor.stop().await.expect("stop");
    println!("  stopped in {:?}", stopping.elapsed());
    wait_for(&mut rx, "Stopped", Duration::from_secs(30), |s| {
        s.state == State::Stopped
    })
    .await;
    println!(
        "  port {port} free again: {}",
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    );

    println!("== port conflict");
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let busy_port = holder.local_addr().expect("addr").port();
    spec.profile.port = busy_port;
    println!(
        "  start on busy port {busy_port}: {:?}",
        supervisor.start(spec).await
    );
    drop(holder);
    supervisor.shutdown().await;
    println!("done");
}

async fn orphan_start(work: &Path, model: &Path) {
    let root = work.join("llamacpp");
    let dir = installed_build(&root);
    let supervisor = Supervisor::spawn(work.join("supervisor.record"))
        .await
        .expect("supervisor");
    let mut rx = supervisor.subscribe();
    let port = free_port();
    supervisor
        .start(StartSpec::new(root, dir, profile(model, port)))
        .await
        .expect("start");
    let ready = wait_for(&mut rx, "Ready", Duration::from_secs(120), |s| {
        s.state == State::Ready
    })
    .await;
    println!(
        "server pid {:?} on port {port}; ending abruptly without stopping it",
        ready.pid
    );
    std::fs::write(work.join("orphan.port"), port.to_string()).expect("note port");
    std::process::abort();
}

async fn orphan_reclaim(work: &Path) {
    let root = work.join("llamacpp");
    let port: u16 = std::fs::read_to_string(work.join("orphan.port"))
        .expect("run orphan-start first")
        .trim()
        .parse()
        .expect("port");
    println!(
        "before: port {port} free = {}",
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    );
    let supervisor = Supervisor::spawn(work.join("supervisor.record"))
        .await
        .expect("supervisor");
    let result = supervisor.reclaim_orphan(&root).await;
    println!("reclaim: {result:?}");
    assert_eq!(result, Reclaim::Reclaimed);
    println!(
        "after: port {port} free = {}",
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    );
    supervisor.shutdown().await;
}
