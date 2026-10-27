// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bridge::{CoreCommand, UiEvent};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc};
use workstation_runtimes::ollama::{
    Discovery, ENDPOINT, InstalledModel, ModelInspection, RunningModel,
};

const MAX_VISIBLE_MODELS: usize = 100;
const MAX_TEXT_CHARACTERS: usize = 1024;

#[derive(Default)]
struct LoadState {
    loading: bool,
    has_snapshot: bool,
    stale: bool,
}

#[derive(Default)]
struct InspectionState {
    id: u64,
    in_flight: Option<u64>,
    tags_stale: bool,
    installed: Vec<InstalledModel>,
    selected: Option<(InstalledModel, Option<String>)>,
    buttons: Vec<gtk::Button>,
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
    inspection_details: gtk::Box,
    state: Rc<RefCell<LoadState>>,
    inspection: Rc<RefCell<InspectionState>>,
    commands: tokio::sync::mpsc::Sender<CoreCommand>,
    managed: super::managed::ManagedCard,
    layout: gtk::Box,
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
        let managed = super::managed::ManagedCard::new(commands.clone());
        layout.append(&managed.widget);
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
            inspection_details: gtk::Box::new(gtk::Orientation::Vertical, 12),
            state: Rc::new(RefCell::new(LoadState::default())),
            inspection: Rc::new(RefCell::new(InspectionState::default())),
            commands,
            managed,
            layout,
        };
        let state = Rc::clone(&runtimes.state);
        let spinner = runtimes.spinner.clone();
        let status = runtimes.status.clone();
        let commands = runtimes.commands.clone();
        let inspection = Rc::clone(&runtimes.inspection);
        let details = runtimes.inspection_details.clone();
        runtimes.refresh.connect_clicked(move |button| {
            invalidate_inspection(&inspection);
            render_pending_inspection(&details, &inspection);
            request(&state, button, &spinner, &status, &commands);
        });
        runtimes
    }

    /// Add a section below the managed runtime card.
    pub fn append(&self, section: &impl IsA<gtk::Widget>) {
        self.layout.append(section);
    }

    /// Reads the saved managed-runtime state (local files only) and starts supervision.
    pub fn load_managed(&self) {
        self.managed.load();
    }

    pub fn receive(&self, event: &UiEvent) {
        self.managed.receive(event);
        match event {
            UiEvent::OllamaDiscovered(discovery) => {
                self.state.borrow_mut().finish(true);
                invalidate_inspection(&self.inspection);
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
            UiEvent::OllamaInspected { id, result } => {
                let mut inspection = self.inspection.borrow_mut();
                if !complete_inspection(&mut inspection, *id) {
                    return;
                }
                drop(inspection);
                self.render_inspection_result(result);
                return;
            }
            _ => return,
        }
        self.spinner.stop();
        self.refresh.set_sensitive(true);
    }

    fn render(&self, discovery: &Discovery) {
        {
            let mut inspection = self.inspection.borrow_mut();
            update_installed_snapshot(
                &mut inspection,
                discovery.installed.as_ref().cloned().unwrap_or_default(),
            );
        }
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
                    let model_row = row(&model.name, &installed_text(model));
                    let inspect = gtk::Button::with_label("Inspect");
                    inspect.set_valign(gtk::Align::Center);
                    let state = Rc::clone(&self.inspection);
                    let commands = self.commands.clone();
                    let model = model.clone();
                    let inspection = state.borrow();
                    inspect.set_sensitive(inspection.in_flight.is_none() && !inspection.tags_stale);
                    drop(inspection);
                    state.borrow_mut().buttons.push(inspect.clone());
                    let state = Rc::downgrade(&state);
                    let details = self.inspection_details.clone();
                    inspect.connect_clicked(move |button| {
                        if let Some(state) = state.upgrade() {
                            inspect_model(&state, button, &commands, model.clone());
                            render_pending_inspection(&details, &state);
                        }
                    });
                    model_row.add_suffix(&inspect);
                    installed.add(&model_row);
                }
                if let Some(notice) = truncation_notice(models.len()) {
                    installed.add(&row("Model list truncated", &notice));
                }
            }
            Err(message) => installed.add(&row("Installed model list unavailable", message)),
        }
        self.readings.append(&installed);
        self.readings.append(&self.inspection_details);

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

    fn render_inspection_result(&self, result: &Result<Box<ModelInspection>, String>) {
        while let Some(child) = self.inspection_details.first_child() {
            self.inspection_details.remove(&child);
        }
        let state = self.inspection.borrow();
        if let Some((model, _)) = &state.selected {
            let details = selected_details_group(model);
            match result {
                Err(message) => details.add(&row("Inspection unavailable", message)),
                Ok(inspection) => {
                    render_model_inspection(&details, model, &state.installed, inspection)
                }
            }
            self.inspection_details.append(&details);
        }
    }
}

fn render_pending_inspection(details: &gtk::Box, state: &RefCell<InspectionState>) {
    while let Some(child) = details.first_child() {
        details.remove(&child);
    }
    if let Some((model, result)) = &state.borrow().selected {
        let group = selected_details_group(model);
        match result {
            None => group.add(&row(
                "Inspection",
                "Loading model metadata and loaded-model readings…",
            )),
            Some(message) => group.add(&row("Inspection unavailable", message)),
        }
        details.append(&group);
    }
}

fn selected_details_group(model: &InstalledModel) -> adw::PreferencesGroup {
    let group = group("Selected model inspection");
    group.set_description(Some(
        "Actual K/V cache types, micro-batch size, concurrency, and compatibility are unknown. These declarations do not identify the file selected in Overview.",
    ));
    group.add(&row("Model", &model.name));
    group
}

fn invalidate_inspection(state: &RefCell<InspectionState>) {
    let mut state = state.borrow_mut();
    state.id = state.id.wrapping_add(1);
    state.selected = None;
    state.tags_stale = true;
    for button in &state.buttons {
        button.set_sensitive(false);
    }
}

fn update_installed_snapshot(state: &mut InspectionState, installed: Vec<InstalledModel>) {
    state.buttons.clear();
    state.installed = installed;
    state.tags_stale = false;
}

fn begin_inspection(state: &mut InspectionState, model: InstalledModel) -> Option<u64> {
    if state.in_flight.is_some() || state.tags_stale {
        return None;
    }
    state.id = state.id.wrapping_add(1);
    state.in_flight = Some(state.id);
    state.selected = Some((model, None));
    for button in &state.buttons {
        button.set_sensitive(false);
    }
    Some(state.id)
}

fn complete_inspection(state: &mut InspectionState, id: u64) -> bool {
    if state.in_flight != Some(id) {
        return false;
    }
    state.in_flight = None;
    if !state.tags_stale {
        for button in &state.buttons {
            button.set_sensitive(true);
        }
    }
    state.id == id && !state.tags_stale
}

fn reject_inspection(state: &mut InspectionState, id: u64, message: &str) {
    if state.in_flight != Some(id) {
        return;
    }
    state.in_flight = None;
    if !state.tags_stale {
        for button in &state.buttons {
            button.set_sensitive(true);
        }
    }
    if let Some((_, selected)) = &mut state.selected {
        *selected = Some(message.into());
    }
}

fn queue_inspection(
    state: &RefCell<InspectionState>,
    commands: &tokio::sync::mpsc::Sender<CoreCommand>,
    model: InstalledModel,
) -> Result<Option<u64>, &'static str> {
    let model_name = model.name.clone();
    let id = match begin_inspection(&mut state.borrow_mut(), model) {
        Some(id) => id,
        None => return Ok(None),
    };
    match commands.try_send(CoreCommand::InspectOllama {
        id,
        model: model_name,
    }) {
        Ok(()) => Ok(Some(id)),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            const MESSAGE: &str = "Worker busy. Select Inspect to retry.";
            reject_inspection(&mut state.borrow_mut(), id, MESSAGE);
            Err(MESSAGE)
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            const MESSAGE: &str = "Worker unavailable. Close and reopen the application to retry.";
            reject_inspection(&mut state.borrow_mut(), id, MESSAGE);
            Err(MESSAGE)
        }
    }
}

fn inspect_model(
    state: &RefCell<InspectionState>,
    button: &gtk::Button,
    commands: &tokio::sync::mpsc::Sender<CoreCommand>,
    model: InstalledModel,
) {
    match queue_inspection(state, commands, model) {
        Ok(Some(_)) => button.set_sensitive(false),
        Ok(None) => {}
        Err(message) => button.set_tooltip_text(Some(message)),
    }
}

fn render_model_inspection(
    parent: &adw::PreferencesGroup,
    selected: &InstalledModel,
    installed: &[InstalledModel],
    inspection: &ModelInspection,
) {
    let metadata_group = group("Model metadata");
    metadata_group.set_description(Some(
        "Declared parameters describe the model metadata and do not confirm Ollama's loaded settings.",
    ));
    match &inspection.metadata {
        Err(message) => metadata_group.add(&row("Metadata unavailable", message)),
        Ok(metadata) => {
            metadata_group.add(&row(
                "Routing",
                if metadata.cloud_routed() || selected.cloud_routed() {
                    "Cloud routed"
                } else {
                    "Routing unverified"
                },
            ));
            let details = metadata.details.as_ref();
            metadata_group.add(&row(
                "Architecture",
                metadata.architecture().unwrap_or("Unknown"),
            ));
            metadata_group.add(&row(
                "Family",
                details
                    .and_then(|details| details.family.as_deref())
                    .unwrap_or("Unknown"),
            ));
            let context = metadata
                .declared_context_length()
                .filter(|length| *length > 0)
                .map(|length| format!("{length} tokens"))
                .unwrap_or_else(|| "Unknown".into());
            metadata_group.add(&row("Declared training context", &context));
            metadata_group.add(&row(
                "Parameter size",
                details
                    .and_then(|d| d.parameter_size.as_deref())
                    .unwrap_or("Unknown"),
            ));
            metadata_group.add(&row(
                "Quantization",
                details
                    .and_then(|d| d.quantization_level.as_deref())
                    .unwrap_or("Unknown"),
            ));
            if let Some(capabilities) = &metadata.capabilities {
                metadata_group.add(&row(
                    "Capabilities",
                    &capabilities
                        .iter()
                        .map(|v| display_text(v))
                        .collect::<Vec<_>>()
                        .join(" · "),
                ));
            } else {
                metadata_group.add(&row("Capabilities", "Unknown"));
            }
            metadata_group.add(&row(
                "Declared parameters",
                metadata
                    .parameters
                    .as_deref()
                    .map(display_text)
                    .as_deref()
                    .unwrap_or("Unknown"),
            ));
        }
    }
    parent.add(&metadata_group);

    let loaded_group = group("Loaded allocation from this inspection");
    match &inspection.running {
        Err(message) => loaded_group.add(&row("Loaded readings unavailable", message)),
        Ok(running) => {
            if let Some(model) = matching_running(selected, installed, running) {
                loaded_group.add(&row("Loaded model", &running_text(model)));
            } else {
                loaded_group.add(&row(
                    "Loaded model",
                    "Unverified: exact name and digest could not be uniquely matched.",
                ));
            }
        }
    }
    parent.add(&loaded_group);
}

fn matching_running<'a>(
    selected: &InstalledModel,
    installed: &[InstalledModel],
    running: &'a [RunningModel],
) -> Option<&'a RunningModel> {
    let digest = selected.digest.as_ref()?;
    if selected.name.trim().is_empty() || digest.trim().is_empty() {
        return None;
    }
    let mut installed_matches = installed.iter().filter(|model| model.name == selected.name);
    let installed_model = installed_matches.next()?;
    if installed_matches.next().is_some() || installed_model.digest.as_ref() != Some(digest) {
        return None;
    }
    let mut running_matches = running.iter().filter(|model| model.name == selected.name);
    let running_model = running_matches.next()?;
    if running_matches.next().is_some() || running_model.digest.as_ref() != Some(digest) {
        return None;
    }
    Some(running_model)
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
            .filter(|length| *length > 0)
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
            digest: None,
            size: Some(1073741825),
            size_vram: Some(1073741824),
            context_length: None,
        };
        let text = running_text(&model);
        assert!(text.contains("1073741825 bytes (1.00 GiB)"));
        assert!(text.contains("CPU: 1 bytes (0.00 GiB)"));
        assert!(text.contains("GPU: 1073741824 bytes (1.00 GiB)"));
        assert!(text.contains("Context: Unknown"));
        model.context_length = Some(0);
        assert!(running_text(&model).contains("Context: Unknown"));
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

    #[test]
    fn associates_loaded_allocation_only_with_unique_exact_name_and_digest() {
        let installed = InstalledModel {
            name: "example:latest".into(),
            digest: Some("sha256:abc".into()),
            size: None,
            details: None,
            remote_model: None,
            remote_host: None,
        };
        let running = RunningModel {
            name: installed.name.clone(),
            digest: installed.digest.clone(),
            size: Some(10),
            size_vram: Some(8),
            context_length: Some(4096),
        };
        assert!(
            matching_running(
                &installed,
                std::slice::from_ref(&installed),
                std::slice::from_ref(&running)
            )
            .is_some()
        );
        assert!(matching_running(&installed, &[], std::slice::from_ref(&running)).is_none());
        let mut no_digest = running.clone();
        no_digest.digest = None;
        assert!(
            matching_running(&installed, std::slice::from_ref(&installed), &[no_digest]).is_none()
        );
        let mut mismatch = running.clone();
        mismatch.digest = Some("sha256:other".into());
        assert!(
            matching_running(&installed, std::slice::from_ref(&installed), &[mismatch]).is_none()
        );
        assert!(
            matching_running(
                &installed,
                &[installed.clone(), installed.clone()],
                std::slice::from_ref(&running)
            )
            .is_none()
        );
        assert!(
            matching_running(
                &installed,
                std::slice::from_ref(&installed),
                &[running.clone(), running.clone()]
            )
            .is_none()
        );
        let same_name_other_digest = InstalledModel {
            digest: Some("sha256:other".into()),
            ..installed.clone()
        };
        assert!(
            matching_running(
                &installed,
                &[installed.clone(), same_name_other_digest],
                std::slice::from_ref(&running)
            )
            .is_none()
        );
    }

    fn installed_model(name: &str) -> InstalledModel {
        InstalledModel {
            name: name.into(),
            digest: Some(format!("digest:{name}")),
            size: None,
            details: None,
            remote_model: None,
            remote_host: None,
        }
    }

    #[test]
    fn invalidation_discards_old_result_and_allows_new_selection_after_completion() {
        let state = RefCell::new(InspectionState::default());
        let first_id = begin_inspection(&mut state.borrow_mut(), installed_model("first")).unwrap();
        assert!(begin_inspection(&mut state.borrow_mut(), installed_model("second")).is_none());
        assert!(!complete_inspection(
            &mut state.borrow_mut(),
            first_id.wrapping_add(1)
        ));
        assert_eq!(state.borrow().in_flight, Some(first_id));
        invalidate_inspection(&state);
        assert!(!complete_inspection(&mut state.borrow_mut(), first_id));
        assert!(state.borrow().selected.is_none());
        update_installed_snapshot(&mut state.borrow_mut(), vec![installed_model("second")]);
        let second_id =
            begin_inspection(&mut state.borrow_mut(), installed_model("second")).unwrap();
        assert_ne!(first_id, second_id);
        assert_eq!(state.borrow().selected.as_ref().unwrap().0.name, "second");
    }

    #[test]
    fn full_and_closed_command_channels_report_actionable_retry_states() {
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        let state = RefCell::new(InspectionState::default());
        assert_eq!(
            queue_inspection(&state, &commands, installed_model("retry")),
            Err("Worker busy. Select Inspect to retry.")
        );
        assert!(state.borrow().in_flight.is_none());
        assert_eq!(
            state.borrow().selected.as_ref().unwrap().1.as_deref(),
            Some("Worker busy. Select Inspect to retry.")
        );
        receiver.try_recv().unwrap();
        assert!(queue_inspection(&state, &commands, installed_model("retry")).is_ok());

        let (closed_commands, closed_receiver) = tokio::sync::mpsc::channel(1);
        drop(closed_receiver);
        let closed_state = RefCell::new(InspectionState::default());
        assert_eq!(
            queue_inspection(&closed_state, &closed_commands, installed_model("retry")),
            Err("Worker unavailable. Close and reopen the application to retry.")
        );
        assert!(closed_state.borrow().in_flight.is_none());
    }
}

#[cfg(test)]
#[path = "runtimes_smoke.rs"]
mod runtimes_smoke;
