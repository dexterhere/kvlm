// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bridge::{CoreCommand, UiEvent};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc};
use tokio::sync::mpsc;
use workstation_core::{
    db::{HistoryEntry, PartialMetrics, RunCompletion, RunMetrics, RunOutcome},
    hardware::HardwareProfile,
};

const MAX_HISTORY: usize = 100;
const MAX_TEXT: usize = 1024;

#[derive(Default)]
struct State {
    generation: u64,
    pending: Option<u64>,
    reload_after_pending: bool,
    loaded: bool,
}

pub struct BenchmarkHistory {
    pub widget: gtk::Box,
    reload: gtk::Button,
    status: gtk::Label,
    feedback: gtk::Label,
    entries: gtk::ListBox,
    state: Rc<RefCell<State>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl BenchmarkHistory {
    pub fn new(commands: mpsc::Sender<CoreCommand>) -> Self {
        let title = gtk::Label::builder()
            .label("Recent benchmark history")
            .wrap(true)
            .hexpand(true)
            .xalign(0.0)
            .css_classes(["title-2"])
            .build();
        let reload = gtk::Button::with_label("Reload");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        header.append(&title);
        header.append(&reload);

        let status = label("History has not been loaded.");
        let feedback = label("");
        let entries = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let note = label(
            "Showing up to the newest 100 saved runs. Running entries are marked “Unfinished (running or interrupted)”; history cannot tell which state applies.",
        );
        note.add_css_class("dim-label");

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        widget.append(&header);
        widget.append(&note);
        widget.append(&status);
        widget.append(&feedback);
        widget.append(&entries);

        let history = Self {
            widget,
            reload,
            status,
            feedback,
            entries,
            state: Rc::new(RefCell::new(State::default())),
            commands,
        };
        history.connect_reload();
        history
    }

    fn connect_reload(&self) {
        let state = Rc::downgrade(&self.state);
        let commands = self.commands.clone();
        let status = self.status.downgrade();
        self.reload.connect_clicked(move |button| {
            if let Some(state) = state.upgrade() {
                request(&state, &commands, &status, button);
            }
        });
    }

    pub fn request(&self) {
        request(
            &self.state,
            &self.commands,
            &self.status.downgrade(),
            &self.reload,
        );
    }

    pub fn receive(&self, event: &UiEvent) {
        match event {
            UiEvent::BenchmarkHistoryLoaded { id, result } => {
                let reload_again = {
                    let mut state = self.state.borrow_mut();
                    if state.pending != Some(*id) {
                        return;
                    }
                    state.pending = None;
                    let reload_again = state.reload_after_pending;
                    state.reload_after_pending = false;
                    reload_again
                };
                self.reload.set_sensitive(true);
                match result {
                    Ok(entries) => {
                        self.state.borrow_mut().loaded = true;
                        render_entries(&self.entries, entries);
                        let count = self.entries.first_child().is_some();
                        self.status.set_text(if count {
                            "History loaded. Showing at most 100 newest saved runs."
                        } else {
                            "No saved benchmark history yet."
                        });
                    }
                    Err(message) => {
                        let stale = self.state.borrow().loaded;
                        self.status.set_text(&display_text(&format!(
                            "History reload failed{}: {message}",
                            if stale { "; showing the previous snapshot" } else { "" }
                        )));
                    }
                }
                if reload_again {
                    self.request();
                }
            }
            UiEvent::BenchmarkHistorySaved { benchmark_id, result } => match result {
                Ok(_) => {
                    self.feedback
                        .set_text(&format!("Run #{benchmark_id} saved to history."));
                    let mut state = self.state.borrow_mut();
                    if state.pending.is_some() {
                        state.reload_after_pending = true;
                    } else {
                        drop(state);
                        self.request();
                    }
                }
                Err(message) => self.feedback.set_text(&display_text(&format!(
                    "Run #{benchmark_id} could not be saved; any in-memory result remains unchanged. {message}"
                ))),
            },
            _ => {}
        }
    }
}

fn request(
    state: &Rc<RefCell<State>>,
    commands: &mpsc::Sender<CoreCommand>,
    status: &gtk::glib::WeakRef<gtk::Label>,
    button: &gtk::Button,
) {
    let id = {
        let mut state = state.borrow_mut();
        if state.pending.is_some() {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        let id = state.generation;
        state.pending = Some(id);
        id
    };
    button.set_sensitive(false);
    if let Some(label) = status.upgrade() {
        label.set_text("Loading benchmark history…");
    }
    let state = Rc::downgrade(state);
    let commands = commands.clone();
    let status = status.clone();
    let button = button.downgrade();
    gtk::glib::MainContext::default().spawn_local(async move {
        if commands
            .send(CoreCommand::LoadBenchmarkHistory { id })
            .await
            .is_err()
            && let Some(state) = state.upgrade()
        {
            let mut state = state.borrow_mut();
            if state.pending == Some(id) {
                state.pending = None;
                state.reload_after_pending = false;
                if let Some(button) = button.upgrade() {
                    button.set_sensitive(true);
                }
                if let Some(status) = status.upgrade() {
                    status.set_text("History worker unavailable. Reopen the application to retry.");
                }
            }
        }
    });
}

fn render_entries(container: &gtk::ListBox, entries: &[HistoryEntry]) {
    clear_entries(container);
    for entry in entries.iter().take(MAX_HISTORY) {
        container.append(&entry_row(entry));
    }
}

fn entry_row(entry: &HistoryEntry) -> adw::ExpanderRow {
    let state = entry
        .completion
        .as_ref()
        .map(completion_title)
        .unwrap_or("Unfinished (running or interrupted)");
    let row = adw::ExpanderRow::new();
    row.set_use_markup(false);
    row.set_title(&display_text(&format!(
        "Run #{} · {}",
        entry.id, entry.snapshot.model_name
    )));
    row.set_subtitle(&display_text(&format!(
        "{} · {}",
        state,
        timestamp(entry.snapshot.started_at_unix_ms)
    )));
    row.set_expanded(false);
    let snapshot = &entry.snapshot;
    let mut details = vec![
        field("Started", &timestamp(snapshot.started_at_unix_ms)),
        field("Model", &snapshot.model_name),
        field("Model digest", optional(snapshot.model_digest.as_deref())),
        field("Model size", &bytes(snapshot.model_size_bytes)),
        field("Runtime endpoint", &snapshot.runtime_endpoint),
    ];
    add_hardware_details(
        &mut details,
        snapshot.hardware.as_ref(),
        snapshot.hardware_notice.as_deref(),
    );
    add_workload_details(&mut details, &snapshot.workload);
    if let Some(completion) = &entry.completion {
        add_completion_details(&mut details, completion);
    } else {
        details.push(field("Completion", "Unfinished (running or interrupted)"));
    }
    row.add_row(&detail("Snapshot and outcome", &details.join("\n")));
    row
}

fn add_hardware_details(
    details: &mut Vec<String>,
    hardware: Option<&HardwareProfile>,
    notice: Option<&str>,
) {
    details.push(field(
        "Hardware snapshot",
        if hardware.is_some() {
            "Available"
        } else {
            "Unknown"
        },
    ));
    if let Some(hardware) = hardware {
        let cpu = hardware.cpu.as_ref();
        details.push(field(
            "CPU",
            cpu.and_then(|cpu| cpu.model.as_deref())
                .unwrap_or("Unknown"),
        ));
        details.push(field(
            "CPU cores",
            &cpu.map(|cpu| {
                format!(
                    "{} logical · {} physical",
                    cpu.logical_cores,
                    optional_usize(cpu.physical_cores)
                )
            })
            .unwrap_or_else(|| "Unknown".into()),
        ));
        details.push(field(
            "System RAM",
            &hardware
                .system_ram
                .map(|ram| {
                    format!(
                        "{} total · {} available",
                        bytes(Some(ram.total_bytes)),
                        bytes(ram.available_bytes)
                    )
                })
                .unwrap_or_else(|| "Unknown".into()),
        ));
        if hardware.gpus.is_empty() {
            details.push(field("GPUs", "Unknown"));
        } else {
            for (index, gpu) in hardware.gpus.iter().take(8).enumerate() {
                details.push(field(
                    &format!("GPU {}", index + 1),
                    &format!(
                        "{} · {:?} · dedicated VRAM {}",
                        gpu.name,
                        gpu.vendor,
                        gpu.dedicated_vram
                            .map(|ram| bytes(Some(ram.total_bytes)))
                            .unwrap_or_else(|| "Unknown".into())
                    ),
                ));
            }
            if hardware.gpus.len() > 8 {
                details.push(field("GPUs", "Additional devices omitted"));
            }
        }
    }
    if let Some(notice) = notice {
        details.push(field("Hardware note", notice));
    }
}

fn add_workload_details(
    details: &mut Vec<String>,
    workload: &workstation_core::db::WorkloadSnapshot,
) {
    details.push(field("Workload", &workload.id));
    details.push(field("Prompt SHA-256", &workload.prompt_sha256));
    details.push(field(
        "Request limits",
        &format!(
            "{} output tokens · {} context · seed {} · temperature {} · think {}",
            workload.num_predict,
            workload.num_ctx,
            workload.seed,
            workload.temperature,
            if workload.think { "on" } else { "off" }
        ),
    ));
}

fn add_completion_details(details: &mut Vec<String>, completion: &RunCompletion) {
    details.push(field(
        "Finished",
        &timestamp(completion.finished_at_unix_ms),
    ));
    details.push(field(
        "Runtime version",
        optional(completion.runtime_version.as_deref()),
    ));
    details.push(field(
        "Observed model digest",
        optional(completion.model_digest.as_deref()),
    ));
    match &completion.outcome {
        RunOutcome::Completed(metrics) => add_metrics_details(details, metrics),
        RunOutcome::Cancelled(partial) => add_partial_details(details, "Cancelled", partial),
        RunOutcome::TimedOut(partial) => add_partial_details(details, "Timed out", partial),
        RunOutcome::Failed { message, partial } => {
            add_partial_details(details, "Failed", partial);
            details.push(field("Failure", message));
        }
    }
}

fn add_metrics_details(details: &mut Vec<String>, metrics: &RunMetrics) {
    details.push(field("Outcome", "Completed"));
    details.push(field(
        "First response text",
        &nanoseconds(metrics.first_response_ns),
    ));
    details.push(field("Elapsed", &nanoseconds(Some(metrics.elapsed_ns))));
    details.push(field(
        "Response chunks",
        &metrics.response_chunks.to_string(),
    ));
    details.push(field(
        "Server evaluation tokens",
        &optional_u64(metrics.eval_count),
    ));
    details.push(field(
        "Server evaluation duration",
        &nanoseconds(metrics.eval_duration_ns),
    ));
    details.push(field(
        "Model load duration",
        &nanoseconds(metrics.load_duration_ns),
    ));
    details.push(field("Completion reason", &metrics.done_reason));
}

fn add_partial_details(details: &mut Vec<String>, outcome: &str, partial: &PartialMetrics) {
    details.push(field("Outcome", outcome));
    details.push(field(
        "Partial first response text",
        &nanoseconds(partial.first_response_ns),
    ));
    details.push(field(
        "Partial response chunks",
        &partial.response_chunks.to_string(),
    ));
}

fn detail(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_use_markup(false);
    row.set_title(&display_text(title));
    row.set_subtitle(value);
    row.set_title_lines(0);
    row.set_subtitle_lines(0);
    row
}

fn field(title: &str, value: &str) -> String {
    format!("{}: {}", display_text(title), display_text(value))
}

fn label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(display_text(text))
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build()
}

fn clear_entries(container: &gtk::ListBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn completion_title(completion: &RunCompletion) -> &'static str {
    match &completion.outcome {
        RunOutcome::Completed(_) => "Completed",
        RunOutcome::Cancelled(_) => "Cancelled",
        RunOutcome::TimedOut(_) => "Timed out",
        RunOutcome::Failed { .. } => "Failed",
    }
}

fn timestamp(unix_ms: u64) -> String {
    i64::try_from(unix_ms / 1000)
        .ok()
        .and_then(|seconds| gtk::glib::DateTime::from_unix_utc(seconds).ok())
        .and_then(|date| date.format("%Y-%m-%d %H:%M:%S").ok())
        .map(|formatted| format!("{formatted}.{:03} UTC", unix_ms % 1000))
        .unwrap_or_else(|| "Unknown".into())
}

fn bytes(bytes: Option<u64>) -> String {
    bytes
        .map(|bytes| format!("{bytes} bytes"))
        .unwrap_or_else(|| "Unknown".into())
}

fn nanoseconds(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value} ns"))
        .unwrap_or_else(|| "Unknown".into())
}

fn optional(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Unknown")
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "Unknown".into())
}

fn optional_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "Unknown".into())
}

fn display_text(text: &str) -> String {
    let mut characters = text.chars();
    let prefix: String = characters.by_ref().take(MAX_TEXT).collect();
    if characters.next().is_some() {
        format!("{prefix}… (truncated)")
    } else {
        prefix
    }
}

#[cfg(test)]
#[path = "benchmark_history_smoke.rs"]
mod smoke;
