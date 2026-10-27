// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in live acceptance of the production lab path: the real managed supervisor, a real
//! `llama-server`, a real model and the real experiment store, with no GTK. Run by hand to
//! record evidence; it needs an isolated data directory so the user's data is never touched:
//!
//! ```text
//! KVLM_ACCEPTANCE_ISOLATED=1 XDG_DATA_HOME=<empty dir> \
//!   KVLM_ACCEPTANCE_INSTALL=<dir holding llama-server and its libraries> \
//!   KVLM_ACCEPTANCE_MODEL=<model.gguf> \
//!   cargo test --locked -p workstation-app lab::acceptance -- --ignored --nocapture
//! ```

use super::*;
use crate::managed::ManagedBackend;
use workstation_core::{
    db::profiles::InstallRow,
    runtime::{
        RuntimeLifecycleState,
        profile::{CacheType, LaunchProfile},
    },
};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_dir(&entry.path(), &target);
        } else if kind.is_symlink() {
            let link = std::fs::read_link(entry.path()).unwrap();
            let _ = std::fs::remove_file(&target);
            std::os::unix::fs::symlink(link, &target).unwrap();
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

async fn wait_ready(managed: &LiveManaged) -> managed::Active {
    for _ in 0..600 {
        if let Some((active, status)) = managed.running().await
            && matches!(
                status.state,
                RuntimeLifecycleState::Ready | RuntimeLifecycleState::Busy
            )
        {
            return active;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the managed server did not become ready");
}

async fn run(lab: &LiveLab, request: LabRequest) -> RunDone {
    let checked = lab.check(request.clone()).await.expect("check passes");
    println!("CHECK: {}", checked.description);
    let (progress, _seen) = watch::channel(Progress::default());
    lab.run(
        LabRequest {
            server: Some(checked.server),
            ..request
        },
        Arc::new(AtomicBool::new(false)),
        progress,
    )
    .await
    .expect("the run finishes")
}

fn show(done: &RunDone) {
    let e = &done.experiment;
    println!(
        "EXPERIMENT #{} {:?} cold={} passes={}",
        e.id,
        e.state,
        e.spec.cold,
        e.passes.len()
    );
    println!("  requested: {:?}", e.spec.requested);
    println!("  observed:  {:?}", e.observed);
    println!("  summary:   {:?}", e.summary);
    for f in &e.findings {
        println!("  {} {:?}: {}", f.rule_id, f.status, f.message);
    }
    println!("  notice: {:?}", done.notice);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live acceptance: needs KVLM_ACCEPTANCE_ISOLATED=1, an isolated XDG_DATA_HOME, an installed llama-server and a model"]
async fn live_cold_run_follow_up_and_comparison() {
    assert_eq!(
        std::env::var("KVLM_ACCEPTANCE_ISOLATED").as_deref(),
        Ok("1"),
        "refusing to run against real user data"
    );
    let model = std::env::var("KVLM_ACCEPTANCE_MODEL").expect("KVLM_ACCEPTANCE_MODEL");
    let install = std::env::var("KVLM_ACCEPTANCE_INSTALL").expect("KVLM_ACCEPTANCE_INSTALL");
    let data = std::env::var("XDG_DATA_HOME").expect("XDG_DATA_HOME");
    assert!(
        std::fs::read_dir(&data).map_or(true, |mut d| d.next().is_none()),
        "the data directory must start empty"
    );

    // Lay out one installed build the way the installer would have.
    let store = open_store().unwrap();
    let data_dir = workstation_core::db::default_database_path()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    copy_dir(
        Path::new(&install),
        &data_dir.join("runtimes/llamacpp/b-accept-cpu"),
    );
    store
        .record_install(&InstallRow {
            version: "b-accept".into(),
            backend: "cpu".into(),
            dir_name: "b-accept-cpu".into(),
            sha256: "0".repeat(64),
            installed_at_unix_ms: 1,
        })
        .unwrap();
    let profile = LaunchProfile {
        name: "accept".into(),
        model_path: model,
        port: free_port(),
        context_length: 1024,
        gpu_layers: 0,
        cache_type_k: CacheType::F16,
        cache_type_v: CacheType::F16,
        flash_attention: false,
        ubatch_size: 512,
        extra: vec![],
    };
    store.save_profile(&profile, 1).unwrap();
    drop(store);

    let managed = managed::live();
    managed
        .start("accept".into(), "b-accept-cpu".into())
        .await
        .expect("the managed server starts");
    wait_ready(&managed).await;
    let lab = LiveLab::new();

    // Nothing is refused for lack of a Check; a stale server token is.
    let stale = lab
        .run(
            LabRequest {
                passes: 3,
                cold: false,
                follow_up: None,
                server: Some("someone|else|1".into()),
            },
            Arc::new(AtomicBool::new(false)),
            watch::channel(Progress::default()).0,
        )
        .await;
    println!("STALE TOKEN: {:?}", stale.as_ref().err());
    assert!(stale.is_err(), "a Check of another server must not run");

    let baseline = run(
        &lab,
        LabRequest {
            passes: 3,
            cold: true,
            follow_up: None,
            server: None,
        },
    )
    .await;
    show(&baseline);
    assert_eq!(baseline.experiment.state, State::Completed);
    assert!(baseline.experiment.summary.is_some());
    assert_eq!(baseline.experiment.findings.len(), 4);

    let variant = run(
        &lab,
        LabRequest {
            passes: 3,
            cold: false,
            follow_up: Some((baseline.experiment.id, Variable::FlashAttentionOn)),
            server: None,
        },
    )
    .await;
    show(&variant);
    assert_eq!(variant.experiment.state, State::Completed);
    assert_eq!(
        variant.experiment.spec.requested.flash_attention,
        Some(true)
    );
    assert_eq!(
        variant
            .experiment
            .observed
            .as_ref()
            .and_then(|o| o.flash_attention),
        Some(true),
        "the running process's own command line shows the change took effect"
    );

    // The user's profile is back, unchanged.
    let active = wait_ready(&managed).await;
    assert_eq!(active.profile, profile, "the original profile was restored");

    let comparison = compare_stored(&lab, baseline.experiment.id, variant.experiment.id)
        .await
        .expect("a controlled comparison");
    println!("COMPARISON: {comparison:#?}");
    assert_eq!(comparison.controlled, Some(Variable::FlashAttentionOn));

    managed.shutdown().await;
}
