// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK fixtures and explicit real-hardware acceptance flows.

use super::*;
use crate::bridge::{self, UiEvent};
use std::{
    env,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use workstation_core::{db::HistoryEntry, models::artifact::canonical_manifest_digest};
use workstation_runtimes::ollama::{
    Discovery, GenerationProgress, GenerationSummary, ModelDetails,
};

#[test]
#[ignore = "requires a graphical GTK display"]
fn quick_benchmark_selection_cancel_and_completion_flow() {
    adw::init().expect("GTK display is available");
    let (commands, mut command_rx) = mpsc::channel(1);
    let panel = Benchmarks::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(640)
        .child(&panel.widget)
        .build();
    window.present();
    assert!(wait_for_layout(&window));
    assert!(window.width() <= 360);
    assert!(!panel.run.is_sensitive(), "model choice must be explicit");

    panel.receive(&discovery());
    assert_eq!(panel.state.borrow().models.len(), 3, "cloud model excluded");
    panel.model.set_selected(2);
    assert!(!panel.run.is_sensitive(), "missing digest must block Run");
    panel.model.set_selected(1);
    assert!(panel.run.is_sensitive());
    let (verification_id, model_name, digest) = receive_verification(&mut command_rx);
    assert_eq!(model_name, "fixture:latest");
    assert_eq!(
        digest,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: verification_id,
        result: Ok(None),
    });
    panel
        .commands
        .try_send(CoreCommand::DetectHardware)
        .unwrap();
    panel.run.emit_clicked();
    assert!(panel.status.label().contains("Worker busy"));
    assert!(panel.state.borrow().in_flight.is_none());
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DetectHardware)
    ));
    panel.run.emit_clicked();
    let (id, cancellation) = receive_run(&mut command_rx);
    assert!(!*cancellation.borrow());
    assert!(!panel.run.is_sensitive());
    panel.cancel.emit_clicked();
    assert!(
        *cancellation.borrow(),
        "Cancel signals the worker out of band"
    );
    panel.receive(&UiEvent::BenchmarkFinished {
        id,
        outcome: BenchmarkOutcome::Cancelled(GenerationProgress {
            first_output_after: Some(Duration::from_millis(20)),
            response_chunks: 2,
        }),
        resources: None,
    });
    assert!(label_text(panel.widget.upcast_ref()).contains("Partial response chunks"));

    panel.run.emit_clicked();
    let (complete_id, _) = receive_run(&mut command_rx);
    panel.receive(&UiEvent::BenchmarkFinished {
        id: complete_id,
        outcome: BenchmarkOutcome::Completed(QuickBenchmarkReport {
            workload_id: "quick-text-v1",
            generation: GenerationSummary {
                model_name: "fixture:latest".into(),
                model_digest: "sha256:fixture".into(),
                runtime_version: "fixture-runtime".into(),
                first_output_after: Some(Duration::from_millis(250)),
                elapsed: Duration::from_secs(2),
                response_chunks: 8,
                eval_count: Some(64),
                eval_duration_ns: Some(1_000_000_000),
                load_duration_ns: Some(500_000_000),
                done_reason: "stop".into(),
            },
            generation_tokens_per_second: Some(64.0),
        }),
        resources: Some(Box::new(resource_fixture())),
    });
    let labels = label_text(panel.widget.upcast_ref());
    assert!(labels.contains("Client first response text (includes model load)"));
    assert!(labels.contains("250.00 ms"));
    assert!(labels.contains("64.00 tokens/s"));
    assert!(labels.contains("Server evaluation token count"));
    assert!(labels.contains("64"));
    assert!(labels.contains("System RAM nonavailable (system-wide peak): 4096 bytes"));
    assert!(
        labels.contains("GPU 1 (Fixture GPU) driver-reported VRAM peak: 2048 bytes · 4 samples")
    );
    assert!(labels.contains("GTT peak (overlaps system RAM): 1024 bytes · 3 samples"));
    assert!(labels.contains("lower-bound peaks"));
    drain_layout();
    assert!(
        window.width() <= 360,
        "memory results must fit narrow windows"
    );

    panel.run.emit_clicked();
    let (stale_id, stale_cancel) = receive_run(&mut command_rx);
    panel.model.set_selected(2);
    assert!(*stale_cancel.borrow());
    assert!(!panel.run.is_sensitive());
    panel.receive(&UiEvent::BenchmarkFinished {
        id: stale_id,
        outcome: BenchmarkOutcome::Completed(report_fixture()),
        resources: Some(Box::new(resource_fixture())),
    });
    assert!(!label_text(panel.widget.upcast_ref()).contains("Completed quick benchmark"));
    assert!(!panel.run.is_sensitive());

    panel.model.set_selected(1);
    assert!(panel.run.is_sensitive());
    let (verification_id, model_name, digest) = receive_verification(&mut command_rx);
    assert_eq!(model_name, "fixture:latest");
    assert_eq!(
        digest,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: verification_id,
        result: Ok(None),
    });
    panel.run.emit_clicked();
    let (inventory_id, inventory_cancel) = receive_run(&mut command_rx);
    panel.receive(&UiEvent::OllamaFailed("fixture discovery failure".into()));
    assert!(*inventory_cancel.borrow());
    assert!(panel.state.borrow().models.is_empty());
    assert!(!panel.run.is_sensitive());
    panel.receive(&UiEvent::BenchmarkFinished {
        id: inventory_id,
        outcome: BenchmarkOutcome::Completed(report_fixture()),
        resources: None,
    });
    assert!(!label_text(panel.widget.upcast_ref()).contains("Completed quick benchmark"));
    panel.receive(&discovery());
    assert_eq!(panel.state.borrow().models.len(), 3);
    assert!(
        !panel.run.is_sensitive(),
        "recovery still requires an explicit choice"
    );
    panel.model.set_selected(3);
    drain_layout();
    assert!(
        window.width() <= 360,
        "populated model selector grew to {} pixels",
        window.width()
    );
    panel.model.set_selected(1);
    drop(command_rx);
    panel.run.emit_clicked();
    assert!(panel.status.label().contains("Worker unavailable"));
    assert!(panel.state.borrow().in_flight.is_none());

    window.close();
    drop(panel);
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn verification_clears_stale_results_and_retry_can_recover() {
    adw::init().expect("GTK display is available");
    let (commands, mut command_rx) = mpsc::channel(1);
    let panel = Benchmarks::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(640)
        .child(&panel.widget)
        .build();
    window.present();
    assert!(wait_for_layout(&window));
    panel.receive(&discovery());
    panel.model.set_selected(1);
    let (first_id, _, _) = receive_verification(&mut command_rx);
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: first_id,
        result: Ok(Some(17)),
    });
    assert!(
        panel
            .verification_status
            .label()
            .contains("Benchmark Verified")
    );

    panel
        .commands
        .try_send(CoreCommand::DetectHardware)
        .unwrap();
    panel.receive(&UiEvent::HardwareDetected(Box::new(
        workstation_core::hardware::HardwareProfile {
            profile_hash: Some("fresh".into()),
            cpu: None,
            system_ram: None,
            gpus: vec![],
            issues: vec![],
            captured_at_unix_ms: 1,
        },
    )));
    assert!(panel.verification_status.label().contains("Worker busy"));
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DetectHardware)
    ));
    panel.check_verification.emit_clicked();
    let (busy_retry_id, _, _) = receive_verification(&mut command_rx);
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: busy_retry_id,
        result: Ok(Some(18)),
    });
    assert!(panel.verification_status.label().contains("run #18"));

    panel.receive(&UiEvent::HardwareDetected(Box::new(
        workstation_core::hardware::HardwareProfile {
            profile_hash: Some("fresh-again".into()),
            cpu: None,
            system_ram: None,
            gpus: vec![],
            issues: vec![],
            captured_at_unix_ms: 2,
        },
    )));
    let (stale_id, _, _) = receive_verification(&mut command_rx);
    assert!(
        !panel
            .verification_status
            .label()
            .contains("Benchmark Verified")
    );
    panel.receive(&UiEvent::HardwareFailed("fixture probe failure".into()));
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: stale_id,
        result: Ok(Some(18)),
    });
    assert!(
        !panel
            .verification_status
            .label()
            .contains("Benchmark Verified")
    );

    panel.check_verification.emit_clicked();
    let (retry_id, _, _) = receive_verification(&mut command_rx);
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: retry_id,
        result: Ok(Some(19)),
    });
    assert!(panel.verification_status.label().contains("run #19"));

    panel.model.set_selected(2);
    assert!(
        !panel
            .verification_status
            .label()
            .contains("Benchmark Verified")
    );
    panel.receive(&UiEvent::BenchmarkVerificationChecked {
        id: retry_id,
        result: Ok(Some(20)),
    });
    assert!(
        !panel
            .verification_status
            .label()
            .contains("Benchmark Verified")
    );

    window.close();
    drop(panel);
}

#[test]
#[ignore = "requires KVLM_ACCEPTANCE_MODEL, isolated XDG_DATA_HOME, local Ollama, and real hardware"]
fn real_local_benchmark_and_saved_badge() {
    let model_name = acceptance_model_name();
    let (runtime, commands, events, panel, window) = acceptance_panel();
    discover_and_select(&commands, &events, &panel, &model_name);
    wait_for_initial_absence(&panel, &events);
    let manifest_digest = selected_manifest_digest(&panel);
    assert!(
        !panel
            .verification_status
            .label()
            .contains("Benchmark Verified"),
        "fresh acceptance database must not already verify selected model"
    );
    assert!(
        panel.run.is_sensitive(),
        "selected local model must be runnable"
    );
    panel.run.emit_clicked();

    let completed = wait_for_event(&panel, &events, Duration::from_secs(120), |event| {
        matches!(event, UiEvent::BenchmarkFinished { .. })
    });
    let (benchmark_id, report, resources) = match completed {
        UiEvent::BenchmarkFinished {
            id,
            outcome: BenchmarkOutcome::Completed(report),
            resources,
        } => (id, report, resources),
        UiEvent::BenchmarkFinished { outcome, .. } => {
            panic!("real benchmark did not complete: {outcome:?}")
        }
        _ => unreachable!(),
    };
    assert_eq!(report.generation.model_name, model_name);
    assert_eq!(
        canonical_manifest_digest(&report.generation.model_digest).as_deref(),
        Some(manifest_digest.as_str())
    );
    let saved = wait_for_event(&panel, &events, Duration::from_secs(20), |event| {
        matches!(event, UiEvent::BenchmarkHistorySaved { .. })
    });
    let (saved_benchmark_id, history_id) = match saved {
        UiEvent::BenchmarkHistorySaved {
            benchmark_id,
            result: Ok(history_id),
        } => (benchmark_id, history_id),
        UiEvent::BenchmarkHistorySaved {
            result: Err(error), ..
        } => panic!("real benchmark history save failed: {error}"),
        _ => unreachable!(),
    };
    assert_eq!(saved_benchmark_id, benchmark_id);

    let loaded = wait_for_event(&panel, &events, Duration::from_secs(20), |event| {
        matches!(event, UiEvent::BenchmarkHistoryLoaded { .. })
    });
    let rows = match loaded {
        UiEvent::BenchmarkHistoryLoaded {
            result: Ok(rows), ..
        } => rows,
        UiEvent::BenchmarkHistoryLoaded {
            result: Err(error), ..
        } => panic!("saved benchmark history reload failed: {error}"),
        _ => unreachable!(),
    };
    assert_completed_history_row(&rows, history_id, &model_name, &manifest_digest);
    let verified_id = wait_for_verified_badge(&panel, &events, Duration::from_secs(20));
    assert_eq!(verified_id, history_id);

    println!(
        "Real benchmark #{benchmark_id}, saved row #{history_id}: model={}, manifest={}, server eval_count={:?}, eval_duration_ns={:?}, load_duration_ns={:?}, decode_tokens_per_second={:?}, resources={:?}",
        report.generation.model_name,
        report.generation.model_digest,
        report.generation.eval_count,
        report.generation.eval_duration_ns,
        report.generation.load_duration_ns,
        report.generation_tokens_per_second,
        resources,
    );

    close_acceptance(runtime, events, panel, window);
}

#[test]
#[ignore = "requires prior real_local_benchmark_and_saved_badge run, KVLM_ACCEPTANCE_MODEL, isolated XDG_DATA_HOME, local Ollama, and real hardware"]
fn saved_badge_survives_process_restart() {
    let model_name = acceptance_model_name();
    let (runtime, commands, events, panel, window) = acceptance_panel();
    discover_and_select(&commands, &events, &panel, &model_name);
    let manifest_digest = selected_manifest_digest(&panel);
    let verified_id = wait_for_verified_badge(&panel, &events, Duration::from_secs(30));

    panel.request_history();
    let loaded = wait_for_event(&panel, &events, Duration::from_secs(20), |event| {
        matches!(event, UiEvent::BenchmarkHistoryLoaded { .. })
    });
    let rows = match loaded {
        UiEvent::BenchmarkHistoryLoaded {
            result: Ok(rows), ..
        } => rows,
        UiEvent::BenchmarkHistoryLoaded {
            result: Err(error), ..
        } => panic!("restart history load failed: {error}"),
        _ => unreachable!(),
    };
    assert_completed_history_row(&rows, verified_id, &model_name, &manifest_digest);
    println!(
        "Restart verification found completed saved benchmark row #{verified_id} for {model_name}."
    );

    close_acceptance(runtime, events, panel, window);
}

fn acceptance_model_name() -> String {
    let xdg_data_home = env::var_os("XDG_DATA_HOME")
        .expect("acceptance runner must provide isolated XDG_DATA_HOME");
    assert!(
        Path::new(&xdg_data_home).is_absolute(),
        "acceptance XDG_DATA_HOME must be absolute"
    );
    let model = env::var("KVLM_ACCEPTANCE_MODEL")
        .expect("set KVLM_ACCEPTANCE_MODEL to an installed local Ollama model name");
    assert!(
        !model.trim().is_empty(),
        "acceptance model name must not be empty"
    );
    model
}

fn acceptance_panel() -> (
    tokio::runtime::Runtime,
    mpsc::Sender<CoreCommand>,
    async_channel::Receiver<UiEvent>,
    Benchmarks,
    gtk::Window,
) {
    adw::init().expect("GTK display is available");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Tokio runtime starts");
    let (commands, events) = bridge::start(runtime.handle());
    let panel = Benchmarks::new(commands.clone());
    let window = gtk::Window::builder()
        .default_width(900)
        .default_height(800)
        .child(&panel.widget)
        .build();
    window.present();
    assert!(wait_for_layout(&window));
    (runtime, commands, events, panel, window)
}

fn discover_and_select(
    commands: &mpsc::Sender<CoreCommand>,
    events: &async_channel::Receiver<UiEvent>,
    panel: &Benchmarks,
    model_name: &str,
) {
    commands
        .try_send(CoreCommand::DiscoverOllama)
        .expect("Ollama discovery command is queued");
    let discovery = wait_for_event(panel, events, Duration::from_secs(20), |event| {
        matches!(
            event,
            UiEvent::OllamaDiscovered(_) | UiEvent::OllamaFailed(_)
        )
    });
    match discovery {
        UiEvent::OllamaDiscovered(discovery) => {
            discovery
                .installed
                .as_ref()
                .expect("installed model inventory is available");
        }
        UiEvent::OllamaFailed(error) => panic!("attached Ollama discovery failed: {error}"),
        _ => unreachable!(),
    }
    let selected_index = panel
        .state
        .borrow()
        .models
        .iter()
        .position(|model| model.name == model_name)
        .unwrap_or_else(|| panic!("model {model_name:?} is absent or not selectable"));
    let selected = panel.state.borrow().models[selected_index].clone();
    assert!(!selected.cloud_routed(), "acceptance model must be local");
    assert!(
        selected
            .digest
            .as_deref()
            .and_then(canonical_manifest_digest)
            .is_some(),
        "acceptance model must have a full SHA-256 Ollama manifest digest"
    );
    panel
        .model
        .set_selected(u32::try_from(selected_index + 1).unwrap());
    assert_eq!(
        panel
            .state
            .borrow()
            .selected
            .as_ref()
            .map(|model| model.name.as_str()),
        Some(model_name)
    );
}

fn wait_for_initial_absence(panel: &Benchmarks, events: &async_channel::Receiver<UiEvent>) {
    let result = wait_for_event(panel, events, Duration::from_secs(30), |event| {
        matches!(event, UiEvent::BenchmarkVerificationChecked { .. })
    });
    match result {
        UiEvent::BenchmarkVerificationChecked {
            result: Ok(None), ..
        } => assert!(panel.verification_status.label().contains("No matching")),
        UiEvent::BenchmarkVerificationChecked { result, .. } => {
            panic!("fresh acceptance database must have no matching verification: {result:?}")
        }
        _ => unreachable!(),
    }
}

fn selected_manifest_digest(panel: &Benchmarks) -> String {
    panel
        .state
        .borrow()
        .selected
        .as_ref()
        .and_then(|model| model.digest.as_deref())
        .and_then(canonical_manifest_digest)
        .expect("selected acceptance model has canonical manifest digest")
}

fn assert_completed_history_row(
    rows: &[HistoryEntry],
    id: i64,
    model_name: &str,
    manifest_digest: &str,
) {
    let entry = rows
        .iter()
        .find(|entry| entry.id == id)
        .expect("completed benchmark row survived history reload");
    assert_eq!(entry.snapshot.model_name, model_name);
    assert_eq!(
        entry
            .snapshot
            .model_digest
            .as_deref()
            .and_then(canonical_manifest_digest)
            .as_deref(),
        Some(manifest_digest)
    );
    let completion = entry
        .completion
        .as_ref()
        .expect("terminal completion saved");
    assert_eq!(
        completion
            .model_digest
            .as_deref()
            .and_then(canonical_manifest_digest)
            .as_deref(),
        Some(manifest_digest)
    );
    assert!(matches!(
        Some(&completion.outcome),
        Some(workstation_core::db::RunOutcome::Completed(_))
    ));
}

fn wait_for_verified_badge(
    panel: &Benchmarks,
    events: &async_channel::Receiver<UiEvent>,
    timeout: Duration,
) -> i64 {
    let deadline = Instant::now() + timeout;
    let mut last_retry = Instant::now();
    while Instant::now() < deadline {
        pump_events(panel, events);
        if let Some(id) = verified_run_id(&panel.verification_status.label()) {
            return id;
        }
        if panel.check_verification.is_sensitive()
            && panel.state.borrow().verification_pending.is_none()
            && last_retry.elapsed() >= Duration::from_millis(250)
        {
            panel.check_verification.emit_clicked();
            last_retry = Instant::now();
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "saved Benchmark Verified badge did not appear: {}",
        panel.verification_status.label()
    );
}

fn verified_run_id(status: &str) -> Option<i64> {
    status
        .split_once("Benchmark Verified")?
        .1
        .split_once("run #")?
        .1
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn wait_for_event(
    panel: &Benchmarks,
    events: &async_channel::Receiver<UiEvent>,
    timeout: Duration,
    mut predicate: impl FnMut(&UiEvent) -> bool,
) -> UiEvent {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(event) = pump_events_until(panel, events, &mut predicate) {
            return event;
        }
        assert!(Instant::now() < deadline, "timed out waiting for app event");
        thread::sleep(Duration::from_millis(10));
    }
}

fn pump_events(panel: &Benchmarks, events: &async_channel::Receiver<UiEvent>) {
    let _: Option<UiEvent> = pump_events_until(panel, events, &mut |_| false);
}

fn pump_events_until(
    panel: &Benchmarks,
    events: &async_channel::Receiver<UiEvent>,
    predicate: &mut impl FnMut(&UiEvent) -> bool,
) -> Option<UiEvent> {
    let context = gtk::glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
    }
    loop {
        match events.try_recv() {
            Ok(event) => {
                panel.receive(&event);
                if predicate(&event) {
                    return Some(event);
                }
            }
            Err(async_channel::TryRecvError::Empty) => return None,
            Err(async_channel::TryRecvError::Closed) => panic!("app event channel closed"),
        }
    }
}

fn close_acceptance(
    runtime: tokio::runtime::Runtime,
    events: async_channel::Receiver<UiEvent>,
    panel: Benchmarks,
    window: gtk::Window,
) {
    window.close();
    events.close();
    drop(panel);
    runtime.shutdown_timeout(Duration::from_secs(5));
}

fn receive_run(receiver: &mut mpsc::Receiver<CoreCommand>) -> (u64, watch::Receiver<bool>) {
    match receiver.try_recv().unwrap() {
        CoreCommand::RunQuickBenchmark { id, model, cancel } => {
            assert_eq!(model.name, "fixture:latest");
            (id, cancel)
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

fn receive_verification(receiver: &mut mpsc::Receiver<CoreCommand>) -> (u64, String, String) {
    match receiver.try_recv().unwrap() {
        CoreCommand::CheckBenchmarkVerification {
            id,
            model_name,
            model_digest,
        } => (id, model_name, model_digest),
        other => panic!("unexpected command: {other:?}"),
    }
}

fn report_fixture() -> QuickBenchmarkReport {
    QuickBenchmarkReport {
        workload_id: "quick-text-v1",
        generation: GenerationSummary {
            model_name: "fixture:latest".into(),
            model_digest: "sha256:fixture".into(),
            runtime_version: "fixture-runtime".into(),
            first_output_after: None,
            elapsed: Duration::from_secs(1),
            response_chunks: 0,
            eval_count: None,
            eval_duration_ns: None,
            load_duration_ns: None,
            done_reason: "stop".into(),
        },
        generation_tokens_per_second: None,
    }
}

fn resource_fixture() -> workstation_core::hardware::resources::ResourceSummary {
    use workstation_core::hardware::resources::{GpuMemoryPeak, ResourceSummary};
    ResourceSummary {
        interval_ms: 100,
        window_ms: 1200,
        samples: 12,
        system_ram_samples: 10,
        peak_system_ram_nonavailable_bytes: Some(4096),
        gpus: vec![GpuMemoryPeak {
            id: "gpu0".into(),
            name: "Fixture GPU".into(),
            vram_samples: 4,
            peak_vram_used_bytes: Some(2048),
            gtt_samples: 3,
            peak_gtt_used_bytes: Some(1024),
        }],
        probe_failed: true,
        probe_in_flight: false,
        limit_reached: false,
    }
}

fn discovery() -> UiEvent {
    let long_name = "fixture-long-name".repeat(300);
    UiEvent::OllamaDiscovered(Box::new(Discovery {
        version: "fixture".into(),
        installed: Ok(vec![
            model(
                "fixture:latest",
                Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                false,
            ),
            model("fixture-cloud:latest", Some("sha256:cloud"), true),
            model("fixture:no-digest", None, false),
            model(&long_name, Some("sha256:long"), false),
        ]),
        running: Ok(Vec::new()),
    }))
}

fn model(name: &str, digest: Option<&str>, cloud: bool) -> InstalledModel {
    InstalledModel {
        name: name.into(),
        digest: digest.map(str::to_owned),
        size: Some(100),
        details: Some(ModelDetails {
            family: Some("fixture".into()),
            parameter_size: None,
            quantization_level: None,
        }),
        remote_model: cloud.then(|| "cloud".into()),
        remote_host: None,
    }
}

fn label_text(widget: &gtk::Widget) -> String {
    let mut text = String::new();
    if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
        text.push_str(&label.label());
        text.push('\n');
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        text.push_str(&label_text(&current));
        child = current.next_sibling();
    }
    text
}

fn wait_for_layout(window: &gtk::Window) -> bool {
    let context = gtk::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        if window.width() > 0 && window.height() > 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

fn drain_layout() {
    let context = gtk::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(1));
    }
}
