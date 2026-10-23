// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use gtk::{gio, glib};
use workstation_core::{APP_ID, APP_NAME};

pub fn run(runtime: &tokio::runtime::Handle) -> glib::ExitCode {
    glib::set_application_name(APP_NAME);
    let app = adw::Application::builder().application_id(APP_ID).build();
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
    app.set_accels_for_action("app.quit", &["<Primary>q"]);
    app.run()
}
