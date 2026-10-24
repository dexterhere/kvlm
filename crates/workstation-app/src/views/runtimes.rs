// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bridge::{CoreCommand, UiEvent};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc};
use workstation_runtimes::ollama::{Discovery, ENDPOINT, InstalledModel, RunningModel};

const MAX_VISIBLE_MODELS: usize = 100;
const MAX_TEXT_CHARACTERS: usize = 1024;

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
        self.has_snapshot |= success;
        self.stale = !success && self.has_snapshot;
    }
}

pub struct Runtimes {
    pub widget: gtk::ScrolledWindow,
    pub refresh: gtk::Button,
    spinner: gtk::Spinner,
    status: gtk::Label,
    readings: gtk::Box,
    state: Rc<RefCell<LoadState>>,
    commands: tokio::sync::mpsc::Sender<CoreCommand>,
}

impl Runtimes {
    pub fn new(commands: tokio::sync::mpsc::Sender<CoreCommand>) -> Self {
        let refresh = gtk::Button::with_mnemonic("_Refresh");
        refresh.set_tooltip_text(Some("Refresh attached Ollama discovery (Ctrl+R)"));
        let spinner = gtk::Spinner::new();
        let status = gtk::Label::builder()
            .label("Ollama has not been checked yet.")
            .wrap(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let heading = gtk::Label::builder()
            .label("Runtimes & Endpoints")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build();
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.append(&spinner);
        controls.append(&status);
        controls.append(&refresh);
        let readings = gtk::Box::new(gtk::Orientation::Vertical, 24);
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
        let runtimes = Self {
            widget,
            refresh,
            spinner,
            status,
            readings,
            state: Rc::new(RefCell::new(LoadState::default())),
            commands,
        };
        let state = Rc::clone(&runtimes.state);
        let spinner = runtimes.spinner.clone();
        let status = runtimes.status.clone();
        let commands = runtimes.commands.clone();
        runtimes
            .refresh
            .connect_clicked(move |button| request(&state, button, &spinner, &status, &commands));
        runtimes
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
        match event {
            UiEvent::OllamaDiscovered(discovery) => {
                self.state.borrow_mut().finish(true);
                while let Some(child) = self.readings.first_child() {
                    self.readings.remove(&child);
                }
                self.render(discovery);
                self.status.set_text(
                    if discovery.installed.is_ok() && discovery.running.is_ok() {
                        "Connected to Ollama · Attached"
                    } else {
                        "Connected to Ollama · Some model readings unavailable"
                    },
                );
            }
            UiEvent::OllamaFailed(message) => {
                self.state.borrow_mut().finish(false);
                let prefix = if self.state.borrow().stale {
                    "Ollama unavailable · Showing previous snapshot (stale)"
                } else {
                    "Ollama unavailable"
                };
                self.status.set_text(&format!(
                    "{prefix}\n{message}\nStart your Ollama service separately, then Refresh to retry."
                ));
            }
            _ => return,
        }
        self.spinner.stop();
        self.refresh.set_sensitive(true);
    }

    fn render(&self, discovery: &Discovery) {
        let service = group("Attached Ollama");
        service.add(&row("Endpoint", ENDPOINT));
        service.add(&row("Version", &discovery.version));
        service.set_description(Some(
            "Externally owned service. Discovery only reads version and model lists; start and stop Ollama separately.",
        ));
        self.readings.append(&service);

        let installed = group("Installed models");
        installed.set_description(Some(
            "Cloud metadata is flagged when reported. Other models have unverified routing; discovery does not run any model.",
        ));
        match &discovery.installed {
            Ok(models) if models.is_empty() => {
                installed.add(&row(
                    "No installed models",
                    "Ollama returned an empty model list.",
                ));
            }
            Ok(models) => {
                for model in visible_models(models) {
                    installed.add(&row(&model.name, &installed_text(model)));
                }
                if let Some(notice) = truncation_notice(models.len()) {
                    installed.add(&row("Model list truncated", &notice));
                }
            }
            Err(message) => installed.add(&row("Installed model list unavailable", message)),
        }
        self.readings.append(&installed);

        let running = group("Loaded models");
        running.set_description(Some(
            "Memory is Ollama's reported allocation, not measured process RAM or evidence of PCIe spill. Unknown or inconsistent memory splits are not estimated.",
        ));
        match &discovery.running {
            Ok(models) if models.is_empty() => {
                running.add(&row(
                    "No loaded models",
                    "Ollama returned an empty loaded-model list.",
                ));
            }
            Ok(models) => {
                for model in visible_models(models) {
                    running.add(&row(&model.name, &running_text(model)));
                }
                if let Some(notice) = truncation_notice(models.len()) {
                    running.add(&row("Model list truncated", &notice));
                }
            }
            Err(message) => running.add(&row("Loaded model list unavailable", message)),
        }
        self.readings.append(&running);
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
    status.set_text("Checking attached Ollama…");
    if let Err(error) = commands.try_send(CoreCommand::DiscoverOllama) {
        state.borrow_mut().finish(false);
        button.set_sensitive(true);
        spinner.stop();
        let message = match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => "Worker busy. Refresh to retry.",
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                "Worker unavailable. Close and reopen the application to retry."
            }
        };
        status.set_text(&format!(
            "{message}{}",
            if state.borrow().stale {
                "\nShowing previous snapshot (stale)."
            } else {
                ""
            }
        ));
    }
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(display_text(title))
        .subtitle(display_text(subtitle))
        .use_markup(false)
        .title_lines(0)
        .subtitle_lines(0)
        .build()
}

fn visible_models<T>(models: &[T]) -> &[T] {
    &models[..models.len().min(MAX_VISIBLE_MODELS)]
}

fn truncation_notice(total: usize) -> Option<String> {
    (total > MAX_VISIBLE_MODELS).then(|| {
        format!(
            "Showing the first {MAX_VISIBLE_MODELS} of {total} models. Remaining models are omitted from this view."
        )
    })
}

fn display_text(text: &str) -> String {
    let mut characters = text.chars();
    let prefix: String = characters.by_ref().take(MAX_TEXT_CHARACTERS).collect();
    if characters.next().is_some() {
        format!("{prefix}… (truncated)")
    } else {
        prefix
    }
}

fn bytes_text(bytes: Option<u64>) -> String {
    bytes
        .map(|bytes| format!("{bytes} bytes ({:.2} GiB)", bytes as f64 / 1073741824.0))
        .unwrap_or_else(|| "Unknown".into())
}

fn installed_text(model: &InstalledModel) -> String {
    let routing = if model.cloud_routed() {
        "Cloud Routed"
    } else {
        "Routing unverified"
    };
    let mut lines = vec![routing.into(), format!("Size: {}", bytes_text(model.size))];
    if let Some(details) = &model.details {
        let metadata: Vec<_> = [
            details.family.as_deref(),
            details.parameter_size.as_deref(),
            details.quantization_level.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .collect();
        if !metadata.is_empty() {
            lines.push(metadata.join(" · "));
        }
    }
    if let Some(digest) = &model.digest {
        lines.push(format!("Digest: {digest}"));
    }
    lines.join("\n")
}

fn running_text(model: &RunningModel) -> String {
    let split = match model.cpu_bytes() {
        Some(cpu) => format!(
            "CPU: {}\nGPU: {}",
            bytes_text(Some(cpu)),
            bytes_text(model.size_vram)
        ),
        None => "CPU / GPU split: Unknown".into(),
    };
    format!(
        "Total allocation: {}\n{split}\nContext: {}",
        bytes_text(model.size),
        model
            .context_length
            .map(|size| format!("{size} tokens"))
            .unwrap_or_else(|| "Unknown".into())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_model_rows_and_reports_omitted_models() {
        let models: Vec<_> = (0..150_000).collect();
        assert_eq!(visible_models(&models), &models[..100]);
        assert!(visible_models::<u8>(&[]).is_empty());
        assert!(truncation_notice(100).is_none());
        assert!(
            truncation_notice(models.len())
                .unwrap()
                .contains("100 of 150000")
        );
    }

    #[test]
    fn limits_text_without_splitting_unicode_or_interpreting_markup() {
        let markup = "<b>模型</b> & literal";
        assert_eq!(display_text(markup), markup);
        let exact = "界".repeat(MAX_TEXT_CHARACTERS);
        assert_eq!(display_text(&exact), exact);
        assert_eq!(
            display_text(&format!("{exact}🦀")),
            format!("{exact}… (truncated)")
        );
    }

    #[test]
    fn retains_stale_snapshot_and_allows_retry_after_failure() {
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
    fn allocation_display_preserves_exact_bytes_and_unknown_split() {
        assert_eq!(bytes_text(None), "Unknown");
        assert_eq!(bytes_text(Some(0)), "0 bytes (0.00 GiB)");
        let mut model = RunningModel {
            name: "example".into(),
            size: Some(1073741825),
            size_vram: Some(1073741824),
            context_length: None,
        };
        let text = running_text(&model);
        assert!(text.contains("1073741825 bytes (1.00 GiB)"));
        assert!(text.contains("CPU: 1 bytes (0.00 GiB)"));
        assert!(text.contains("GPU: 1073741824 bytes (1.00 GiB)"));
        assert!(text.contains("Context: Unknown"));
        model.size_vram = Some(u64::MAX);
        assert!(running_text(&model).contains("CPU / GPU split: Unknown"));
        model.size_vram = None;
        assert!(running_text(&model).contains("CPU / GPU split: Unknown"));
    }

    #[test]
    fn discovery_never_claims_unverified_models_are_local() {
        let mut model = InstalledModel {
            name: "example".into(),
            digest: None,
            size: None,
            details: None,
            remote_model: None,
            remote_host: None,
        };
        assert!(installed_text(&model).starts_with("Routing unverified"));
        model.remote_model = Some("example-cloud".into());
        assert!(installed_text(&model).starts_with("Cloud Routed"));
    }
}
