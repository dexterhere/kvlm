// SPDX-License-Identifier: GPL-3.0-or-later
//! First-run setup guide. It explains what the application does and does not do, then sends the
//! user to the page that performs the first check. It makes no request, reads no hardware and
//! changes nothing itself; the only thing it stores is an empty marker file that says it was seen.

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib;
use workstation_core::hardware::HardwareProfile;

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
        .title("Setup guide")
        .default_width(520)
        .default_height(560)
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

    // Quick scan page, built first because the last intro step pushes it.
    let found = gtk::Label::builder()
        .label("Reading your hardware…")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let to_start = button("Continue", true);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 16);
    column.append(&found);
    column.append(&to_start);
    let scan_page = adw::NavigationPage::builder()
        .title("Your hardware")
        .child(
            &adw::StatusPage::builder()
                .icon_name("computer-symbolic")
                .title("What this computer can run")
                .child(&column)
                .build(),
        )
        .build();
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
                found.set_label("The scan is taking long. Overview shows it when it finishes.");
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    let last = {
        let hardware = button("Open Overview", true);
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
        adw::NavigationPage::builder()
            .title("Start")
            .child(
                &adw::StatusPage::builder()
                    .icon_name("object-select-symbolic")
                    .title("You are set")
                    .description(
                        "Pick where to start. You can reopen this guide from the main menu.",
                    )
                    .child(&column)
                    .build(),
            )
            .build()
    };
    to_start.connect_clicked({
        let nav = nav.downgrade();
        move |_| {
            if let Some(nav) = nav.upgrade() {
                nav.push(&last);
            }
        }
    });

    // The intro pages chain: each Next pushes the following page, the last pushes the scan.
    let mut following = vec![scan_page];
    for step in STEPS.iter().skip(1).rev() {
        let next = button("Next", true);
        let page = page(step, &next);
        let target = following.last().expect("seeded above").clone();
        next.connect_clicked({
            let nav = nav.downgrade();
            move |_| {
                if let Some(nav) = nav.upgrade() {
                    nav.push(&target);
                }
            }
        });
        following.push(page);
    }
    let first_next = button("Next", true);
    let first = page(&STEPS[0], &first_next);
    let second = following.last().expect("at least one page").clone();
    first_next.connect_clicked({
        let nav = nav.downgrade();
        move |_| {
            if let Some(nav) = nav.upgrade() {
                nav.push(&second);
            }
        }
    });
    nav.add(&first);

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
