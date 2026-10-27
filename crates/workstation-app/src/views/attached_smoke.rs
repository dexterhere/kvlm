// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage for the attached-runtimes section; command channel only.

use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

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

fn checks(widget: &gtk::Widget, out: &mut Vec<gtk::Button>) {
    if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
        out.push(button);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        checks(&current, out);
        child = current.next_sibling();
    }
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn check_flow_is_explicit_one_at_a_time_and_drops_stale_answers() {
    adw::init().expect("GTK display is available");
    let (commands, mut rx) = mpsc::channel(8);
    let attached = Attached::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .child(&attached.widget)
        .build();
    window.present();
    pump();
    // Nothing is asked until a button is pressed.
    assert!(rx.try_recv().is_err());
    let mut buttons = Vec::new();
    checks(&attached.widget.clone().upcast(), &mut buttons);
    buttons.retain(|b| b.label().as_deref() == Some("Check"));
    assert_eq!(buttons.len(), 2);
    buttons[1].emit_clicked();
    let (id, engine, port) = match rx.try_recv().unwrap() {
        AttachedCommand::Check { id, engine, port } => (id, engine, port),
    };
    assert_eq!((engine, port), (Engine::Vllm, 8000));
    assert!(!buttons[1].is_sensitive(), "one check at a time");
    buttons[1].emit_clicked();
    assert!(rx.try_recv().is_err());
    attached.receive(&AttachedEvent::Checked {
        id: id + 9,
        engine,
        found: Found::LmStudio(Err("stale".into())),
    });
    assert!(!buttons[1].is_sensitive());
    attached.receive(&AttachedEvent::Checked {
        id,
        engine,
        found: Found::Vllm(Box::new(vllm::Discovery {
            healthy: Err(workstation_runtimes::local_http::LocalError::Unreachable),
            models: Err(workstation_runtimes::local_http::LocalError::Unreachable),
            metrics: Err(workstation_runtimes::local_http::LocalError::Unreachable),
            truncated: false,
        })),
    });
    pump();
    assert!(buttons[1].is_sensitive());
    let (min_width, ..) = attached.widget.measure(gtk::Orientation::Horizontal, -1);
    assert!(min_width <= 360, "layout needs {min_width} pixels");
    window.close();
}
