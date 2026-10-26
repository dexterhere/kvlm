// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage for the managed card; typed fixtures and command channels only.

use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};
use workstation_core::{
    db::profiles::InstallRow,
    runtime::profile::{CacheType, LaunchProfile},
};
use workstation_runtimes::supervisor::Status;

fn pump() {
    let context = gtk::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(150);
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn buttons(widget: &gtk::Widget, out: &mut Vec<gtk::Button>) {
    if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
        out.push(button);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        buttons(&current, out);
        child = current.next_sibling();
    }
}

fn button(card: &ManagedCard, label: &str) -> gtk::Button {
    let mut all = Vec::new();
    buttons(&card.widget.clone().upcast(), &mut all);
    all.into_iter()
        .find(|b| b.label().as_deref().map(|l| l.replace('_', "")) == Some(label.to_owned()))
        .unwrap_or_else(|| panic!("no button {label}"))
}

fn snapshot() -> Snapshot {
    Snapshot {
        installs: vec![InstallRow {
            version: "b1".into(),
            backend: "cpu".into(),
            dir_name: "b1-cpu".into(),
            sha256: "0".repeat(64),
            installed_at_unix_ms: 1,
        }],
        profiles: vec![LaunchProfile {
            name: "tiny".into(),
            model_path: "/m/t.gguf".into(),
            port: 8081,
            context_length: 4096,
            gpu_layers: 0,
            cache_type_k: CacheType::F16,
            cache_type_v: CacheType::F16,
            flash_attention: false,
            ubatch_size: 512,
            extra: vec![],
        }],
        backends: vec![Backend::Cpu],
        supervisor_error: None,
    }
}

fn live(state: RuntimeLifecycleState) -> UiEvent {
    UiEvent::Managed(ManagedEvent::Live(Box::new(Live {
        status: Status {
            state,
            generation: Some("abcdef01".into()),
            pid: Some(1),
            port: Some(8081),
            failure: None,
            restarts: 0,
        },
        busy: None,
        props: None,
    })))
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn start_stop_and_install_flow() {
    adw::init().expect("GTK display is available");
    let (commands, mut rx) = tokio::sync::mpsc::channel(8);
    let card = ManagedCard::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(620)
        .child(&card.widget)
        .build();
    window.present();
    pump();

    card.load();
    let load_id = match rx.try_recv().unwrap() {
        CoreCommand::Managed(ManagedCommand::Load { id }) => id,
        other => panic!("unexpected {other:?}"),
    };
    // Nothing can start before the saved state arrives.
    assert!(!button(&card, "Stop").is_sensitive());
    card.receive(&UiEvent::Managed(ManagedEvent::Snapshot {
        id: load_id,
        result: Ok(Box::new(snapshot())),
    }));
    pump();
    let start = button(&card, "Start");
    assert!(start.is_sensitive());
    start.emit_clicked();
    let id = match rx.try_recv().unwrap() {
        CoreCommand::Managed(ManagedCommand::Start {
            id,
            profile,
            dir_name,
        }) => {
            assert_eq!((profile.as_str(), dir_name.as_str()), ("tiny", "b1-cpu"));
            id
        }
        other => panic!("unexpected {other:?}"),
    };
    pump();
    // While the request is pending a second Start is not offered.
    assert!(!button(&card, "Start").is_sensitive());
    card.receive(&UiEvent::Managed(ManagedEvent::Action {
        id,
        result: Ok(()),
    }));
    card.receive(&live(RuntimeLifecycleState::Ready));
    pump();
    assert!(button(&card, "Stop").is_sensitive());
    assert!(!button(&card, "Start").is_sensitive());
    assert!(
        !button(&card, "Remove").is_sensitive(),
        "a running build cannot be removed"
    );
    let shown: Vec<String> = {
        fn collect(w: &gtk::Widget, out: &mut Vec<String>) {
            if let Ok(l) = w.clone().downcast::<gtk::Label>() {
                out.push(l.label().to_string());
            }
            let mut c = w.first_child();
            while let Some(n) = c {
                collect(&n, out);
                c = n.next_sibling();
            }
        }
        let mut out = Vec::new();
        collect(&card.widget.clone().upcast(), &mut out);
        out
    };
    assert!(shown.iter().any(|s| s.contains("http://127.0.0.1:8081/v1")));
    // The window manager may resize the test window, so check the widget's own minimum.
    let (min_width, ..) = card.widget.measure(gtk::Orientation::Horizontal, -1);
    assert!(min_width <= 360, "layout needs {min_width} pixels");

    button(&card, "Stop").emit_clicked();
    assert!(matches!(
        rx.try_recv().unwrap(),
        CoreCommand::Managed(ManagedCommand::Stop { .. })
    ));

    // A full channel is reported instead of dropping the click.
    let (tiny, _keep) = tokio::sync::mpsc::channel(1);
    let busy = ManagedCard::new(tiny.clone());
    tiny.try_send(CoreCommand::DetectHardware).unwrap();
    busy.load();
    pump();
    let mut texts = Vec::new();
    fn walk(w: &gtk::Widget, out: &mut Vec<String>) {
        if let Ok(l) = w.clone().downcast::<gtk::Label>() {
            out.push(l.label().to_string());
        }
        let mut c = w.first_child();
        while let Some(n) = c {
            walk(&n, out);
            c = n.next_sibling();
        }
    }
    walk(&busy.widget.clone().upcast(), &mut texts);
    assert!(texts.iter().any(|t| t.contains("busy or closing")));
    window.close();
    find_install_progress_and_cancel_flow();
}

/// Runs inside the single GTK test: GTK can be initialised on one thread only.
fn find_install_progress_and_cancel_flow() {
    use workstation_runtimes::llamacpp::release::Asset;
    let (commands, mut rx) = tokio::sync::mpsc::channel(8);
    let card = ManagedCard::new(commands);
    let window = gtk::Window::builder().child(&card.widget).build();
    window.present();
    card.load();
    let load_id = match rx.try_recv().unwrap() {
        CoreCommand::Managed(ManagedCommand::Load { id }) => id,
        other => panic!("unexpected {other:?}"),
    };
    let mut empty = snapshot();
    empty.installs.clear();
    card.receive(&UiEvent::Managed(ManagedEvent::Snapshot {
        id: load_id,
        result: Ok(Box::new(empty)),
    }));
    pump();
    // Nothing is fetched until the user asks.
    assert!(rx.try_recv().is_err());
    assert!(
        !button(&card, "Start").is_sensitive(),
        "no build installed yet"
    );
    button(&card, "Find").emit_clicked();
    let id = match rx.try_recv().unwrap() {
        CoreCommand::Managed(ManagedCommand::Builds { id, backends }) => {
            assert_eq!(backends, vec![Backend::Cpu]);
            id
        }
        other => panic!("unexpected {other:?}"),
    };
    let candidate = Candidate {
        tag: "b2".into(),
        backend: Backend::Cpu,
        archives: vec![Asset {
            name: "a".into(),
            size: 10,
            sha256: "0".repeat(64),
        }],
    };
    card.receive(&UiEvent::Managed(ManagedEvent::Builds {
        id,
        result: Ok(vec![candidate]),
    }));
    pump();
    button(&card, "Install").emit_clicked();
    let id = match rx.try_recv().unwrap() {
        CoreCommand::Managed(ManagedCommand::Install { id, candidate }) => {
            assert_eq!(candidate.tag, "b2");
            id
        }
        other => panic!("unexpected {other:?}"),
    };
    card.receive(&UiEvent::Managed(ManagedEvent::InstallProgress(Progress {
        archive: 0,
        archives: 1,
        done: 5,
        total: 10,
    })));
    pump();
    assert!(
        !button(&card, "Install").is_sensitive(),
        "one install at a time"
    );
    button(&card, "Cancel").emit_clicked();
    assert!(matches!(
        rx.try_recv().unwrap(),
        CoreCommand::Managed(ManagedCommand::CancelInstall)
    ));
    card.receive(&UiEvent::Managed(ManagedEvent::Installed {
        id,
        result: Err("The install was cancelled.".into()),
    }));
    pump();
    assert!(button(&card, "Find").is_sensitive());
    window.close();
}
