// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK coverage for the downloads panel; rows are typed fixtures.

use super::*;
use workstation_runtimes::downloader::DownloadOutcome;

fn row(id: i64, state: DownloadState) -> DownloadRow {
    DownloadRow {
        id,
        artifact_id: "hf:fixture".into(),
        repo_id: "o/n".into(),
        revision: "a".repeat(40),
        filename: "m\n.gguf".into(),
        expected_sha256: "ab".repeat(32),
        expected_size: 1000,
        state,
        bytes_done: 250,
        failure_code: None,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    }
}

fn pump(commands: &mut mpsc::Receiver<CoreCommand>) -> CoreCommand {
    let ctx = gtk::glib::MainContext::default();
    for _ in 0..200 {
        while ctx.iteration(false) {}
        if let Ok(command) = commands.try_recv() {
            return command;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("no command was sent");
}

fn first_button(widget: &adw::ActionRow, label: &str) -> gtk::Button {
    fn find(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && button.label().as_deref() == Some(label)
        {
            return Some(button.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = find(&current, label) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }
    find(widget.upcast_ref(), label).unwrap_or_else(|| panic!("no {label} button"))
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn rows_progress_and_two_step_discard() {
    adw::init().expect("GTK display is available");
    let (commands, mut rx) = mpsc::channel(8);
    let panel = Downloads::new(commands);
    gtk::Window::builder()
        .child(&panel.widget)
        .build()
        .present();
    assert!(!panel.widget.is_visible());

    panel.load();
    let CoreCommand::LoadDownloads { id } = pump(&mut rx) else {
        panic!("load")
    };
    panel.receive(&UiEvent::DownloadsLoaded {
        id: id + 9,
        result: Ok(vec![row(1, DownloadState::Running)]),
    });
    assert!(!panel.widget.is_visible(), "stale answer ignored");
    panel.receive(&UiEvent::DownloadsLoaded {
        id,
        result: Ok(vec![
            row(1, DownloadState::Running),
            row(2, DownloadState::Paused),
        ]),
    });
    assert!(panel.widget.is_visible());
    let (running, paused) = {
        let state = panel.state.borrow();
        (state.rows[&1].row.clone(), state.rows[&2].row.clone())
    };
    assert_eq!(running.title(), "m?.gguf");

    panel.receive(&UiEvent::DownloadProgress(Progress {
        download_id: 1,
        bytes_done: 500,
        total: 1000,
    }));
    assert!((panel.state.borrow().rows[&1].bar.fraction() - 0.5).abs() < 1e-9);

    first_button(&running, "Pause").emit_clicked();
    assert!(matches!(pump(&mut rx), CoreCommand::PauseDownload));
    first_button(&paused, "Resume").emit_clicked();
    let CoreCommand::StartDownload { artifact, .. } = pump(&mut rx) else {
        panic!("resume")
    };
    assert_eq!(artifact.sha256.as_deref(), Some("ab".repeat(32).as_str()));

    let discard = first_button(&paused, "Discard");
    discard.emit_clicked();
    assert!(rx.try_recv().is_err(), "first click only arms");
    first_button(&paused, "Confirm discard").emit_clicked();
    assert!(matches!(
        pump(&mut rx),
        CoreCommand::DiscardDownload { download_id: 2, .. }
    ));

    panel.receive(&UiEvent::DownloadFinished {
        outcome: DownloadOutcome {
            download_id: Some(1),
            bytes_done: 5,
            end: DownloadEnd::Cancelled,
        },
    });
    assert!(panel.status.text().contains("cancelled"));
    assert!(matches!(pump(&mut rx), CoreCommand::LoadDownloads { .. }));
}
