// SPDX-License-Identifier: GPL-3.0-or-later
//! Preferences window: appearance, the setup guide and a plain statement of what Kvlm never does.

use adw::prelude::*;

use crate::prefs::{self, Theme};

pub fn present(parent: &adw::ApplicationWindow, open_setup: impl Fn() + 'static) {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .title("Preferences")
        .default_width(520)
        .default_height(480)
        .build();
    let page = adw::PreferencesPage::new();

    let appearance = adw::PreferencesGroup::builder()
        .title("Appearance")
        .description("Choose light or dark, or let your desktop decide.")
        .build();
    let labels: Vec<&str> = Theme::ALL.iter().map(|theme| theme.label()).collect();
    let theme = adw::ComboRow::builder()
        .title("Colour scheme")
        .model(&gtk::StringList::new(&labels))
        .selected(
            Theme::ALL
                .iter()
                .position(|candidate| *candidate == prefs::theme())
                .unwrap_or(0) as u32,
        )
        .build();
    theme.connect_selected_notify(|row| {
        if let Some(choice) = Theme::ALL.get(row.selected() as usize) {
            prefs::set_theme(*choice);
        }
    });
    appearance.add(&theme);
    page.add(&appearance);

    let help = adw::PreferencesGroup::builder().title("Help").build();
    let guide = adw::ActionRow::builder()
        .title("Setup guide")
        .subtitle("A short tour of what Kvlm does, with a quick look at this computer.")
        .activatable(true)
        .build();
    guide.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let weak = window.downgrade();
    guide.connect_activated(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
        open_setup();
    });
    help.add(&guide);
    page.add(&help);

    let privacy = adw::PreferencesGroup::builder()
        .title("Privacy")
        .description("These are fixed and cannot be turned on by accident.")
        .build();
    for (title, subtitle) in [
        ("No telemetry", "Kvlm never reports usage or crashes."),
        (
            "Nothing at startup",
            "The network is used only when you press a button such as a download or an update check.",
        ),
        (
            "Your runtimes stay yours",
            "Ollama, LM Studio and vLLM are read, never started, stopped or changed.",
        ),
    ] {
        privacy.add(
            &adw::ActionRow::builder()
                .title(title)
                .subtitle(subtitle)
                .use_markup(false)
                .build(),
        );
    }
    page.add(&privacy);
    window.add(&page);
    window.present();
}
