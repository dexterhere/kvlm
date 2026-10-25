// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK coverage; history data is supplied as typed fixtures.

use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};
use workstation_core::{
    db::{RunOutcome, RunSnapshot, WorkloadSnapshot},
    hardware::HardwareProfile,
};

#[test]
#[ignore = "requires a graphical GTK display"]
fn history_load_retry_stale_save_and_outcome_flow() {
    adw::init().expect("GTK display is available");
    let (commands, mut command_rx) = mpsc::channel(1);
    let history = BenchmarkHistory::new(commands.clone());
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(640)
        .child(&history.widget)
        .build();
    window.present();
    assert!(wait_for_layout(&window));
    assert!(window.width() <= 360);

    // Startup load waits behind another command instead of being dropped on a full queue.
    commands.try_send(CoreCommand::DetectHardware).unwrap();
    history.request();
    assert!(matches!(
        wait_command(&mut command_rx),
        CoreCommand::DetectHardware
    ));
    let first_id = load_id(wait_command(&mut command_rx));
    let mut completed = entry(1, Some(RunOutcome::Completed(metrics())));
    completed.snapshot.model_name = "<b>literal-model</b>".repeat(80);
    completed.completion.as_mut().unwrap().resources = Some(resource_fixture());
    history.receive(&UiEvent::BenchmarkHistoryLoaded {
        id: first_id,
        result: Ok(vec![
            completed,
            entry(2, Some(RunOutcome::Cancelled(partial()))),
            entry(3, Some(RunOutcome::TimedOut(partial()))),
            entry(
                4,
                Some(RunOutcome::Failed {
                    message: "fixture failure".into(),
                    partial: partial(),
                }),
            ),
            entry(5, None),
        ]),
    });
    let rendered = label_text(history.widget.upcast_ref());
    for expected in [
        "Completed",
        "Cancelled",
        "Timed out",
        "Failed",
        "Unfinished (running or interrupted)",
        "Server evaluation tokens",
        "Unknown",
        "System RAM nonavailable (system-wide peak): 4096 bytes",
        "GPU 1 (Fixture GPU) driver-reported VRAM peak: 2048 bytes · 4 samples",
        "GTT peak (overlaps system RAM): 1024 bytes · 3 samples",
        "lower-bound peaks",
        "Memory sampling: Not sampled",
    ] {
        assert!(rendered.contains(expected), "missing {expected:?}");
    }

    pump_gtk();
    assert!(
        window.width() <= 360,
        "long history must fit a narrow window"
    );
    let first_row = history
        .entries
        .first_child()
        .unwrap()
        .downcast::<adw::ExpanderRow>()
        .unwrap();
    assert!(!first_row.uses_markup());
    first_row.set_expanded(true);
    assert!(label_text(history.widget.upcast_ref()).contains("Completion reason: stop"));
    assert!(label_text(history.widget.upcast_ref()).contains("truncated"));

    history.reload.emit_clicked();
    let second_id = load_id(wait_command(&mut command_rx));
    history.receive(&UiEvent::BenchmarkHistorySaved {
        benchmark_id: 6,
        result: Ok(6),
    });
    history.receive(&UiEvent::BenchmarkHistoryLoaded {
        id: first_id,
        result: Err("stale reply".into()),
    });
    assert_eq!(history.state.borrow().pending, Some(second_id));
    history.receive(&UiEvent::BenchmarkHistoryLoaded {
        id: second_id,
        result: Err("fixture database failure".into()),
    });
    assert!(label_text(history.widget.upcast_ref()).contains("Run #1"));
    let third_id = load_id(wait_command(&mut command_rx));
    history.receive(&UiEvent::BenchmarkHistoryLoaded {
        id: third_id,
        result: Err("fixture retry failure".into()),
    });
    assert!(history.status.label().contains("previous snapshot"));
    history.reload.emit_clicked();
    let fourth_id = load_id(wait_command(&mut command_rx));
    history.receive(&UiEvent::BenchmarkHistoryLoaded {
        id: fourth_id,
        result: Ok(Vec::new()),
    });
    assert!(
        history
            .status
            .label()
            .contains("No saved benchmark history")
    );
    assert!(!label_text(history.widget.upcast_ref()).contains("Run #1"));

    history.receive(&UiEvent::BenchmarkHistorySaved {
        benchmark_id: 7,
        result: Err("fixture write failure".into()),
    });
    assert!(history.feedback.label().contains("Run #7"));
    assert!(
        history
            .feedback
            .label()
            .contains("in-memory result remains unchanged")
    );

    let (closed_commands, closed_rx) = mpsc::channel(1);
    let closed_history = BenchmarkHistory::new(closed_commands);
    drop(closed_rx);
    closed_history.request();
    assert!(wait_for_label(&closed_history.status, "worker unavailable"));
    assert!(closed_history.state.borrow().pending.is_none());

    window.close();
}

fn load_id(command: CoreCommand) -> u64 {
    match command {
        CoreCommand::LoadBenchmarkHistory { id } => id,
        other => panic!("unexpected command: {other:?}"),
    }
}

fn wait_command(receiver: &mut mpsc::Receiver<CoreCommand>) -> CoreCommand {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        pump_gtk();
        if let Ok(command) = receiver.try_recv() {
            return command;
        }
        assert!(Instant::now() < deadline, "timed out waiting for command");
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_label(label: &gtk::Label, text: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        pump_gtk();
        if label.label().to_lowercase().contains(text) {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

fn wait_for_layout(window: &gtk::Window) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        pump_gtk();
        if window.width() > 0 && window.height() > 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

fn pump_gtk() {
    let context = gtk::glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
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

fn entry(id: i64, outcome: Option<RunOutcome>) -> HistoryEntry {
    HistoryEntry {
        id,
        snapshot: RunSnapshot {
            started_at_unix_ms: 1_700_000_000_000,
            model_name: format!("fixture-model-{id}"),
            model_digest: Some(format!("sha256:{id}")),
            model_size_bytes: Some(1024),
            hardware: None::<HardwareProfile>,
            hardware_notice: None,
            runtime_endpoint: "http://127.0.0.1:11434".into(),
            workload: WorkloadSnapshot {
                id: "quick-text-v1".into(),
                prompt_sha256: "fixture-prompt-hash".into(),
                num_predict: 128,
                num_ctx: 2048,
                seed: 42,
                temperature: 0,
                think: false,
            },
        },
        completion: outcome.map(|outcome| RunCompletion {
            finished_at_unix_ms: 1_700_000_001_000,
            runtime_version: Some("fixture-runtime".into()),
            model_digest: Some(format!("sha256:{id}")),
            outcome,
            resources: None,
        }),
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
        probe_failed: false,
        probe_in_flight: false,
        limit_reached: false,
    }
}

fn metrics() -> RunMetrics {
    RunMetrics {
        first_response_ns: Some(250_000_000),
        elapsed_ns: 2_000_000_000,
        response_chunks: 8,
        eval_count: Some(64),
        eval_duration_ns: Some(1_000_000_000),
        load_duration_ns: Some(500_000_000),
        done_reason: "stop".into(),
    }
}

fn partial() -> PartialMetrics {
    PartialMetrics {
        first_response_ns: None,
        response_chunks: 0,
    }
}
