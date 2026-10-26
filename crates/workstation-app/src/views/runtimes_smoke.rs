// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage; this test uses only typed fixtures and command channels.

use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};
use workstation_runtimes::ollama::{ModelDetails, ModelMetadata};

#[test]
#[ignore = "requires a graphical GTK display"]
fn installed_model_inspection_flow() {
    adw::init().expect("GTK display is available");
    let (commands, mut command_rx) = tokio::sync::mpsc::channel(1);
    let panel = Runtimes::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(620)
        .child(&panel.widget)
        .build();
    window.present();
    assert!(wait_for_layout(&window));
    assert!(
        window.width() <= 360,
        "narrow layout grew to {} pixels",
        window.width()
    );

    panel.refresh.emit_clicked();
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DiscoverOllama)
    ));
    panel.receive(&discovery());
    let inspect = panel.inspection.borrow().buttons[0].clone();
    assert!(inspect.is_sensitive());
    inspect.emit_clicked();
    let first_id = match command_rx.try_recv().unwrap() {
        CoreCommand::InspectOllama { id, model } => {
            assert_eq!(model, "fixture:latest");
            id
        }
        other => panic!("unexpected command: {other:?}"),
    };
    assert!(!inspect.is_sensitive());
    assert!(
        labels(&panel.widget.clone().upcast())
            .iter()
            .any(|s| s.contains("Loading model metadata"))
    );

    panel.receive(&inspected(first_id));
    let rendered = labels(&panel.widget.clone().upcast());
    assert!(rendered.iter().any(|s| s == "Declared training context"));
    assert!(rendered.iter().any(|s| s == "8192 tokens"));
    assert!(rendered.iter().any(|s| s == "Declared parameters"));
    assert!(rendered.iter().any(|s| s.contains("num_ctx 2048")));
    assert!(rendered.iter().any(|s| s.contains("Context: 4096 tokens")));
    assert!(
        rendered
            .iter()
            .any(|s| s.contains("Total allocation: 100 bytes"))
    );
    assert!(
        !rendered
            .iter()
            .any(|s| s.contains("Loading model metadata"))
    );
    assert!(inspect.is_sensitive());
    drain_layout();
    assert!(
        window.width() <= 360,
        "populated layout grew to {} pixels",
        window.width()
    );

    panel.refresh.emit_clicked();
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DiscoverOllama)
    ));
    assert!(
        !inspect.is_sensitive(),
        "stale model rows must be disabled during refresh"
    );
    panel.receive(&UiEvent::OllamaFailed("fixture failure".into()));
    assert!(
        !inspect.is_sensitive(),
        "failed refresh must leave stale model rows disabled"
    );
    panel.receive(&inspected(first_id));
    assert!(panel.inspection_details.first_child().is_none());
    assert!(panel.status.label().contains("fixture failure"));

    panel.refresh.emit_clicked();
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DiscoverOllama)
    ));
    panel.receive(&discovery());
    let recovered = panel.inspection.borrow().buttons[0].clone();
    assert!(
        recovered.is_sensitive(),
        "successful discovery should restore Inspect"
    );
    recovered.emit_clicked();
    let second_id = match command_rx.try_recv().unwrap() {
        CoreCommand::InspectOllama { id, model } => {
            assert_eq!(model, "fixture:latest");
            id
        }
        other => panic!("unexpected command: {other:?}"),
    };
    assert_ne!(first_id, second_id);
    panel.refresh.emit_clicked();
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DiscoverOllama)
    ));
    assert!(!recovered.is_sensitive());
    panel.receive(&UiEvent::OllamaFailed("fixture refresh failure".into()));
    assert!(!recovered.is_sensitive());
    panel.receive(&inspected(second_id));
    assert!(panel.inspection_details.first_child().is_none());
    assert!(panel.status.label().contains("fixture refresh failure"));

    panel.refresh.emit_clicked();
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::DiscoverOllama)
    ));
    panel.receive(&discovery());
    let final_inspect = panel.inspection.borrow().buttons[0].clone();
    assert!(final_inspect.is_sensitive());
    final_inspect.emit_clicked();
    let third_id = match command_rx.try_recv().unwrap() {
        CoreCommand::InspectOllama { id, model } => {
            assert_eq!(model, "fixture:latest");
            id
        }
        other => panic!("unexpected command: {other:?}"),
    };
    panel.receive(&UiEvent::OllamaInspected {
        id: third_id,
        result: Err("inspection fixture failure".into()),
    });
    assert!(
        labels(&panel.widget.clone().upcast())
            .iter()
            .any(|s| s.contains("inspection fixture failure"))
    );
    assert!(
        final_inspect.is_sensitive(),
        "failed inspection must leave the same button retryable"
    );
    final_inspect.emit_clicked();
    assert!(matches!(
        command_rx.try_recv(),
        Ok(CoreCommand::InspectOllama { model, .. }) if model == "fixture:latest"
    ));

    window.close();
    drop(panel);
}

fn discovery() -> UiEvent {
    UiEvent::OllamaDiscovered(Box::new(Discovery {
        version: "fixture".into(),
        installed: Ok(vec![InstalledModel {
            name: "fixture:latest".into(),
            digest: Some("sha256:fixture".into()),
            size: Some(100),
            details: Some(ModelDetails {
                family: Some("fixture-family".into()),
                parameter_size: None,
                quantization_level: None,
            }),
            remote_model: None,
            remote_host: None,
        }]),
        running: Ok(Vec::new()),
    }))
}

fn inspected(id: u64) -> UiEvent {
    UiEvent::OllamaInspected {
        id,
        result: Ok(Box::new(ModelInspection {
            metadata: Ok(ModelMetadata {
                details: Some(ModelDetails {
                    family: Some("fixture-family".into()),
                    parameter_size: Some("7B".into()),
                    quantization_level: Some("Q4_K_M".into()),
                }),
                capabilities: Some(vec!["completion".into()]),
                parameters: Some("num_ctx 2048".into()),
                model_info: Some(
                    [
                        ("general.architecture".into(), "llama".into()),
                        ("llama.context_length".into(), 8192_u64.into()),
                    ]
                    .into_iter()
                    .collect(),
                ),
                remote_model: None,
                remote_host: None,
            }),
            running: Ok(vec![RunningModel {
                name: "fixture:latest".into(),
                digest: Some("sha256:fixture".into()),
                size: Some(100),
                size_vram: Some(80),
                context_length: Some(4096),
            }]),
        })),
    }
}

fn labels(widget: &gtk::Widget) -> Vec<String> {
    let mut output = Vec::new();
    if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
        output.push(label.label().to_string());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        output.extend(labels(&current));
        child = current.next_sibling();
    }
    output
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
