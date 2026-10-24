// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{
    bridge::CoreCommand,
    fit_state::{Completion, FitState},
    fit_workflow::{FitReport, FitRequest, parse_scenario},
};
use adw::prelude::*;
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use workstation_core::{
    fit::{
        gguf_fit::{GgufFitStatus, LayoutAssumption},
        memory_math::{
            FitAssessment, FitClass, FitReason, KvCacheType, MemoryBreakdown, MemoryLimits,
        },
    },
    models::gguf::LayoutWarning,
};

const GIB: f64 = 1_073_741_824.0;

#[cfg(test)]
#[path = "model_fit_smoke.rs"]
mod smoke;

struct PanelState {
    requests: FitState,
    selected_path: Option<PathBuf>,
    available_ram_bytes: Option<u64>,
    captured_at_unix_ms: Option<u64>,
    hardware_error: Option<String>,
}

struct Inner {
    commands: tokio::sync::mpsc::Sender<CoreCommand>,
    state: RefCell<PanelState>,
    file_row: adw::ActionRow,
    choose_file: gtk::Button,
    context_entry: adw::EntryRow,
    microbatch_entry: adw::EntryRow,
    key_cache: adw::ComboRow,
    value_cache: adw::ComboRow,
    assume_layout: adw::SwitchRow,
    calculate: gtk::Button,
    spinner: gtk::Spinner,
    budget_label: gtk::Label,
    status_label: gtk::Label,
    metadata_label: gtk::Label,
    warning_label: gtk::Label,
    breakdown_label: gtk::Label,
}

pub struct ModelFit {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
}

impl ModelFit {
    pub fn new(commands: tokio::sync::mpsc::Sender<CoreCommand>) -> Self {
        let file_group = adw::PreferencesGroup::builder()
            .title("Model artifact")
            .description(
                "Choose a local GGUF file. The inspector reads metadata and tensor layout only.",
            )
            .build();
        let file_row = adw::ActionRow::builder()
            .title("GGUF file")
            .subtitle("No file selected")
            .use_markup(false)
            .build();
        let choose_file = gtk::Button::with_label("Choose file…");
        choose_file.set_valign(gtk::Align::Center);
        file_row.add_suffix(&choose_file);
        file_row.set_activatable_widget(Some(&choose_file));
        file_group.add(&file_row);

        let scenario_group = adw::PreferencesGroup::builder()
            .title("CPU scenario")
            .description("Requested values for this estimate; they are not read from a running inference process.")
            .build();
        let context_entry = adw::EntryRow::builder()
            .title("Requested context tokens")
            .text("4096")
            .input_purpose(gtk::InputPurpose::Digits)
            .build();
        let microbatch_entry = adw::EntryRow::builder()
            .title("Prompt microbatch tokens")
            .text("512")
            .input_purpose(gtk::InputPurpose::Digits)
            .build();
        let key_cache = cache_row("Key cache type");
        let value_cache = cache_row("Value cache type");
        let sequence_row = adw::ActionRow::builder()
            .title("Sequences")
            .subtitle("1 sequence · fixed scenario assumption, not runtime-observed")
            .use_markup(false)
            .build();
        scenario_group.add(&context_entry);
        scenario_group.add(&microbatch_entry);
        scenario_group.add(&key_cache);
        scenario_group.add(&value_cache);
        scenario_group.add(&sequence_row);

        let layout_group = adw::PreferencesGroup::builder()
            .title("Architecture assumption")
            .description("GGUF metadata cannot prove full-attention layout. Enabling this permits an estimate only; runtime compatibility remains unknown.")
            .build();
        let assume_layout = adw::SwitchRow::builder()
            .title("Assume uniform full attention")
            .subtitle("Explicit scenario assumption; does not verify the model or runtime")
            .active(false)
            .build();
        layout_group.add(&assume_layout);

        let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let calculate = gtk::Button::with_mnemonic("_Calculate estimate");
        calculate.add_css_class("suggested-action");
        calculate.set_sensitive(false);
        let spinner = gtk::Spinner::new();
        action_row.append(&spinner);
        action_row.append(&calculate);

        let result_group = adw::PreferencesGroup::builder()
            .title("Estimate")
            .description(
                "A static memory estimate is not a runtime compatibility check or benchmark.",
            )
            .build();
        let budget_label = plain_label("Available RAM: waiting for a hardware reading.");
        let status_label = plain_label("Choose a local GGUF file to begin.");
        let metadata_label = plain_label("No metadata inspected.");
        let warning_label = plain_label("No layout warnings.");
        let breakdown_label = plain_label("No memory breakdown.");
        for label in [
            &budget_label,
            &status_label,
            &metadata_label,
            &warning_label,
            &breakdown_label,
        ] {
            result_group.add(label);
        }

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        widget.append(&file_group);
        widget.append(&scenario_group);
        widget.append(&layout_group);
        widget.append(&action_row);
        widget.append(&result_group);

        let inner = Rc::new(Inner {
            commands,
            state: RefCell::new(PanelState {
                requests: FitState::default(),
                selected_path: None,
                available_ram_bytes: None,
                captured_at_unix_ms: None,
                hardware_error: None,
            }),
            file_row,
            choose_file,
            context_entry,
            microbatch_entry,
            key_cache,
            value_cache,
            assume_layout,
            calculate,
            spinner,
            budget_label,
            status_label,
            metadata_label,
            warning_label,
            breakdown_label,
        });

        connect_controls(&inner);
        refresh_calculate_button(&inner);
        Self { widget, inner }
    }

    pub fn receive(&self, id: u64, result: &Result<Box<FitReport>, String>) {
        let completion = { self.inner.state.borrow_mut().requests.complete(id) };
        match completion {
            Completion::Ignored => (),
            Completion::Stale => {
                self.inner.spinner.stop();
                refresh_calculate_button(&self.inner);
            }
            Completion::Current => {
                self.inner.spinner.stop();
                match result {
                    Ok(report) => render_report(&self.inner, report),
                    Err(message) => {
                        self.inner
                            .status_label
                            .set_text(&format!("Estimate failed: {message}\nNo result is shown."));
                        self.inner.metadata_label.set_text("No metadata result.");
                        self.inner
                            .warning_label
                            .set_text("No layout warnings available.");
                        self.inner
                            .breakdown_label
                            .set_text("No memory breakdown available.");
                    }
                }
                refresh_calculate_button(&self.inner);
            }
        }
    }

    pub fn hardware_reading(&self, available: Option<u64>, captured_at_unix_ms: u64) {
        invalidate_result(&self.inner, "Hardware readings changed; calculate again.");
        let mut state = self.inner.state.borrow_mut();
        state.available_ram_bytes = available;
        state.captured_at_unix_ms = Some(captured_at_unix_ms);
        state.hardware_error = None;
        drop(state);
        update_budget(&self.inner);
        refresh_calculate_button(&self.inner);
    }

    pub fn hardware_failed(&self) {
        invalidate_result(
            &self.inner,
            "Hardware reading failed; the RAM budget is unknown.",
        );
        let mut state = self.inner.state.borrow_mut();
        state.available_ram_bytes = None;
        state.captured_at_unix_ms = None;
        state.hardware_error = Some("Latest hardware reading failed".into());
        drop(state);
        update_budget(&self.inner);
        refresh_calculate_button(&self.inner);
    }
}

fn connect_controls(inner: &Rc<Inner>) {
    let weak = Rc::downgrade(inner);
    inner.choose_file.connect_clicked(move |button| {
        if let Some(inner) = weak.upgrade() {
            choose_file(&inner, button);
        }
    });

    for entry in [&inner.context_entry, &inner.microbatch_entry] {
        let weak = Rc::downgrade(inner);
        entry.connect_changed(move |_| {
            if let Some(inner) = weak.upgrade() {
                invalidate_result(&inner, "Scenario changed; previous estimate cleared.");
                refresh_calculate_button(&inner);
            }
        });
    }
    for combo in [&inner.key_cache, &inner.value_cache] {
        let weak = Rc::downgrade(inner);
        combo.connect_selected_notify(move |_| {
            if let Some(inner) = weak.upgrade() {
                invalidate_result(&inner, "Scenario changed; previous estimate cleared.");
                refresh_calculate_button(&inner);
            }
        });
    }
    let weak = Rc::downgrade(inner);
    inner.assume_layout.connect_active_notify(move |_| {
        if let Some(inner) = weak.upgrade() {
            invalidate_result(
                &inner,
                "Layout assumption changed; previous estimate cleared.",
            );
            refresh_calculate_button(&inner);
        }
    });
    let weak = Rc::downgrade(inner);
    inner.calculate.connect_clicked(move |_| {
        if let Some(inner) = weak.upgrade() {
            calculate(&inner);
        }
    });
}

fn choose_file(inner: &Rc<Inner>, button: &gtk::Button) {
    let Some(parent) = button
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    else {
        inner
            .status_label
            .set_text("The file chooser needs an active window.");
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title("Choose a local GGUF file")
        .modal(true)
        .accept_label("Choose")
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("GGUF files (*.gguf)"));
    filter.add_pattern("*.gguf");
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));

    let weak = Rc::downgrade(inner);
    dialog.open(
        Some(&parent),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(file) => match file.path() {
                    Some(path) => set_selected_file(&inner, path),
                    None => {
                        clear_selected_file(&inner, "Only local files can be inspected.");
                    }
                },
                Err(error)
                    if error.matches(gtk::DialogError::Cancelled)
                        || error.matches(gtk::DialogError::Dismissed) => {}
                Err(_) => inner
                    .status_label
                    .set_text("The file chooser could not open. Try choosing the file again."),
            }
        },
    );
}

fn set_selected_file(inner: &Rc<Inner>, path: PathBuf) {
    invalidate_result(inner, "File selection changed; previous estimate cleared.");
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Selected GGUF file");
    inner.file_row.set_subtitle(filename);
    inner.state.borrow_mut().selected_path = Some(path);
    refresh_calculate_button(inner);
}

fn clear_selected_file(inner: &Rc<Inner>, message: &str) {
    invalidate_result(inner, message);
    inner.file_row.set_subtitle("No local file selected");
    inner.state.borrow_mut().selected_path = None;
    refresh_calculate_button(inner);
}

fn calculate(inner: &Rc<Inner>) {
    let Some(path) = inner.state.borrow().selected_path.clone() else {
        inner
            .status_label
            .set_text("Choose a local GGUF file first.");
        return;
    };
    let key_cache = selected_cache(&inner.key_cache);
    let value_cache = selected_cache(&inner.value_cache);
    let (Some(key_cache), Some(value_cache)) = (key_cache, value_cache) else {
        inner
            .status_label
            .set_text("Choose both cache types before calculating.");
        return;
    };
    let scenario = match parse_scenario(
        inner.context_entry.text().as_str(),
        inner.microbatch_entry.text().as_str(),
        key_cache,
        value_cache,
    ) {
        Ok(scenario) => scenario,
        Err(message) => {
            inner.status_label.set_text(message);
            return;
        }
    };
    let layout = if inner.assume_layout.is_active() {
        LayoutAssumption::UniformFullAttention
    } else {
        LayoutAssumption::Unconfirmed
    };
    let id = match inner.state.borrow_mut().requests.begin() {
        Some(id) => id,
        None => return,
    };
    inner.calculate.set_sensitive(false);
    inner.spinner.start();
    inner
        .status_label
        .set_text("Inspecting the selected file and estimating the supplied scenario…");
    clear_report_fields(inner);
    let request = CoreCommand::EstimateFit(FitRequest {
        id,
        path,
        scenario,
        layout,
    });
    let send_error = match inner.commands.try_send(request) {
        Ok(()) => None,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            Some("Worker is busy. Wait for the hardware refresh to finish, then calculate again.")
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            Some("Worker is unavailable. Close and reopen the application to retry.")
        }
    };
    if let Some(message) = send_error {
        let completion = inner.state.borrow_mut().requests.complete(id);
        if completion != Completion::Ignored {
            inner.spinner.stop();
            inner.status_label.set_text(message);
            refresh_calculate_button(inner);
        }
    }
}

fn invalidate_result(inner: &Rc<Inner>, message: &str) {
    inner.state.borrow_mut().requests.invalidate();
    clear_report_fields(inner);
    inner.status_label.set_text(message);
}

fn clear_report_fields(inner: &Inner) {
    inner.metadata_label.set_text("No metadata result.");
    inner
        .warning_label
        .set_text("Layout warnings have not been inspected.");
    inner.breakdown_label.set_text("No memory breakdown.");
}

fn refresh_calculate_button(inner: &Rc<Inner>) {
    let state = inner.state.borrow();
    inner
        .calculate
        .set_sensitive(state.selected_path.is_some() && !state.requests.is_busy());
}

fn update_budget(inner: &Inner) {
    let state = inner.state.borrow();
    let available = state
        .available_ram_bytes
        .map(|bytes| format!("{} ({:.2} GiB)", bytes, bytes as f64 / GIB))
        .unwrap_or_else(|| "Unknown".into());
    let captured = state
        .captured_at_unix_ms
        .map(format_timestamp)
        .unwrap_or_else(|| "time unavailable".into());
    let notice = state
        .hardware_error
        .as_deref()
        .map(|message| format!(" · {message}"))
        .unwrap_or_default();
    inner.budget_label.set_text(&format!(
        "Available RAM reading: {available} · captured {captured}{notice}. Each estimate requests a fresh worker reading."
    ));
}

fn render_report(inner: &Inner, report: &FitReport) {
    {
        let mut state = inner.state.borrow_mut();
        state.available_ram_bytes = report.available_ram_bytes;
        state.captured_at_unix_ms = Some(report.captured_at_unix_ms);
        state.hardware_error = report.ram_notice.clone();
    }
    update_budget(inner);

    let architecture = report
        .metadata
        .architecture
        .as_deref()
        .map(|name| format!("{name:?}"))
        .unwrap_or_else(|| "Unknown".into());
    let llama = report
        .metadata
        .llama
        .as_ref()
        .map(|meta| {
            format!(
                "layers {}, embedding {}, attention heads {}, KV heads {}, key length {}, value length {}, declared context {}",
                display_opt(meta.block_count),
                display_opt(meta.embedding_length),
                display_opt(meta.attention_heads),
                display_opt(meta.kv_heads),
                display_opt(meta.key_length),
                display_opt(meta.value_length),
                display_opt(meta.context_length),
            )
        })
        .unwrap_or_else(|| "No LLaMA metadata record".into());
    inner.metadata_label.set_text(&format!(
        "Architecture: {architecture}\nFile size estimate: {} bytes\nTensor count: {} · tensor storage bytes: {}\nLLaMA declarations: {llama}",
        report.metadata.file_bytes,
        report.metadata.tensor_count,
        report.metadata.tensor_bytes,
    ));

    let warnings = if report.metadata.warnings.is_empty() {
        "No recognized layout warning markers. Their absence does not prove dense full attention."
            .into()
    } else {
        report
            .metadata
            .warnings
            .iter()
            .map(warning_text)
            .collect::<Vec<_>>()
            .join(" · ")
    };
    inner
        .warning_label
        .set_text(&format!("Layout warnings: {warnings}"));

    let status = status_text(report.estimate.status);
    let assessment = &report.estimate.assessment;
    let class = fit_class_text(assessment.class);
    let reason = fit_reason_text(assessment.reason);
    inner.status_label.set_text(&format!(
        "{status}\nFit assessment: {class} · {reason}\nRuntime compatibility remains unknown; this result is not verification."
    ));
    inner.breakdown_label.set_text(&assessment_text(assessment));
}

fn assessment_text(assessment: &FitAssessment) -> String {
    let mut lines = Vec::new();
    match assessment.breakdown {
        Some(MemoryBreakdown {
            weights_bytes,
            kv_cache_bytes,
            buffer_bytes,
            overhead_bytes,
            total_bytes,
            allocated_context_tokens,
        }) => {
            lines.push("Estimated memory breakdown (bytes; static scenario):".into());
            for (name, bytes) in [
                ("Weights / file size", weights_bytes),
                ("KV cache", kv_cache_bytes),
                ("Compute buffers", buffer_bytes),
                ("Runtime allowance", overhead_bytes),
                ("Total", total_bytes),
            ] {
                lines.push(format!("{name}: {bytes} ({:.2} GiB)", bytes as f64 / GIB));
            }
            lines.push(format!(
                "Context allocation rounded to {allocated_context_tokens} tokens."
            ));
        }
        None => lines.push("Memory breakdown unavailable for these inputs and assumptions.".into()),
    }
    match assessment.limits {
        Some(MemoryLimits {
            comfortable_bytes,
            maximum_bytes,
        }) => {
            lines.push(format!(
                "CPU RAM budget limits: comfortable {} bytes · maximum {} bytes.",
                comfortable_bytes, maximum_bytes
            ));
        }
        None => lines.push(
            "CPU budget limits unavailable because no current RAM value was supplied.".into(),
        ),
    }
    lines.join("\n")
}

fn cache_row(title: &str) -> adw::ComboRow {
    let values = gtk::StringList::new(&["F16", "Q8_0", "Q4_0"]);
    let expression = gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    );
    adw::ComboRow::builder()
        .title(title)
        .model(&values)
        .expression(&expression)
        .selected(0)
        .build()
}

fn selected_cache(row: &adw::ComboRow) -> Option<KvCacheType> {
    match row.selected() {
        0 => Some(KvCacheType::F16),
        1 => Some(KvCacheType::Q8_0),
        2 => Some(KvCacheType::Q4_0),
        _ => None,
    }
}

fn plain_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .use_markup(false)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .build()
}

fn display_opt(value: Option<u32>) -> String {
    value.map_or_else(|| "Unknown".into(), |value| value.to_string())
}

fn status_text(status: GgufFitStatus) -> &'static str {
    match status {
        GgufFitStatus::NoTensorData => {
            "No tensor data was found; no weights estimate is available."
        }
        GgufFitStatus::UnsupportedArchitecture => {
            "Architecture is not supported by this narrow estimate."
        }
        GgufFitStatus::LayoutWarnings => {
            "This model declares a layout this estimate cannot evaluate."
        }
        GgufFitStatus::UnknownSequenceCount => {
            "Sequence count is unknown; one-sequence fit math is unavailable."
        }
        GgufFitStatus::MultipleSequences => "This estimate supports one sequence only.",
        GgufFitStatus::UnconfirmedLayout => {
            "Layout is unconfirmed. An explicit attention assumption is required for a conditional estimate."
        }
        GgufFitStatus::MissingDimensions => {
            "Required architecture dimensions are missing; no complete estimate is available."
        }
        GgufFitStatus::AssumedGeometry => {
            "This estimate uses an explicit assumption of uniform full attention; model layout is unverified."
        }
    }
}

fn fit_class_text(class: FitClass) -> &'static str {
    match class {
        FitClass::Comfortable | FitClass::Constrained => {
            "Unverified; a memory estimate cannot establish runtime support"
        }
        FitClass::NotRecommended => "Not recommended for this CPU scenario",
        FitClass::Unknown => "Unknown",
    }
}

fn fit_reason_text(reason: FitReason) -> &'static str {
    match reason {
        FitReason::WithinComfortableBudget => "within policy comfortable limit",
        FitReason::WithinMaximumBudget => "within supplied maximum budget",
        FitReason::ExceedsMaximumBudget => "exceeds supplied maximum budget",
        FitReason::UnsupportedRuntime => "runtime unsupported",
        FitReason::UnverifiedRuntime => "runtime compatibility unverified",
        FitReason::MissingArtifactMetadata => "artifact size unavailable",
        FitReason::MissingArchitectureMetadata => "required architecture metadata unavailable",
        FitReason::MissingMemoryReading => "available memory reading unavailable",
    }
}

fn warning_text(warning: &LayoutWarning) -> &'static str {
    match warning {
        LayoutWarning::ExpertMetadata => "expert/MoE metadata",
        LayoutWarning::SlidingWindowMetadata => "sliding-window attention metadata",
        LayoutWarning::RecurrentMetadata => "recurrent/SSM metadata",
        LayoutWarning::HybridMetadata => "hybrid attention metadata",
        LayoutWarning::AdapterMetadata => "adapter metadata",
        LayoutWarning::NonCausalAttention => "non-causal attention declaration",
        LayoutWarning::NonModelType => "artifact is not declared as a base model",
        LayoutWarning::TensorLayoutMetadata => "special tensor layout metadata",
        LayoutWarning::AuxiliaryMetadata => "vision or auxiliary model metadata",
    }
}

fn format_timestamp(timestamp_ms: u64) -> String {
    gtk::glib::DateTime::from_unix_local((timestamp_ms / 1000) as i64)
        .and_then(|time| time.format("%H:%M:%S"))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "unknown time".into())
}
