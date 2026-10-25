// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bridge::{CoreCommand, UiEvent};
use adw::prelude::*;
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};
use tokio::sync::{mpsc, watch};
use workstation_bench::runner::{BenchmarkOutcome, QuickBenchmarkReport};
use workstation_runtimes::ollama::{GenerationProgress, InstalledModel};

const MAX_VISIBLE_MODELS: usize = 100;
const MAX_TEXT_CHARACTERS: usize = 1024;

struct ActiveRun {
    id: u64,
    cancel: watch::Sender<bool>,
    cancel_requested: bool,
}

#[derive(Default)]
struct RunState {
    generation: u64,
    inventory_ready: bool,
    models: Vec<InstalledModel>,
    selected: Option<InstalledModel>,
    in_flight: Option<ActiveRun>,
}

pub struct Benchmarks {
    pub widget: gtk::ScrolledWindow,
    model: adw::ComboRow,
    run: gtk::Button,
    cancel: gtk::Button,
    status: gtk::Label,
    results: gtk::Box,
    state: Rc<RefCell<RunState>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl Benchmarks {
    pub fn new(commands: mpsc::Sender<CoreCommand>) -> Self {
        let choices = choices_model(&[]);
        let model = model_row(&choices);
        let run = gtk::Button::with_mnemonic("_Check and Run");
        run.set_sensitive(false);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.set_sensitive(false);
        let status = gtk::Label::builder()
            .label("Refresh Runtimes to load installed models.")
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build();
        let results = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let workload = adw::PreferencesGroup::builder()
            .title("Quick workload")
            .description(
                "quick-text-v1 · 128 max output tokens · 2,048 context · seed 42 · temperature 0",
            )
            .build();
        workload.add(&row(
            "Workload",
            "One fixed text-generation request. No editable prompt or chat history.",
        ));
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.append(&run);
        controls.append(&cancel);
        let heading = gtk::Label::builder()
            .label("Benchmark Lab")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build();
        let note = gtk::Label::builder()
            .label("Choose an installed model, then Check and Run. Known cloud-routed models are excluded. The runtime checks local-only support before inference. A request may load the model and use memory; this app never changes Ollama's service configuration.")
            .xalign(0.0)
            .wrap(true)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .build();
        content.append(&heading);
        content.append(&note);
        let selection = adw::PreferencesGroup::new();
        selection.add(&model);
        content.append(&selection);
        content.append(&workload);
        content.append(&controls);
        content.append(&status);
        content.append(&results);
        let clamp = adw::Clamp::builder()
            .maximum_size(900)
            .child(&content)
            .build();
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .vexpand(true)
            .build();
        let panel = Self {
            widget,
            model,
            run,
            cancel,
            status,
            results,
            state: Rc::new(RefCell::new(RunState::default())),
            commands,
        };
        panel.connect_controls();
        panel
    }

    fn connect_controls(&self) {
        let state = Rc::downgrade(&self.state);
        let run = self.run.downgrade();
        let status = self.status.clone();
        let results = self.results.clone();
        let cancel = self.cancel.clone();
        self.model.connect_selected_notify(move |row| {
            let Some(state) = state.upgrade() else { return };
            let Some(run) = run.upgrade() else { return };
            let selected = {
                let state = state.borrow();
                row.selected()
                    .checked_sub(1)
                    .and_then(|index| state.models.get(index as usize))
                    .cloned()
            };
            select_model(&state, selected, &status, &results, &run, &cancel);
        });

        let state = Rc::downgrade(&self.state);
        let commands = self.commands.clone();
        let status = self.status.clone();
        let results = self.results.clone();
        let cancel = self.cancel.clone();
        self.run.connect_clicked(move |button| {
            if let Some(state) = state.upgrade() {
                start_run(&state, &commands, &status, &results, button, &cancel);
            }
        });

        let state = Rc::downgrade(&self.state);
        let status = self.status.clone();
        self.cancel.connect_clicked(move |button| {
            if let Some(state) = state.upgrade() {
                cancel_run(&state, &status, button);
            }
        });
    }

    pub fn receive(&self, event: &UiEvent) {
        match event {
            UiEvent::OllamaDiscovered(discovery) => match &discovery.installed {
                Ok(models) => {
                    let (selectable, cloud_count, duplicate_count, truncated_count) =
                        selectable_models(models);
                    self.replace_inventory(
                        selectable,
                        cloud_count,
                        duplicate_count,
                        truncated_count,
                    );
                }
                Err(message) => {
                    self.inventory_failed(&format!("Installed model list unavailable: {message}"))
                }
            },
            UiEvent::OllamaFailed(message) => {
                self.inventory_failed(&format!("Ollama discovery failed: {message}"));
            }
            UiEvent::BenchmarkFinished { id, outcome } => {
                finish_run(
                    &self.state,
                    *id,
                    outcome,
                    &self.status,
                    &self.results,
                    &self.run,
                    &self.cancel,
                );
            }
            _ => {}
        }
    }

    fn inventory_failed(&self, message: &str) {
        self.replace_inventory(Vec::new(), 0, 0, 0);
        self.status.set_text(&display_text(message));
        self.state.borrow_mut().inventory_ready = false;
    }

    fn replace_inventory(
        &self,
        models: Vec<InstalledModel>,
        cloud_count: usize,
        duplicate_count: usize,
        truncated_count: usize,
    ) {
        let choices = {
            let mut state = self.state.borrow_mut();
            invalidate_run(&mut state);
            state.inventory_ready = true;
            state.models = models;
            state.selected = None;
            choices_model(&state.models)
        };
        clear_results(&self.results);
        self.model.set_model(Some(&choices));
        self.model.set_selected(0);
        let count = self.state.borrow().models.len();
        let mut message = if count == 0 {
            "No selectable installed models. Refresh Runtimes after installing a model.".into()
        } else {
            format!("{count} installed model(s) available. Choose one to continue.")
        };
        if cloud_count > 0 {
            message.push_str(&format!(
                " {cloud_count} known cloud-routed model(s) excluded."
            ));
        }
        if duplicate_count > 0 {
            message.push_str(&format!(
                " {duplicate_count} duplicate-name model(s) excluded."
            ));
        }
        if truncated_count > 0 {
            message.push_str(&format!(
                " Showing the first {MAX_VISIBLE_MODELS}; {truncated_count} more omitted."
            ));
        }
        self.status.set_text(&message);
        sync_buttons(&self.state.borrow(), &self.run, &self.cancel);
    }
}

fn model_row(model: &gtk::StringList) -> adw::ComboRow {
    let expression = gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    );
    adw::ComboRow::builder()
        .title("Installed model")
        .model(model)
        .expression(&expression)
        .selected(0)
        .build()
}

fn choices_model(models: &[InstalledModel]) -> gtk::StringList {
    let mut labels = vec!["Choose an installed model…".to_string()];
    labels.extend(models.iter().map(|model| display_text(&model.name)));
    let labels: Vec<_> = labels.iter().map(String::as_str).collect();
    gtk::StringList::new(&labels)
}

fn selectable_models(models: &[InstalledModel]) -> (Vec<InstalledModel>, usize, usize, usize) {
    let mut names = HashMap::new();
    for model in models {
        *names.entry(model.name.as_str()).or_insert(0usize) += 1;
    }
    let cloud = models.iter().filter(|model| model.cloud_routed()).count();
    let duplicate_names = models
        .iter()
        .filter(|model| names[model.name.as_str()] > 1)
        .count();
    let is_selectable =
        |model: &&InstalledModel| !model.cloud_routed() && names[model.name.as_str()] == 1;
    let selectable_count = models.iter().filter(is_selectable).count();
    let selectable = models
        .iter()
        .filter(is_selectable)
        .take(MAX_VISIBLE_MODELS)
        .cloned()
        .collect();
    let truncated = selectable_count.saturating_sub(MAX_VISIBLE_MODELS);
    (selectable, cloud, duplicate_names, truncated)
}

fn select_model(
    state: &RefCell<RunState>,
    selected: Option<InstalledModel>,
    status: &gtk::Label,
    results: &gtk::Box,
    run: &gtk::Button,
    cancel: &gtk::Button,
) {
    let mut state = state.borrow_mut();
    if same_identity(state.selected.as_ref(), selected.as_ref()) {
        return;
    }
    invalidate_run(&mut state);
    state.selected = selected;
    clear_results(results);
    let message = if state.in_flight.is_some() {
        "Selection changed. Waiting for the cancelled run to finish.".into()
    } else {
        match state.selected.as_ref() {
            None => "Choose an installed model.".to_string(),
            Some(model) if !has_digest(model) => {
                "This model has no digest in the current inventory and cannot be benchmarked."
                    .into()
            }
            Some(_) => "Ready to check local-only support and run one request.".into(),
        }
    };
    status.set_text(&message);
    sync_buttons(&state, run, cancel);
}

fn same_identity(left: Option<&InstalledModel>, right: Option<&InstalledModel>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.name == right.name && left.digest == right.digest,
        (None, None) => true,
        _ => false,
    }
}

fn has_digest(model: &InstalledModel) -> bool {
    model
        .digest
        .as_deref()
        .is_some_and(|digest| !digest.trim().is_empty())
}

fn invalidate_run(state: &mut RunState) {
    state.generation = state.generation.wrapping_add(1);
    if let Some(active) = &mut state.in_flight {
        let _ = active.cancel.send(true);
        active.cancel_requested = true;
    }
}

fn start_run(
    state: &RefCell<RunState>,
    commands: &mpsc::Sender<CoreCommand>,
    status: &gtk::Label,
    results: &gtk::Box,
    run: &gtk::Button,
    cancel: &gtk::Button,
) {
    let (id, model, cancel_receiver) = {
        let mut state = state.borrow_mut();
        if state.in_flight.is_some() || !state.inventory_ready {
            return;
        }
        let Some(model) = state.selected.clone().filter(has_digest) else {
            status.set_text("Choose an installed model with a digest first.");
            return;
        };
        state.generation = state.generation.wrapping_add(1);
        let id = state.generation;
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        state.in_flight = Some(ActiveRun {
            id,
            cancel: cancel_sender,
            cancel_requested: false,
        });
        (id, model, cancel_receiver)
    };
    clear_results(results);
    status.set_text("Checking local-only support and running one request…");
    sync_buttons(&state.borrow(), run, cancel);
    if let Err(error) = commands.try_send(CoreCommand::RunQuickBenchmark {
        id,
        model,
        cancel: cancel_receiver,
    }) {
        let message = match error {
            mpsc::error::TrySendError::Full(_) => "Worker busy. Select Check and Run to retry.",
            mpsc::error::TrySendError::Closed(_) => {
                "Worker unavailable. Close and reopen the application to retry."
            }
        };
        let mut state = state.borrow_mut();
        if state
            .in_flight
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            state.in_flight = None;
        }
        status.set_text(message);
        sync_buttons(&state, run, cancel);
    }
}

fn cancel_run(state: &RefCell<RunState>, status: &gtk::Label, cancel: &gtk::Button) {
    let mut state = state.borrow_mut();
    if let Some(active) = &mut state.in_flight
        && !active.cancel_requested
    {
        let _ = active.cancel.send(true);
        active.cancel_requested = true;
        status.set_text("Cancelling; waiting for the worker to finish.");
        cancel.set_sensitive(false);
    }
}

fn finish_run(
    state: &RefCell<RunState>,
    id: u64,
    outcome: &BenchmarkOutcome,
    status: &gtk::Label,
    results: &gtk::Box,
    run: &gtk::Button,
    cancel: &gtk::Button,
) {
    let (current, inventory_ready, has_selection) = {
        let mut state = state.borrow_mut();
        let current = match take_matching_run(&mut state, id) {
            Some(current) => current,
            None => return,
        };
        (current, state.inventory_ready, state.selected.is_some())
    };
    if current {
        render_outcome(results, status, outcome);
    } else if inventory_ready && has_selection {
        status.set_text("Previous run finished. Select Check and Run to start again.");
    } else if inventory_ready {
        status.set_text("Choose an installed model to start a benchmark.");
    }
    sync_buttons(&state.borrow(), run, cancel);
}

fn take_matching_run(state: &mut RunState, id: u64) -> Option<bool> {
    if !state
        .in_flight
        .as_ref()
        .is_some_and(|active| active.id == id)
    {
        return None;
    }
    state.in_flight = None;
    Some(state.generation == id && state.selected.is_some())
}

fn render_outcome(results: &gtk::Box, status: &gtk::Label, outcome: &BenchmarkOutcome) {
    clear_results(results);
    match outcome {
        BenchmarkOutcome::Completed(report) => {
            status
                .set_text("Quick benchmark completed. Results are one client/server observation.");
            results.append(&completed_group(report));
        }
        BenchmarkOutcome::Cancelled(progress) => {
            status.set_text("Benchmark cancelled. Partial progress is not a completed result.");
            results.append(&partial_group("Cancelled progress", progress));
        }
        BenchmarkOutcome::TimedOut(progress) => {
            status.set_text("Benchmark timed out. Partial progress is not a completed result.");
            results.append(&partial_group("Timed-out progress", progress));
        }
        BenchmarkOutcome::Failed { message, partial } => {
            status.set_text("Benchmark failed. Partial progress is not a completed result.");
            let group = partial_group("Partial progress", partial);
            group.add(&row("Failure", message));
            results.append(&group);
        }
    }
}

fn completed_group(report: &QuickBenchmarkReport) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Completed quick benchmark")
        .description("One request; no warm-up, resource sampling, or repeated-run verification.")
        .build();
    let generation = &report.generation;
    group.add(&row("Workload", report.workload_id));
    group.add(&row("Model", &generation.model_name));
    group.add(&row("Model digest", &generation.model_digest));
    group.add(&row("Runtime version", &generation.runtime_version));
    group.add(&row(
        "Client first response text (includes model load)",
        &duration_text(generation.first_output_after),
    ));
    group.add(&row(
        "Server-reported generation speed",
        &report
            .generation_tokens_per_second
            .filter(|speed| speed.is_finite() && *speed >= 0.0)
            .map(|speed| format!("{speed:.2} tokens/s"))
            .unwrap_or_else(|| "Unknown".into()),
    ));
    group.add(&row(
        "Server evaluation token count",
        &generation
            .eval_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "Unknown".into()),
    ));
    group.add(&row(
        "Server evaluation duration",
        &nanoseconds_text(generation.eval_duration_ns),
    ));
    group.add(&row(
        "Response chunks",
        &generation.response_chunks.to_string(),
    ));
    group.add(&row(
        "Generation request elapsed (client)",
        &duration_text(Some(generation.elapsed)),
    ));
    group.add(&row(
        "Model load duration",
        &nanoseconds_text(generation.load_duration_ns),
    ));
    group.add(&row("Completion reason", &generation.done_reason));
    group
}

fn partial_group(title: &str, progress: &GenerationProgress) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(title)
        .description("These partial readings are not a completed benchmark result.")
        .build();
    group.add(&row(
        "Partial client first response text",
        &duration_text(progress.first_output_after),
    ));
    group.add(&row(
        "Partial response chunks",
        &progress.response_chunks.to_string(),
    ));
    group
}

fn duration_text(duration: Option<Duration>) -> String {
    duration
        .map(|duration| format!("{:.2} ms", duration.as_secs_f64() * 1000.0))
        .unwrap_or_else(|| "Unknown".into())
}

fn nanoseconds_text(nanoseconds: Option<u64>) -> String {
    nanoseconds
        .map(|value| format!("{value} ns"))
        .unwrap_or_else(|| "Unknown".into())
}

fn sync_buttons(state: &RunState, run: &gtk::Button, cancel: &gtk::Button) {
    run.set_sensitive(
        state.inventory_ready
            && state.in_flight.is_none()
            && state.selected.as_ref().is_some_and(has_digest),
    );
    cancel.set_sensitive(
        state
            .in_flight
            .as_ref()
            .is_some_and(|active| !active.cancel_requested),
    );
}

fn clear_results(results: &gtk::Box) {
    while let Some(child) = results.first_child() {
        results.remove(&child);
    }
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

fn display_text(text: &str) -> String {
    let mut characters = text.chars();
    let prefix: String = characters.by_ref().take(MAX_TEXT_CHARACTERS).collect();
    if characters.next().is_some() {
        format!("{prefix}… (truncated)")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workstation_runtimes::ollama::ModelDetails;

    fn model(name: &str, digest: Option<&str>, cloud: bool) -> InstalledModel {
        InstalledModel {
            name: name.into(),
            digest: digest.map(str::to_string),
            size: Some(1),
            details: Some(ModelDetails {
                family: None,
                parameter_size: None,
                quantization_level: None,
            }),
            remote_model: cloud.then(|| "remote".into()),
            remote_host: None,
        }
    }

    #[test]
    fn inventory_excludes_cloud_duplicates_and_caps_choices() {
        let models = vec![
            model("local", Some("sha256:a"), false),
            model("cloud", Some("sha256:b"), true),
            model("duplicate", Some("sha256:c"), false),
            model("duplicate", Some("sha256:d"), false),
            model("missing-digest", None, false),
        ];
        let (selectable, cloud, duplicates, _) = selectable_models(&models);
        assert_eq!(cloud, 1);
        assert_eq!(duplicates, 2);
        assert_eq!(selectable.len(), 2);
        assert_eq!(selectable[0].name, "local");
        assert!(selectable[1].digest.is_none());
        let many: Vec<_> = (0..150)
            .map(|index| model(&format!("model-{index}"), Some("sha256:x"), false))
            .collect();
        let (limited, _, _, truncated) = selectable_models(&many);
        assert_eq!(limited.len(), MAX_VISIBLE_MODELS);
        assert_eq!(truncated, 50);
    }

    #[test]
    fn invalidation_cancels_the_active_run_but_keeps_it_busy_until_completion() {
        let mut state = RunState {
            generation: 7,
            ..RunState::default()
        };
        let (sender, receiver) = watch::channel(false);
        state.in_flight = Some(ActiveRun {
            id: 7,
            cancel: sender,
            cancel_requested: false,
        });
        invalidate_run(&mut state);
        assert!(*receiver.borrow());
        assert_eq!(state.in_flight.as_ref().unwrap().id, 7);
        assert_ne!(state.generation, 7);
        assert_eq!(take_matching_run(&mut state, 6), None);
        assert_eq!(state.in_flight.as_ref().unwrap().id, 7);
        assert_eq!(take_matching_run(&mut state, 7), Some(false));
        assert!(state.in_flight.is_none());
    }
}

#[cfg(test)]
#[path = "bench_lab_smoke.rs"]
mod smoke;
