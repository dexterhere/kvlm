// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use gtk::{gio, glib};
use workstation_core::{APP_ID, APP_NAME};

pub fn run(runtime: &tokio::runtime::Handle) -> glib::ExitCode {
    glib::set_application_name(APP_NAME);
    let app = adw::Application::builder().application_id(APP_ID).build();
    let signal_runtime = runtime.clone();
    let runtime = runtime.clone();

    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
        } else {
            crate::window::build(app, &runtime).present();
        }
    });

    let quit = gio::SimpleAction::new("quit", None);
    let weak_app = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = weak_app.upgrade() {
            app.quit();
        }
    });
    app.add_action(&quit);
    // A terminal interrupt or a session shutdown must still stop the managed server, which
    // happens after `run` returns; the default handlers would end the process first.
    let (signalled, interrupted) = async_channel::bounded::<()>(1);
    signal_runtime.spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut int), Ok(mut term), Ok(mut hup)) = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            return;
        };
        tokio::select! {
            _ = int.recv() => {}
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
        let _ = signalled.send(()).await;
    });
    let weak_app = app.downgrade();
    glib::MainContext::default().spawn_local(async move {
        if interrupted.recv().await.is_ok()
            && let Some(app) = weak_app.upgrade()
        {
            app.quit();
        }
    });
    app.set_accels_for_action("app.quit", &["<Primary>q"]);
    app.run()
}
