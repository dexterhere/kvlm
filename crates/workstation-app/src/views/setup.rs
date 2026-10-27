// SPDX-License-Identifier: GPL-3.0-or-later
//! First-run setup guide. It explains what the application does and does not do, then sends the
//! user to the page that performs the first check. It makes no request, reads no hardware and
//! changes nothing itself; the only thing it stores is an empty marker file that says it was seen.

use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib;

const MARKER: &str = "local-ai-workstation/setup-seen";

fn marker() -> PathBuf {
    glib::user_config_dir().join(MARKER)
}

fn seen(path: &Path) -> bool {
    path.exists()
}

fn remember(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, b"");
}

/// True until the guide has been finished or skipped once.
pub fn needed() -> bool {
    !seen(&marker())
}

struct Step {
    title: &'static str,
    icon: &'static str,
    body: &'static str,
}

const STEPS: [Step; 3] = [
    Step {
        title: "Welcome to Kvlm",
        icon: "computer-symbolic",
        body: "Kvlm tells you whether a local model will fit your memory, shows what your runtimes are doing, and checks the answer with a benchmark.\n\nIt sends no telemetry, makes no network request when it starts, and needs no administrator rights.",
    },
    Step {
        title: "Your runtimes stay yours",
        icon: "network-server-symbolic",
        body: "Ollama, LM Studio and vLLM are read, never started, stopped or changed. Kvlm looks for them only on this computer.\n\nOnly a llama.cpp server that Kvlm installs itself is managed by Kvlm.",
    },
    Step {
        title: "You choose when anything runs",
        icon: "system-run-symbolic",
        body: "Benchmarks use your CPU, GPU and memory, and only after you press Check and then Run. Downloads and the update check happen only when you press their buttons.\n\nA Hugging Face token, if you add one, is kept in your system keyring.",
    },
];

fn page(step: &Step, button: &gtk::Button) -> adw::NavigationPage {
    let status = adw::StatusPage::builder()
        .icon_name(step.icon)
        .title(step.title)
        .description(step.body)
        .child(button)
        .build();
    adw::NavigationPage::builder()
        .title(step.title)
        .child(&status)
        .build()
}

fn button(label: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Center)
        .build();
    button.add_css_class("pill");
    if suggested {
        button.add_css_class("suggested-action");
    }
    button
}

/// Shows the guide. `go` receives the name of the page to open when it closes.
pub fn present(parent: &adw::ApplicationWindow, go: impl Fn(&str) + 'static) {
    let go = Rc::new(go);
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Setup guide")
        .default_width(520)
        .default_height(520)
        .build();
    let nav = adw::NavigationView::new();
    let finish = |target: &'static str| {
        let go = go.clone();
        let dialog = dialog.downgrade();
        move |_: &gtk::Button| {
            remember(&marker());
            go(target);
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    };

    let next = button("Next", true);
    nav.add(&page(&STEPS[0], &next));
    let nav_second = nav.downgrade();
    let second = button("Next", true);
    let second_page = page(&STEPS[1], &second);
    nav.add(&second_page);
    let third = button("Next", true);
    let third_page = page(&STEPS[2], &third);
    nav.add(&third_page);
    let nav_first = nav.downgrade();
    next.connect_clicked(move |_| {
        if let Some(nav) = nav_first.upgrade() {
            nav.push(&second_page);
        }
    });
    second.connect_clicked(move |_| {
        if let Some(nav) = nav_second.upgrade() {
            nav.push(&third_page);
        }
    });

    let hardware = button("Check my hardware", true);
    hardware.connect_clicked(finish("overview"));
    let runtimes = button("Find my runtimes", false);
    runtimes.connect_clicked(finish("runtimes"));
    let later = button("Skip for now", false);
    later.add_css_class("flat");
    later.connect_clicked(finish("overview"));
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&hardware);
    column.append(&runtimes);
    column.append(&later);
    let last = adw::StatusPage::builder()
        .icon_name("object-select-symbolic")
        .title("You are set")
        .description("Pick where to start. You can reopen this guide from the main menu.")
        .child(&column)
        .build();
    third.connect_clicked({
        let nav = nav.downgrade();
        let last = adw::NavigationPage::builder()
            .title("Start")
            .child(&last)
            .build();
        move |_| {
            if let Some(nav) = nav.upgrade() {
                nav.push(&last);
            }
        }
    });

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&nav));
    dialog.set_content(Some(&toolbar));
    // Closing with the window button counts as seeing it, so it never nags.
    dialog.connect_close_request(|_| {
        remember(&marker());
        glib::Propagation::Proceed
    });
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trip_in_an_isolated_directory() {
        let dir = std::env::temp_dir().join(format!("kvlm-setup-test-{}", std::process::id()));
        let path = dir.join("nested/setup-seen");
        assert!(!seen(&path));
        remember(&path);
        assert!(seen(&path));
        let _ = fs::remove_dir_all(&dir);
    }
}
