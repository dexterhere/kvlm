// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared "how do I get a Hugging Face token" steps, used by the setup guide and the Models page.

use adw::prelude::*;
use gtk::gio;

pub const URL: &str = "https://huggingface.co/settings/tokens";
pub const NOTE: &str =
    "Public models need no token. You only need one for gated or private models.";
pub const STEPS: [&str; 4] = [
    "Sign in at huggingface.co, or create a free account.",
    "Open Settings, then Access Tokens, with the button below.",
    "Choose Create new token, pick the Read type and give it a name such as kvlm.",
    "Copy the token (it starts with hf_) and paste it into Kvlm under Models, Hugging Face access, then press Save.",
];

/// Opens the token page in the default browser. Called only from a button the user pressed.
pub fn open() {
    let _ = gio::AppInfo::launch_default_for_uri(URL, None::<&gio::AppLaunchContext>);
}

/// The numbered steps as a boxed list.
pub fn steps_list() -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for (index, step) in STEPS.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(*step)
            .title_lines(0)
            .use_markup(false)
            .build();
        let number = gtk::Label::builder()
            .label((index + 1).to_string())
            .css_classes(["dim-label", "heading"])
            .width_chars(2)
            .build();
        row.add_prefix(&number);
        list.append(&row);
    }
    list
}

pub fn open_button() -> gtk::Button {
    let button = gtk::Button::builder()
        .label("Open token page")
        .halign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    button.set_tooltip_text(Some("Opens huggingface.co in your browser"));
    button.connect_clicked(|_| open());
    button
}
