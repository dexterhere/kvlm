// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage; all runtime data comes from typed fixtures.

use super::*;
use crate::bridge::UiEvent;
use std::{
    thread,
    time::{Duration, Instant},
};
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

fn receive_run(receiver: &mut mpsc::Receiver<CoreCommand>) -> (u64, watch::Receiver<bool>) {
    match receiver.try_recv().unwrap() {
        CoreCommand::RunQuickBenchmark { id, model, cancel } => {
            assert_eq!(model.name, "fixture:latest");
            (id, cancel)
        }
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
            model("fixture:latest", Some("sha256:fixture"), false),
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
