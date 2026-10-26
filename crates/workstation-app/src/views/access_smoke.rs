// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in GTK coverage for the token panel. No keyring is contacted.

use super::*;

fn pump(commands: &mut mpsc::Receiver<CoreCommand>) -> Option<CoreCommand> {
    let ctx = gtk::glib::MainContext::default();
    for _ in 0..100 {
        while ctx.iteration(false) {}
        if let Ok(command) = commands.try_recv() {
            return Some(command);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

#[test]
#[ignore = "requires a graphical GTK display"]
fn saving_clears_the_box_and_answers_update_the_row() {
    adw::init().expect("GTK display is available");
    let (commands, mut rx) = mpsc::channel(8);
    let access = Access::new(commands);
    gtk::Window::builder()
        .child(&access.widget)
        .build()
        .present();
    assert!(pump(&mut rx).is_none(), "nothing is sent on startup");

    // A malformed token is refused locally, cleared, and never echoed.
    access.entry.set_text("not a token!");
    access.buttons[0].emit_clicked();
    assert!(pump(&mut rx).is_none());
    assert_eq!(access.entry.text(), "");
    assert!(!access.row.subtitle().contains("not a token"));

    access.entry.set_text("  hf_abc123\n");
    access.buttons[0].emit_clicked();
    let Some(CoreCommand::SaveToken { id, token }) = pump(&mut rx) else {
        panic!("save")
    };
    assert_eq!(access.entry.text(), "", "the pasted token leaves the box");
    assert!(!format!("{token:?}").contains("abc123"));
    assert!(!access.buttons[0].is_sensitive(), "one action at a time");

    // A second press while waiting sends nothing.
    access.buttons[1].emit_clicked();
    assert!(pump(&mut rx).is_none());

    access.receive(&UiEvent::TokenDone {
        id: id + 5,
        result: Err("stale".into()),
    });
    assert!(
        !access.buttons[0].is_sensitive(),
        "stale answers are ignored"
    );
    access.receive(&UiEvent::TokenDone {
        id,
        result: Ok(TokenOutcome::Saved),
    });
    assert!(access.buttons[0].is_sensitive());
    assert!(access.row.subtitle().contains("Saved to the keyring"));

    access.buttons[2].emit_clicked();
    let Some(CoreCommand::CheckToken { id }) = pump(&mut rx) else {
        panic!("check")
    };
    access.receive(&UiEvent::TokenDone {
        id,
        result: Ok(TokenOutcome::Valid {
            name: "some\nbody".into(),
        }),
    });
    assert!(access.row.subtitle().contains("some\nbody") || access.row.subtitle().contains("some"));
    access.buttons[3].emit_clicked();
    assert!(matches!(
        pump(&mut rx),
        Some(CoreCommand::ClearToken { .. })
    ));
}
