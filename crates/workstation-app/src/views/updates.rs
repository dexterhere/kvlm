// SPDX-License-Identifier: GPL-3.0-or-later
//! "Updates" row on the Overview page. Pressing the button makes one request to the public
//! release feed; nothing is checked at startup or on a timer, and nothing is downloaded.

use adw::prelude::*;
use workstation_runtimes::update::{self, Outcome};

const IDLE: &str = "Not checked. This makes one request to GitHub, only when you press the button, and sends nothing about this computer.";

fn describe(result: &Result<Outcome, update::UpdateError>) -> String {
    match result {
        Ok(Outcome::UpToDate { latest }) => {
            format!("You are up to date. The latest release is {latest}.")
        }
        Ok(Outcome::Newer { latest, link }) => {
            format!("Version {latest} is available. Download it yourself from {link}")
        }
        Err(error) => error.to_string(),
    }
}

/// The row; the button is its only control.
pub fn new(runtime: &tokio::runtime::Handle) -> gtk::ListBox {
    {
        let row = adw::ActionRow::builder()
            .title("Updates")
            .subtitle(IDLE)
            .subtitle_selectable(true)
            .use_markup(false)
            .build();
        let check = gtk::Button::builder()
            .label("Check for updates")
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&check);
        let widget = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        widget.append(&row);
        let pending = std::rc::Rc::new(std::cell::Cell::new(false));
        let handle = runtime.clone();
        let weak_row = row.downgrade();
        let weak_button = check.downgrade();
        check.connect_clicked(move |button| {
            if pending.replace(true) {
                return;
            }
            button.set_sensitive(false);
            if let Some(row) = weak_row.upgrade() {
                row.set_subtitle("Checking…");
            }
            let (sender, receiver) = async_channel::bounded(1);
            handle.spawn(async move {
                let _ = sender
                    .send(update::check(env!("CARGO_PKG_VERSION")).await)
                    .await;
            });
            let (pending, row, button) = (pending.clone(), weak_row.clone(), weak_button.clone());
            gtk::glib::MainContext::default().spawn_local(async move {
                if let Ok(result) = receiver.recv().await {
                    if let Some(row) = row.upgrade() {
                        row.set_subtitle(&describe(&result));
                    }
                    if let Some(button) = button.upgrade() {
                        button.set_sensitive(true);
                    }
                }
                pending.set(false);
            });
        });
        widget
    }
}
