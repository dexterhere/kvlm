// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK flow coverage for the Benchmark Lab page; typed fixtures and command channels
//! only, no server, store or network.

use super::*;
use crate::lab::RunDone;
use std::{
    thread,
    time::{Duration, Instant},
};
use workstation_bench::lab::Progress;
use workstation_core::{
    db::experiments::Listing,
    experiments::{Engine, Finding, NewExperiment, Settings},
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

fn find(lab: &Lab, label: &str) -> Option<gtk::Button> {
    let mut all = Vec::new();
    buttons(&lab.widget, &mut all);
    all.into_iter()
        .find(|b| b.label().as_deref().is_some_and(|l| l.starts_with(label)))
}

fn button(lab: &Lab, label: &str) -> gtk::Button {
    find(lab, label).unwrap_or_else(|| panic!("no button {label}"))
}

fn experiment() -> Experiment {
    Experiment {
        id: 5,
        state: RunState::Completed,
        started_at_unix_ms: 1,
        finished_at_unix_ms: Some(2),
        spec: NewExperiment {
            engine: Engine::Llamacpp,
            ownership: Ownership::Managed,
            cold: false,
            passes_requested: 3,
            workload_id: "lab-fixed-v1".into(),
            prompt_sha256: "0".repeat(64),
            seed: 42,
            max_tokens: 96,
            model_name: "tiny.gguf".into(),
            model_sha256: None,
            runtime_version: None,
            backend: None,
            hardware_hash: None,
            requested: Settings::default(),
            variable: None,
            baseline_id: None,
        },
        observed: None,
        summary: None,
        failure: None,
        passes: vec![],
        findings: vec![Finding {
            rule_id: "DIAG-04".into(),
            status: FindingStatus::Triggered,
            warning: true,
            evidence: vec!["itl_p95_over_median: 3.1".into()],
            missing: vec![],
            follow_up: Some(Variable::FlashAttentionOn),
            message: "Latency is uneven.".into(),
        }],
    }
}

fn next(rx: &mut mpsc::Receiver<LabCommand>) -> LabCommand {
    rx.try_recv().expect("a command was sent")
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn check_run_cancel_and_follow_up_flow() {
    adw::init().expect("GTK display is available");
    let (commands, mut rx) = mpsc::channel(8);
    let lab = Lab::new(commands);
    let window = gtk::Window::builder()
        .default_width(360)
        .default_height(620)
        .child(&lab.widget)
        .build();
    window.present();
    pump();

    // Nothing runs before a Check; Run is not offered.
    assert!(!button(&lab, "Run").is_sensitive());
    assert!(!button(&lab, "Cancel").is_sensitive());
    button(&lab, "Check").emit_clicked();
    let check = match next(&mut rx) {
        LabCommand::Check { id, request } => {
            assert_eq!(request.passes, 3);
            assert!(request.follow_up.is_none());
            id
        }
        other => panic!("unexpected {other:?}"),
    };
    // A stale answer is dropped.
    lab.receive(&LabEvent::Checked {
        id: check + 100,
        result: Ok("stale".into()),
    });
    assert!(!button(&lab, "Run").is_sensitive());
    lab.receive(&LabEvent::Checked {
        id: check,
        result: Ok("Runs 3 passes.".into()),
    });
    pump();
    assert!(button(&lab, "Run").is_sensitive());

    // Changing the request withdraws the Check.
    lab.inner.passes.set_value(5.0);
    pump();
    assert!(!button(&lab, "Run").is_sensitive());
    button(&lab, "Check").emit_clicked();
    let check = match next(&mut rx) {
        LabCommand::Check { id, request } => {
            assert_eq!(request.passes, 5);
            id
        }
        other => panic!("unexpected {other:?}"),
    };
    lab.receive(&LabEvent::Checked {
        id: check,
        result: Ok("Runs 5 passes.".into()),
    });
    button(&lab, "Run").emit_clicked();
    let (run, cancel) = match next(&mut rx) {
        LabCommand::Run {
            id,
            request,
            cancel,
        } => {
            assert_eq!(request.passes, 5);
            (id, cancel)
        }
        other => panic!("unexpected {other:?}"),
    };
    pump();
    assert!(!button(&lab, "Run").is_sensitive(), "one run at a time");
    assert!(!button(&lab, "Check").is_sensitive());
    assert!(button(&lab, "Cancel").is_sensitive());
    lab.receive(&LabEvent::Progress {
        id: run,
        progress: Progress {
            phase: Phase::Measured,
            done: 2,
            of: 5,
        },
    });
    assert_eq!(lab.inner.progress_text.text(), "Measured pass 3 of 5");
    button(&lab, "Cancel").emit_clicked();
    assert!(cancel.load(Ordering::Relaxed));
    // A result for another run does not end this one.
    lab.receive(&LabEvent::Finished {
        id: run + 1,
        result: Err("other".into()),
    });
    assert!(!button(&lab, "Check").is_sensitive());
    lab.receive(&LabEvent::Finished {
        id: run,
        result: Ok(RunDone {
            experiment: Box::new(experiment()),
            notice: Some("The original profile could not be restarted.".into()),
        }),
    });
    pump();
    assert!(button(&lab, "Check").is_sensitive());
    assert!(
        !button(&lab, "Run").is_sensitive(),
        "one Check covers one Run"
    );
    assert!(matches!(next(&mut rx), LabCommand::List { .. }));
    assert!(lab.inner.result.text().contains("tiny.gguf"));
    assert!(lab.inner.notice.text().contains("could not be restarted"));

    // The triggered finding offers a controlled follow-up, which needs its own Check.
    button(&lab, "Check a follow-up").emit_clicked();
    match next(&mut rx) {
        LabCommand::Check { request, .. } => {
            assert_eq!(request.follow_up, Some((5, Variable::FlashAttentionOn)));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(!button(&lab, "Run").is_sensitive());

    // Two rows can be picked, and compared once both are.
    let rows: Vec<_> = [7, 6]
        .into_iter()
        .map(|id| {
            let mut e = experiment();
            e.id = id;
            ExperimentRow {
                id,
                state: RunState::Completed,
                started_at_unix_ms: 1,
                finished_at_unix_ms: Some(2),
                engine: Engine::Llamacpp,
                ownership: Ownership::Managed,
                cold: false,
                workload_id: "lab-fixed-v1".into(),
                model_name: "tiny<b>.gguf".into(),
                variable: None,
                baseline_id: None,
                summary: None,
            }
        })
        .collect();
    let list = lab.inner.next_id();
    lab.inner.state.borrow_mut().list = Some(list);
    lab.receive(&LabEvent::Listed {
        id: list,
        result: Ok(Listing {
            rows,
            unreadable: 1,
        }),
    });
    pump();
    assert!(lab.inner.history_note.text().contains("1 stored records"));
    assert!(!lab.inner.compare.is_sensitive());
    let mut picks = Vec::new();
    fn checks(widget: &gtk::Widget, out: &mut Vec<gtk::CheckButton>) {
        if let Ok(check) = widget.clone().downcast::<gtk::CheckButton>() {
            out.push(check);
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            checks(&current, out);
            child = current.next_sibling();
        }
    }
    checks(&lab.inner.history.clone().upcast(), &mut picks);
    assert_eq!(picks.len(), 2);
    picks[0].set_active(true);
    picks[1].set_active(true);
    pump();
    assert!(lab.inner.compare.is_sensitive());
    lab.inner.compare.emit_clicked();
    match next(&mut rx) {
        LabCommand::Compare { a, b, .. } => assert_eq!((a, b), (6, 7), "older run is the baseline"),
        other => panic!("unexpected {other:?}"),
    }

    // The window manager may resize the test window, so check the widget's own minimum.
    let (min_width, ..) = lab.widget.measure(gtk::Orientation::Horizontal, -1);
    assert!(min_width <= 360, "layout needs {min_width} pixels");
    window.close();
}
