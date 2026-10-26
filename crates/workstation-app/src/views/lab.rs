// SPDX-License-Identifier: GPL-3.0-or-later
//! The Benchmark Lab page: Check, Run and Cancel for a managed llama.cpp server, the stored
//! experiments, the diagnostic findings and the A/B comparison. All work goes through
//! `LabCommand`; nothing here blocks, and nothing runs until the user presses Run after a
//! successful Check of exactly the same request.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use adw::prelude::*;
use tokio::sync::mpsc;
use workstation_bench::lab::Phase;
use workstation_core::{
    db::experiments::ExperimentRow,
    experiments::{Experiment, FindingStatus, Ownership, State as RunState, Variable},
};

use super::lab_text::{
    baseline_label, comparison_text, finding_detail, finding_title, state_label, summary_text,
    variable_label,
};
use crate::lab::{LabCommand, LabEvent, LabRequest};

const MAX_PICKED: usize = 2;
const DEFAULT_PASSES: f64 = 3.0;

#[derive(Default)]
struct State {
    next_id: u64,
    check: Option<(u64, LabRequest)>,
    /// The request the last successful Check described; Run sends exactly this.
    checked: Option<LabRequest>,
    run: Option<(u64, Arc<AtomicBool>)>,
    list: Option<u64>,
    load: Option<u64>,
    compare: Option<u64>,
    picked: Vec<i64>,
    shown: Option<i64>,
}

struct Inner {
    commands: mpsc::Sender<LabCommand>,
    state: RefCell<State>,
    passes: gtk::SpinButton,
    cold: gtk::CheckButton,
    check: gtk::Button,
    run: gtk::Button,
    cancel: gtk::Button,
    check_text: gtk::Label,
    progress_text: gtk::Label,
    bar: gtk::ProgressBar,
    notice: gtk::Label,
    result: gtk::Label,
    findings: gtk::Box,
    history: gtk::ListBox,
    history_note: gtk::Label,
    compare: gtk::Button,
    compare_text: gtk::Label,
}

#[derive(Clone)]
pub struct Lab {
    pub widget: gtk::Widget,
    inner: Rc<Inner>,
}

fn text(content: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(content)
        .wrap(true)
        .xalign(0.0)
        .selectable(true)
        .build()
}

fn heading(content: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(content)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-3"])
        .build()
}

fn clean(message: &str) -> String {
    message
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(1000)
        .collect()
}

pub(super) fn phase_text(phase: Phase, done: u32, of: u32) -> String {
    match phase {
        Phase::Starting => "Preparing".into(),
        Phase::ColdLoad => "Restarting the server to time a cold load".into(),
        Phase::Warmup => "Warm-up pass (not counted)".into(),
        Phase::Measured => format!("Measured pass {} of {of}", (done + 1).min(of.max(1))),
        Phase::Finishing => "Saving results".into(),
    }
}

impl Lab {
    pub fn new(commands: mpsc::Sender<LabCommand>) -> Self {
        let passes = gtk::SpinButton::with_range(1.0, 20.0, 1.0);
        passes.set_value(DEFAULT_PASSES);
        passes.set_tooltip_text(Some("Measured passes, after one warm-up pass"));
        let cold = gtk::CheckButton::with_label("Time a cold model load");
        cold.set_tooltip_text(Some(
            "Restarts the managed server once before the first pass",
        ));
        let check = gtk::Button::with_label("Check");
        let run = gtk::Button::builder()
            .label("Run")
            .css_classes(["suggested-action"])
            .sensitive(false)
            .build();
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .sensitive(false)
            .build();
        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        let passes_row = gtk::Box::builder().spacing(12).build();
        let passes_label = gtk::Label::new(Some("Measured passes"));
        passes_label.set_mnemonic_widget(Some(&passes));
        passes_row.append(&passes_label);
        passes_row.append(&passes);
        controls.append(&passes_row);
        controls.append(&cold);
        let buttons = gtk::Box::builder().spacing(8).build();
        buttons.append(&check);
        buttons.append(&run);
        buttons.append(&cancel);

        let check_text = text("");
        let progress_text = text("");
        let bar = gtk::ProgressBar::new();
        bar.set_visible(false);
        let notice = text("");
        notice.add_css_class("dim-label");
        let result = text("Run the lab to see measured results here.");
        let findings = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        let history = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let history_note = text("Stored experiments have not been loaded.");
        history_note.add_css_class("dim-label");
        let compare = gtk::Button::builder()
            .label("Compare the two picked runs")
            .sensitive(false)
            .halign(gtk::Align::Start)
            .build();
        let compare_text = text("");

        let intro = text(
            "Measures a llama.cpp server this application started. Pick the number of passes, press Check to see exactly what will happen, then Run. Nothing is sent over the network.",
        );
        intro.add_css_class("dim-label");
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .build();
        for child in [
            heading("Benchmark Lab").upcast::<gtk::Widget>(),
            intro.upcast(),
            controls.upcast(),
            buttons.upcast(),
            check_text.clone().upcast(),
            bar.clone().upcast(),
            progress_text.clone().upcast(),
            notice.clone().upcast(),
            heading("Result").upcast(),
            result.clone().upcast(),
            findings.clone().upcast(),
            heading("Stored experiments").upcast(),
            history_note.clone().upcast(),
            history.clone().upcast(),
            compare.clone().upcast(),
            compare_text.clone().upcast(),
        ] {
            page.append(&child);
        }
        let clamp = adw::Clamp::builder().maximum_size(900).child(&page).build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();

        let inner = Rc::new(Inner {
            commands,
            state: RefCell::new(State::default()),
            passes,
            cold,
            check,
            run,
            cancel,
            check_text,
            progress_text,
            bar,
            notice,
            result,
            findings,
            history,
            history_note,
            compare,
            compare_text,
        });
        Self::connect(&inner);
        Self {
            widget: scroll.upcast(),
            inner,
        }
    }

    fn connect(inner: &Rc<Inner>) {
        let weak = Rc::downgrade(inner);
        let on = |f: fn(&Rc<Inner>)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&inner);
                }
            }
        };
        let invalidate = on(Inner::invalidate);
        let again = invalidate.clone();
        inner.passes.connect_value_changed(move |_| invalidate());
        inner.cold.connect_toggled(move |_| again());
        let go = on(|i| i.request_check(None));
        inner.check.connect_clicked(move |_| go());
        let go = on(Inner::request_run);
        inner.run.connect_clicked(move |_| go());
        let go = on(|i| i.cancel_run());
        inner.cancel.connect_clicked(move |_| go());
        let go = on(|i| i.request_compare());
        inner.compare.connect_clicked(move |_| go());
    }

    /// Ask a run in progress to stop. Used when the window closes.
    pub fn cancel(&self) {
        self.inner.cancel_run();
    }

    pub fn load(&self) {
        self.inner.request_list();
    }

    pub fn receive(&self, event: &LabEvent) {
        self.inner.receive(event);
    }
}

impl Inner {
    fn next_id(&self) -> u64 {
        let mut state = self.state.borrow_mut();
        state.next_id = state.next_id.wrapping_add(1);
        state.next_id
    }

    fn send(&self, command: LabCommand) -> bool {
        if self.commands.try_send(command).is_err() {
            self.notice
                .set_text("The lab worker is not available. Reopen the application to retry.");
            return false;
        }
        true
    }

    fn controls(&self) {
        let state = self.state.borrow();
        let idle = state.run.is_none();
        self.check.set_sensitive(idle && state.check.is_none());
        self.run.set_sensitive(idle && state.checked.is_some());
        self.cancel.set_sensitive(!idle);
        self.passes.set_sensitive(idle);
        self.cold.set_sensitive(idle);
        self.compare
            .set_sensitive(idle && state.picked.len() == MAX_PICKED);
    }

    /// A change to the request makes the last Check describe something else.
    fn invalidate(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            state.checked = None;
            state.check = None;
        }
        self.check_text.set_text("");
        self.controls();
    }

    fn request(&self, follow_up: Option<(i64, Variable)>) -> LabRequest {
        LabRequest {
            passes: self.passes.value_as_int().max(1) as u32,
            cold: self.cold.is_active(),
            follow_up,
            server: None,
        }
    }

    fn request_check(self: &Rc<Self>, follow_up: Option<(i64, Variable)>) {
        if self.state.borrow().run.is_some() {
            return;
        }
        let request = self.request(follow_up);
        let id = self.next_id();
        {
            let mut state = self.state.borrow_mut();
            state.checked = None;
            state.check = Some((id, request.clone()));
        }
        self.check_text.set_text("Checking…");
        self.notice.set_text("");
        if !self.send(LabCommand::Check { id, request }) {
            self.state.borrow_mut().check = None;
            self.check_text.set_text("");
        }
        self.controls();
    }

    fn request_run(self: &Rc<Self>) {
        let Some(request) = self.state.borrow().checked.clone() else {
            return;
        };
        if self.state.borrow().run.is_some() {
            return;
        }
        let id = self.next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        if !self.send(LabCommand::Run {
            id,
            request: request.clone(),
            cancel: cancel.clone(),
        }) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.run = Some((id, cancel));
            // One Check covers one Run.
            state.checked = None;
        }
        self.notice.set_text("");
        self.bar.set_visible(true);
        self.bar.set_fraction(0.0);
        self.progress_text.set_text("Preparing");
        self.controls();
    }

    fn cancel_run(&self) {
        if let Some((_, cancel)) = &self.state.borrow().run {
            cancel.store(true, Ordering::Relaxed);
            self.progress_text.set_text("Cancelling…");
        }
    }

    fn request_list(&self) {
        let id = self.next_id();
        self.state.borrow_mut().list = Some(id);
        if !self.send(LabCommand::List { id }) {
            self.state.borrow_mut().list = None;
        }
    }

    fn request_compare(&self) {
        let picked = self.state.borrow().picked.clone();
        let [a, b] = picked[..] else { return };
        // The older run is the baseline.
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let id = self.next_id();
        self.state.borrow_mut().compare = Some(id);
        self.compare_text.set_text("Comparing…");
        if !self.send(LabCommand::Compare { id, a, b }) {
            self.state.borrow_mut().compare = None;
            self.compare_text.set_text("");
        }
    }

    fn receive(self: &Rc<Self>, event: &LabEvent) {
        match event {
            LabEvent::Checked { id, result } => {
                let request = {
                    let mut state = self.state.borrow_mut();
                    match state.check.take() {
                        Some((expected, request)) if expected == *id => request,
                        other => {
                            state.check = other;
                            return;
                        }
                    }
                };
                match result {
                    Ok(checked) => {
                        self.check_text.set_text(&clean(&checked.description));
                        // Run is bound to the server this Check looked at.
                        self.state.borrow_mut().checked = Some(LabRequest {
                            server: Some(checked.server.clone()),
                            ..request
                        });
                    }
                    Err(message) => self.check_text.set_text(&clean(message)),
                }
                self.controls();
            }
            LabEvent::Progress { id, progress } => {
                if self.state.borrow().run.as_ref().map(|r| r.0) != Some(*id) {
                    return;
                }
                let of = progress.of.max(1);
                self.bar
                    .set_fraction((f64::from(progress.done) / f64::from(of)).clamp(0.0, 1.0));
                if !self.cancel_requested() {
                    self.progress_text.set_text(&phase_text(
                        progress.phase,
                        progress.done,
                        progress.of,
                    ));
                }
            }
            LabEvent::Finished { id, result } => {
                if self.state.borrow().run.as_ref().map(|r| r.0) != Some(*id) {
                    return;
                }
                self.state.borrow_mut().run = None;
                self.bar.set_visible(false);
                self.progress_text.set_text("");
                match result {
                    Ok(done) => {
                        self.show(&done.experiment);
                        self.notice
                            .set_text(&done.notice.as_deref().map(clean).unwrap_or_default());
                    }
                    Err(message) => self.notice.set_text(&clean(message)),
                }
                self.controls();
                // A run that failed part way is still stored.
                self.request_list();
            }
            LabEvent::Listed { id, result } => {
                if self.state.borrow().list != Some(*id) {
                    return;
                }
                self.state.borrow_mut().list = None;
                match result {
                    Ok(listing) => {
                        self.state.borrow_mut().picked.clear();
                        self.fill_history(&listing.rows);
                        let mut note = if listing.rows.is_empty() {
                            "No experiments yet.".to_string()
                        } else {
                            format!("Showing the newest {} experiments.", listing.rows.len())
                        };
                        if listing.unreadable > 0 {
                            note.push_str(&format!(
                                " {} stored records could not be read and are kept untouched.",
                                listing.unreadable
                            ));
                        }
                        self.history_note.set_text(&note);
                    }
                    Err(message) => self.history_note.set_text(&clean(message)),
                }
                self.controls();
            }
            LabEvent::Loaded { id, result } => {
                if self.state.borrow().load != Some(*id) {
                    return;
                }
                self.state.borrow_mut().load = None;
                match result {
                    Ok(Some(experiment)) => self.show(experiment),
                    Ok(None) => self.notice.set_text("That experiment no longer exists."),
                    Err(message) => self.notice.set_text(&clean(message)),
                }
            }
            LabEvent::Deleted { experiment, result } => {
                // A View still in flight must not bring the deleted run back.
                self.state.borrow_mut().load = None;
                match result {
                    Ok(()) => {
                        if self.state.borrow().shown == Some(*experiment) {
                            self.state.borrow_mut().shown = None;
                            self.result
                                .set_text("Run the lab to see measured results here.");
                            self.clear_findings();
                        }
                    }
                    Err(message) => self.notice.set_text(&clean(message)),
                }
                self.request_list();
            }
            LabEvent::Compared { id, result } => {
                if self.state.borrow().compare != Some(*id) {
                    return;
                }
                self.state.borrow_mut().compare = None;
                match result {
                    Ok(comparison) => self.compare_text.set_text(&comparison_text(comparison)),
                    Err(message) => self.compare_text.set_text(&clean(message)),
                }
            }
        }
    }

    fn cancel_requested(&self) -> bool {
        self.state
            .borrow()
            .run
            .as_ref()
            .is_some_and(|(_, cancel)| cancel.load(Ordering::Relaxed))
    }

    fn clear_findings(&self) {
        while let Some(child) = self.findings.first_child() {
            self.findings.remove(&child);
        }
    }

    fn show(self: &Rc<Self>, experiment: &Experiment) {
        self.state.borrow_mut().shown = Some(experiment.id);
        self.result.set_text(&clean(&summary_text(experiment)));
        self.clear_findings();
        if experiment.findings.is_empty() {
            return;
        }
        self.findings.append(&heading("Diagnostics"));
        let can_follow_up = experiment.state == RunState::Completed
            && experiment.spec.ownership == Ownership::Managed;
        for finding in &experiment.findings {
            let title = gtk::Label::builder()
                .label(finding_title(finding))
                .xalign(0.0)
                .wrap(true)
                .css_classes(if finding.status == FindingStatus::Triggered {
                    ["heading", "warning"]
                } else {
                    ["heading", "dim-label"]
                })
                .build();
            self.findings.append(&title);
            self.findings
                .append(&text(&clean(&finding_detail(finding))));
            if let (true, FindingStatus::Triggered, Some(variable)) =
                (can_follow_up, finding.status, finding.follow_up)
            {
                let button = gtk::Button::builder()
                    .label(format!("Check a follow-up: {}", variable_label(variable)))
                    .halign(gtk::Align::Start)
                    .build();
                let weak = Rc::downgrade(self);
                let baseline = experiment.id;
                button.connect_clicked(move |_| {
                    if let Some(inner) = weak.upgrade() {
                        inner.request_check(Some((baseline, variable)));
                    }
                });
                self.findings.append(&button);
            }
        }
    }

    fn fill_history(self: &Rc<Self>, rows: &[ExperimentRow]) {
        while let Some(child) = self.history.first_child() {
            self.history.remove(&child);
        }
        for row in rows {
            self.history.append(&self.history_row(row));
        }
    }

    fn history_row(self: &Rc<Self>, row: &ExperimentRow) -> adw::ActionRow {
        let engine = match row.engine {
            workstation_core::experiments::Engine::Llamacpp => "llama.cpp",
            workstation_core::experiments::Engine::Ollama => "Ollama",
        };
        let mut detail = vec![format!("{engine} \u{b7} {}", state_label(row.state))];
        if let Some(speed) = row
            .summary
            .as_ref()
            .and_then(|s| s.decode_tokens_per_s.as_ref())
        {
            detail.push(format!("{:.1} tokens/s", speed.median));
        }
        if let Some(variable) = row.variable {
            detail.push(format!(
                "follow-up of {}: {}",
                baseline_label(row.baseline_id),
                variable_label(variable)
            ));
        }
        let action = adw::ActionRow::builder()
            .title(glib_escape(&format!(
                "#{} \u{b7} {}",
                row.id, row.model_name
            )))
            .subtitle(glib_escape(&detail.join(" \u{b7} ")))
            .build();
        let pick = gtk::CheckButton::builder()
            .valign(gtk::Align::Center)
            .tooltip_text("Pick two runs to compare")
            .build();
        let view = gtk::Button::builder()
            .label("View")
            .valign(gtk::Align::Center)
            .build();
        let delete = gtk::Button::builder()
            .label("Delete")
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        let id = row.id;
        let weak = Rc::downgrade(self);
        pick.connect_toggled(move |button| {
            let Some(inner) = weak.upgrade() else { return };
            let mut state = inner.state.borrow_mut();
            if button.is_active() {
                if state.picked.len() >= MAX_PICKED {
                    drop(state);
                    button.set_active(false);
                    inner
                        .compare_text
                        .set_text("Only two runs can be compared at a time.");
                    return;
                }
                state.picked.push(id);
            } else {
                state.picked.retain(|p| *p != id);
            }
            drop(state);
            inner.controls();
        });
        let weak = Rc::downgrade(self);
        view.connect_clicked(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            let request = inner.next_id();
            inner.state.borrow_mut().load = Some(request);
            if !inner.send(LabCommand::Load {
                id: request,
                experiment: id,
            }) {
                inner.state.borrow_mut().load = None;
            }
        });
        let weak = Rc::downgrade(self);
        delete.connect_clicked(move |button| {
            // The first press asks, the second deletes.
            if button.label().as_deref() != Some("Confirm delete") {
                button.set_label("Confirm delete");
                return;
            }
            let Some(inner) = weak.upgrade() else { return };
            inner.state.borrow_mut().load = None;
            inner.send(LabCommand::Delete { experiment: id });
        });
        action.add_prefix(&pick);
        let flow = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();
        flow.append(&view);
        flow.append(&delete);
        action.add_suffix(&flow);
        action
    }
}

/// Row titles are Pango markup; model names come from files.
fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(&clean(text)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_text_never_counts_past_the_last_pass() {
        assert_eq!(phase_text(Phase::Measured, 0, 3), "Measured pass 1 of 3");
        assert_eq!(phase_text(Phase::Measured, 3, 3), "Measured pass 3 of 3");
        assert_eq!(
            phase_text(Phase::ColdLoad, 0, 3),
            "Restarting the server to time a cold load"
        );
    }

    #[test]
    fn control_characters_are_stripped_from_shown_text() {
        assert_eq!(clean("a\u{1b}[31mb\nc"), "a[31mb\nc");
    }
}

#[cfg(test)]
#[path = "lab_smoke.rs"]
mod lab_smoke;
