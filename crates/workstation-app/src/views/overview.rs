// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bridge::{CoreCommand, UiEvent};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc};
use workstation_core::hardware::{ComputeBackend, HardwareProfile, MemoryPool};

#[derive(Default)]
struct LoadState {
    loading: bool,
    has_snapshot: bool,
    stale: bool,
}

impl LoadState {
    fn begin(&mut self) -> bool {
        if self.loading {
            return false;
        }
        self.loading = true;
        true
    }
    fn finish(&mut self, success: bool) {
        self.loading = false;
        if success {
            self.has_snapshot = true;
        }
        self.stale = !success && self.has_snapshot;
    }
}

pub struct Overview {
    pub widget: gtk::ScrolledWindow,
    pub refresh: gtk::Button,
    spinner: gtk::Spinner,
    status: gtk::Label,
    readings: gtk::Box,
    state: Rc<RefCell<LoadState>>,
    commands: tokio::sync::mpsc::Sender<CoreCommand>,
    fit: super::model_fit::ModelFit,
}

impl Overview {
    pub fn new(commands: tokio::sync::mpsc::Sender<CoreCommand>) -> Self {
        let refresh = gtk::Button::with_mnemonic("_Refresh");
        refresh.set_tooltip_text(Some("Refresh hardware readings (Ctrl+R)"));
        let spinner = gtk::Spinner::new();
        let status = gtk::Label::builder()
            .label("Hardware has not been detected yet.")
            .wrap(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let heading = gtk::Label::builder()
            .label("Overview & Monitor")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build();
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.append(&spinner);
        controls.append(&status);
        controls.append(&refresh);
        let readings = gtk::Box::new(gtk::Orientation::Vertical, 24);
        let fit = super::model_fit::ModelFit::new(commands.clone());
        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .build();
        layout.append(&heading);
        layout.append(&controls);
        layout.append(&fit.widget);
        layout.append(&readings);
        let clamp = adw::Clamp::builder()
            .maximum_size(900)
            .child(&layout)
            .build();
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .vexpand(true)
            .build();
        let state = Rc::new(RefCell::new(LoadState::default()));
        let overview = Self {
            widget,
            refresh,
            spinner,
            status,
            readings,
            state,
            commands,
            fit,
        };
        let state = Rc::clone(&overview.state);
        let spinner = overview.spinner.clone();
        let status = overview.status.clone();
        let commands = overview.commands.clone();
        overview
            .refresh
            .connect_clicked(move |button| request(&state, button, &spinner, &status, &commands));
        overview
    }

    pub fn request(&self) {
        request(
            &self.state,
            &self.refresh,
            &self.spinner,
            &self.status,
            &self.commands,
        );
    }

    pub fn receive(&self, event: &UiEvent) {
        if let UiEvent::FitFinished { id, result } = event {
            self.fit.receive(*id, result);
            return;
        }
        if !matches!(
            event,
            UiEvent::HardwareDetected(_) | UiEvent::HardwareFailed(_)
        ) {
            return;
        }
        self.spinner.stop();
        self.refresh.set_sensitive(true);
        match event {
            UiEvent::HardwareDetected(profile) => {
                self.fit.hardware_reading(
                    profile.system_ram.and_then(|pool| pool.available_bytes),
                    profile.captured_at_unix_ms,
                );
                self.state.borrow_mut().finish(true);
                while let Some(child) = self.readings.first_child() {
                    self.readings.remove(&child);
                }
                self.render(profile);
                let coverage = if profile.issues.is_empty() {
                    "Hardware detected"
                } else {
                    "Partial hardware readings · See probe notices"
                };
                self.status.set_text(&format!(
                    "{coverage}\nSnapshot captured at {}",
                    gtk::glib::DateTime::from_unix_local(
                        (profile.captured_at_unix_ms / 1000) as i64
                    )
                    .and_then(|time| time.format("%H:%M:%S"))
                    .map(|text| text.to_string())
                    .unwrap_or_else(|_| "unknown time".into())
                ));
            }
            UiEvent::HardwareFailed(message) => {
                self.fit.hardware_failed();
                self.state.borrow_mut().finish(false);
                let prefix = if self.state.borrow().stale {
                    "Refresh failed · Showing previous readings"
                } else {
                    "Hardware detection failed"
                };
                self.status.set_text(&format!("{prefix}\n{message}"));
            }
            _ => unreachable!("unrelated events are handled above"),
        }
    }

    fn render(&self, profile: &HardwareProfile) {
        let processor = group("Processor");
        if let Some(cpu) = &profile.cpu {
            processor.add(&row("Model", cpu.model.as_deref().unwrap_or("Unknown")));
            processor.add(&row(
                "Cores",
                &format!(
                    "{} physical · {} logical",
                    cpu.physical_cores
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "Unknown".into()),
                    cpu.logical_cores
                ),
            ));
            let features = cpu
                .features
                .map(|flags| {
                    let enabled: Vec<_> = [
                        ("AVX2", flags.avx2),
                        ("AVX-512", flags.avx512),
                        ("VNNI", flags.vnni),
                        ("AMX", flags.amx),
                    ]
                    .into_iter()
                    .filter_map(|(name, enabled)| enabled.then_some(name))
                    .collect();
                    if enabled.is_empty() {
                        "None of the listed instruction sets reported".into()
                    } else {
                        enabled.join(", ")
                    }
                })
                .unwrap_or_else(|| "Unknown".into());
            processor.add(&row("CPU instruction sets", &features));
        } else {
            processor.add(&row(
                "CPU information unavailable",
                "See probe notices below.",
            ));
        }
        self.readings.append(&processor);
        let ram = group("System memory");
        ram.add(&row(
            "Available / total RAM",
            &memory_text(profile.system_ram),
        ));
        ram.set_description(Some(
            "Available RAM uses Linux MemAvailable; shared GPU memory overlaps this pool.",
        ));
        self.readings.append(&ram);
        if profile.gpus.is_empty() {
            let graphics = group("Graphics");
            graphics.add(&row("No GPU readings", "No device was reported by the available probes. See notices for probe limitations."));
            self.readings.append(&graphics);
        }
        for gpu in &profile.gpus {
            let graphics = group("Graphics device");
            graphics.add(&row("Model", &gpu.name));
            graphics.add(&row("Dedicated VRAM", &memory_text(gpu.dedicated_vram)));
            graphics.add(&row(
                "Shared GPU memory (GTT)",
                &memory_text(gpu.shared_gtt),
            ));
            let capability = gpu
                .supported_backends
                .iter()
                .map(|backend| match backend {
                    ComputeBackend::Cuda {
                        compute_capability: (major, minor),
                        driver_version,
                    } => format!("CUDA compute {major}.{minor} · Driver {driver_version}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            graphics.add(&row(
                "Reported hardware capability",
                if capability.is_empty() {
                    "Not verified"
                } else {
                    &capability
                },
            ));
            graphics.set_description(Some("Memory pools are not additive. Inference runtime acceleration has not been tested."));
            self.readings.append(&graphics);
        }
        if !profile.issues.is_empty() {
            let notices = group("Probe notices");
            for issue in &profile.issues {
                notices.add(&row(&issue.component, &issue.message));
            }
            self.readings.append(&notices);
        }
    }
}

fn request(
    state: &RefCell<LoadState>,
    button: &gtk::Button,
    spinner: &gtk::Spinner,
    status: &gtk::Label,
    commands: &tokio::sync::mpsc::Sender<CoreCommand>,
) {
    if !state.borrow_mut().begin() {
        return;
    }
    button.set_sensitive(false);
    spinner.start();
    status.set_text("Detecting hardware…");
    if commands.try_send(CoreCommand::DetectHardware).is_err() {
        state.borrow_mut().finish(false);
        button.set_sensitive(true);
        spinner.stop();
        status.set_text(if state.borrow().stale {
            "Worker unavailable · Showing previous readings"
        } else {
            "Hardware worker unavailable. Close and reopen the application to retry."
        });
    }
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .use_markup(false)
        .subtitle_lines(0)
        .build()
}

fn memory_text(memory: Option<MemoryPool>) -> String {
    match memory {
        None => "Not available".into(),
        Some(memory) => format!(
            "{} available / {:.2} GiB total",
            memory
                .available_bytes
                .map(|bytes| format!("{:.2} GiB", bytes as f64 / 1073741824.0))
                .unwrap_or_else(|| "Unknown".into()),
            memory.total_bytes as f64 / 1073741824.0
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_snapshot_as_stale_after_failure_and_recovers() {
        let mut state = LoadState::default();
        assert!(state.begin());
        assert!(!state.begin());
        state.finish(false);
        assert!(!state.has_snapshot);
        assert!(!state.stale);
        assert!(state.begin());
        state.finish(true);
        assert!(state.has_snapshot);
        assert!(state.begin());
        state.finish(false);
        assert!(state.stale);
        assert!(state.begin());
        state.finish(true);
        assert!(!state.stale);
    }

    #[test]
    fn memory_display_distinguishes_unknown_from_zero() {
        assert_eq!(memory_text(None), "Not available");
        assert!(
            memory_text(Some(MemoryPool {
                total_bytes: 1024,
                available_bytes: None
            }))
            .contains("Unknown")
        );
        assert!(
            memory_text(Some(MemoryPool {
                total_bytes: 1024,
                available_bytes: Some(0)
            }))
            .starts_with("0.00 GiB")
        );
    }
}
