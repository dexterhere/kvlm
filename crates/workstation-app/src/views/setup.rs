// SPDX-License-Identifier: GPL-3.0-or-later
//! First-run setup guide: a short step-by-step dialog, one card at a time, that explains what the application does, lets the user
//! pick a colour scheme, explains models and runtimes, shows how to get a Hugging Face token and
//! summarises the hardware snapshot the window already took. It makes no request and starts no
//! probe itself; it stores only the marker file that says it was seen (and the theme, if changed).

use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib;
use workstation_core::hardware::HardwareProfile;

use crate::prefs::{self, Theme};

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

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// A rough starting point, not a fit check: it only reads the totals the hardware probe reported.
/// Q4-class files take roughly 0.6 GiB per billion parameters plus room for the context cache.
pub fn advice(profile: &HardwareProfile) -> String {
    let ram = profile
        .system_ram
        .as_ref()
        .map(|m| m.total_bytes as f64 / GIB);
    let vram = profile
        .gpus
        .iter()
        .filter_map(|gpu| gpu.dedicated_vram.as_ref())
        .map(|m| m.total_bytes as f64 / GIB)
        .fold(0.0_f64, f64::max);
    let cpu = profile.cpu.as_ref();
    let mut lines = Vec::new();
    if let Some(cpu) = cpu {
        lines.push(format!("Processor: {} logical cores", cpu.logical_cores));
    }
    if let Some(ram) = ram {
        lines.push(format!("Memory: {ram:.1} GiB"));
    }
    let Some(ram) = ram else {
        lines.push(
            "Memory could not be read, so no suggestion is made. Overview shows what was found."
                .into(),
        );
        return lines.join("\n");
    };
    if vram > 0.0 {
        lines.push(format!("Graphics memory: {vram:.1} GiB"));
        let size = match vram {
            v if v < 3.0 => "about 1B to 3B parameters",
            v if v < 5.0 => "up to about 3B to 4B parameters",
            v if v < 9.0 => "up to about 7B to 8B parameters",
            v if v < 14.0 => "up to about 13B parameters",
            _ => "13B parameters or larger",
        };
        lines.push(format!(
            "Suggestion: start with Q4-class models of {size}. Longer context needs more memory, so the Models page shows the exact fit before you download."
        ));
    } else {
        lines.push("No dedicated graphics memory found, so models run on the processor.".into());
        let size = if ram < 12.0 {
            "1B to 3B"
        } else {
            "up to about 7B"
        };
        lines.push(format!(
            "Suggestion: start with small Q4-class models ({size} parameters); expect them to be slower than on a GPU."
        ));
        if cpu.and_then(|c| c.features).is_some_and(|f| !f.avx2) {
            lines.push(
                "This processor did not report AVX2, which most llama.cpp builds expect.".into(),
            );
        }
    }
    lines.join("\n")
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

fn page(icon: &str, title: &str, body: &str, child: Option<&gtk::Widget>) -> adw::StatusPage {
    let page = adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(body)
        .vexpand(true)
        .build();
    if let Some(child) = child {
        page.set_child(Some(child));
    }
    page
}

fn fact_list(rows: &[(&str, &str, &str)]) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for (icon, title, subtitle) in rows {
        let row = adw::ActionRow::builder()
            .title(*title)
            .subtitle(*subtitle)
            .subtitle_lines(0)
            .use_markup(false)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        list.append(&row);
    }
    list
}

fn clamp(child: &impl IsA<gtk::Widget>) -> adw::Clamp {
    adw::Clamp::builder().maximum_size(440).child(child).build()
}

fn theme_choice() -> gtk::Box {
    let row = gtk::Box::builder()
        .css_classes(["linked"])
        .halign(gtk::Align::Center)
        .build();
    let current = prefs::theme();
    let mut group: Option<gtk::ToggleButton> = None;
    for theme in Theme::ALL {
        let button = gtk::ToggleButton::builder()
            .label(theme.label())
            .active(theme == current)
            .build();
        if let Some(first) = &group {
            button.set_group(Some(first));
        } else {
            group = Some(button.clone());
        }
        button.connect_toggled(move |button| {
            if button.is_active() {
                prefs::set_theme(theme);
            }
        });
        row.append(&button);
    }
    row
}

/// Shows the guide. `go` receives the name of the page to open when it closes. `scan` is polled
/// for the hardware snapshot the window is already taking; the guide never starts a probe itself.
pub fn present(
    parent: &adw::ApplicationWindow,
    scan: Rc<RefCell<Option<HardwareProfile>>>,
    go: impl Fn(&str) + 'static,
) {
    let go = Rc::new(go);
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Welcome to Kvlm")
        .default_width(560)
        .default_height(640)
        .build();
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

    let found = gtk::Label::builder()
        .label("Reading your hardware…")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["card"])
        .margin_start(12)
        .margin_end(12)
        .build();
    found.set_margin_top(6);
    found.set_margin_bottom(6);
    let start = {
        let overview = button("Open My computer", true);
        overview.connect_clicked(finish("overview"));
        let runtimes = button("Find my runtimes", false);
        runtimes.connect_clicked(finish("runtimes"));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
        column.append(&overview);
        column.append(&runtimes);
        column
    };

    let token_steps = {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.append(&super::token_help::steps_list());
        column.append(&super::token_help::open_button());
        clamp(&column)
    };
    let pages = [
        page(
            "applications-science-symbolic",
            "Run AI on your own computer",
            "Kvlm helps you pick a model that fits, shows what is running, and checks real speed. Nothing leaves this computer unless you press a button.",
            None,
        ),
        page(
            "preferences-desktop-appearance-symbolic",
            "Pick a look",
            "You can change this later in Preferences.",
            Some(theme_choice().upcast_ref()),
        ),
        page(
            "dialog-information-symbolic",
            "How local AI fits together",
            "Three parts work together. Kvlm helps with all of them.",
            Some(
                clamp(&fact_list(&[
                    ("folder-download-symbolic", "A model", "The AI itself, a large file you download once. Models from Hugging Face are the most common."),
                    ("system-run-symbolic", "A runtime", "The program that loads the model and answers. Ollama, LM Studio and vLLM are runtimes you install yourself; Kvlm only looks at them. llama.cpp is the engine inside many of them, and Kvlm can also install and run its own copy."),
                    ("computer-symbolic", "Your hardware", "Memory decides what fits. A graphics card (GPU) is much faster than the processor alone."),
                ]))
                .upcast_ref(),
            ),
        ),
        page(
            "channel-secure-symbolic",
            "A Hugging Face token (optional)",
            super::token_help::NOTE,
            Some(token_steps.upcast_ref()),
        ),
        page(
            "computer-symbolic",
            "What this computer can run",
            "A rough starting point from the hardware Kvlm just read.",
            Some(found.upcast_ref()),
        ),
        page(
            "object-select-symbolic",
            "You are ready",
            "Reopen this guide any time from the main menu.",
            Some(start.upcast_ref()),
        ),
    ];

    // A stack with a cross-fade shows exactly one card at a time; a sliding carousel shows the
    // neighbouring cards while it moves.
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(180)
        .hhomogeneous(true)
        .vexpand(true)
        .build();
    for (index, page) in pages.iter().enumerate() {
        stack.add_named(page, Some(&index.to_string()));
    }
    let shown = Rc::new(Cell::new(0_usize));
    let dots = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let dot_widgets: Vec<gtk::Label> = pages
        .iter()
        .map(|_| {
            let dot = gtk::Label::builder()
                .label("\u{25cf}")
                .css_classes(["dim-label"])
                .build();
            dots.append(&dot);
            dot
        })
        .collect();
    dots.update_property(&[gtk::accessible::Property::Label("Setup progress")]);
    let back = gtk::Button::builder().label("Back").build();
    back.add_css_class("flat");
    let next = button("Next", true);
    let skip = gtk::Button::builder().label("Skip").build();
    skip.add_css_class("flat");
    skip.connect_clicked(finish("overview"));
    let bar = gtk::CenterBox::builder()
        .margin_start(18)
        .margin_end(18)
        .margin_bottom(18)
        .build();
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    left.append(&back);
    left.append(&skip);
    bar.set_start_widget(Some(&left));
    bar.set_center_widget(Some(&dots));
    bar.set_end_widget(Some(&next));

    let last = pages.len() - 1;
    let show = {
        let stack = stack.downgrade();
        let (back, next, skip) = (back.downgrade(), next.downgrade(), skip.downgrade());
        let shown = shown.clone();
        move |index: usize| {
            shown.set(index);
            if let Some(stack) = stack.upgrade() {
                stack.set_visible_child_name(&index.to_string());
            }
            for (position, dot) in dot_widgets.iter().enumerate() {
                if position == index {
                    dot.remove_css_class("dim-label");
                    dot.add_css_class("accent");
                } else {
                    dot.remove_css_class("accent");
                    dot.add_css_class("dim-label");
                }
            }
            if let (Some(back), Some(next), Some(skip)) =
                (back.upgrade(), next.upgrade(), skip.upgrade())
            {
                back.set_sensitive(index > 0);
                next.set_visible(index < last);
                skip.set_visible(index < last);
            }
        }
    };
    let show = Rc::new(show);
    show(0);
    next.connect_clicked({
        let (show, shown) = (show.clone(), shown.clone());
        move |_| show((shown.get() + 1).min(last))
    });
    back.connect_clicked({
        let (show, shown) = (show.clone(), shown.clone());
        move |_| show(shown.get().saturating_sub(1))
    });

    {
        let found = found.downgrade();
        let mut waited = 0;
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            let Some(found) = found.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Some(profile) = scan.borrow().as_ref() {
                found.set_label(&advice(profile));
                return glib::ControlFlow::Break;
            }
            waited += 1;
            if waited > 60 {
                found.set_label("The scan is taking long. My computer shows it when it finishes.");
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&stack);
    content.append(&bar);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    toolbar.set_content(Some(&content));
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

    fn profile(ram_gib: u64, vram_gib: Option<u64>) -> HardwareProfile {
        use workstation_core::hardware::{GpuDeviceProfile, GpuVendor, MemoryPool};
        let pool = |gib: u64| MemoryPool {
            total_bytes: gib << 30,
            available_bytes: None,
        };
        HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: Some(pool(ram_gib)),
            gpus: vram_gib
                .map(|gib| GpuDeviceProfile {
                    id: None,
                    name: "test".into(),
                    vendor: GpuVendor::Nvidia,
                    dedicated_vram: Some(pool(gib)),
                    shared_gtt: None,
                    supported_backends: Vec::new(),
                    unified_memory: None,
                })
                .into_iter()
                .collect(),
            issues: Vec::new(),
            captured_at_unix_ms: 0,
        }
    }

    #[test]
    fn advice_follows_the_reported_memory() {
        assert!(advice(&profile(16, Some(6))).contains("7B to 8B"));
        assert!(advice(&profile(8, None)).contains("1B to 3B"));
        assert!(advice(&profile(32, None)).contains("up to about 7B"));
        let mut unknown = profile(8, None);
        unknown.system_ram = None;
        assert!(advice(&unknown).contains("no suggestion"));
    }

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
