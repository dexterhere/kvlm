// SPDX-License-Identifier: GPL-3.0-or-later
//! Downloads panel: progress, pause, resume, cancel and discard for model files.
//! It reads saved state at startup and never starts a transfer on its own.

use crate::bridge::{CoreCommand, UiEvent};
use adw::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use tokio::sync::mpsc;
use workstation_core::{
    db::downloads::{DownloadRow, DownloadState},
    models::artifact::{ArtifactFormat, ModelArtifact},
};
use workstation_runtimes::downloader::{DownloadEnd, Progress};

const MAX_TEXT: usize = 256;

#[derive(Default)]
struct State {
    next_id: u64,
    /// Newest list request; older answers are dropped.
    list: u64,
    /// A progress reading arrived for a row that is not shown yet.
    refresh_asked: bool,
    rows: HashMap<i64, Live>,
}

struct Live {
    row: adw::ActionRow,
    bar: gtk::ProgressBar,
    expected_size: u64,
    /// The row was drawn with Pause and Cancel, so it already reflects a running transfer.
    running: bool,
}

pub struct Downloads {
    pub widget: gtk::Box,
    status: gtk::Label,
    list: gtk::ListBox,
    state: Rc<RefCell<State>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl Downloads {
    pub fn new(commands: mpsc::Sender<CoreCommand>) -> Self {
        let heading = gtk::Label::builder()
            .label("Downloads")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["dim-label"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
        for child in [
            heading.upcast_ref::<gtk::Widget>(),
            status.upcast_ref(),
            list.upcast_ref(),
        ] {
            widget.append(child);
        }
        widget.set_visible(false);
        Self {
            widget,
            status,
            list,
            state: Rc::default(),
            commands,
        }
    }

    /// Startup read of saved downloads. Local state only; no network, nothing resumes.
    pub fn load(&self) {
        self.request(|id| CoreCommand::LoadDownloads { id });
    }

    pub fn receive(&self, event: &UiEvent) {
        match event {
            UiEvent::DownloadProgress(progress) => self.progress(progress),
            UiEvent::DownloadFinished { outcome } => {
                let message = match &outcome.end {
                    DownloadEnd::Completed => "Download finished and verified.".to_owned(),
                    DownloadEnd::Paused { message, .. } => message
                        .clone()
                        .unwrap_or_else(|| "Download paused. Resume to continue.".into()),
                    DownloadEnd::Cancelled => {
                        "Download cancelled. The partial file was kept; resume or discard it."
                            .into()
                    }
                    DownloadEnd::Failed { message, .. } => message.clone(),
                };
                self.say(&message);
                self.load();
            }
            UiEvent::DownloadsLoaded { id, result } => {
                if self.state.borrow().list != *id {
                    return;
                }
                match result {
                    Ok(rows) => self.show(rows),
                    Err(message) => self.say(message),
                }
            }
            _ => {}
        }
    }

    fn say(&self, message: &str) {
        self.status.set_text(&text(message));
        self.status.set_visible(true);
        self.widget.set_visible(true);
    }

    fn request(&self, command: impl FnOnce(u64) -> CoreCommand) {
        let id = {
            let mut state = self.state.borrow_mut();
            state.next_id += 1;
            state.list = state.next_id;
            state.refresh_asked = false;
            state.next_id
        };
        send(&self.commands, &self.status, command(id));
    }

    fn progress(&self, progress: &Progress) {
        let running = {
            let state = self.state.borrow();
            state.rows.get(&progress.download_id).is_some_and(|live| {
                live.bar
                    .set_fraction(fraction(progress.bytes_done, live.expected_size));
                live.row.set_subtitle(&format!(
                    "downloading · {} of {}",
                    size_text(progress.bytes_done),
                    size_text(progress.total)
                ));
                live.running
            })
        };
        // A new or resumed transfer: redraw its row once so it offers Pause and Cancel.
        if !running && !self.state.borrow().refresh_asked {
            self.load();
            self.state.borrow_mut().refresh_asked = true;
        }
    }

    fn show(&self, rows: &[DownloadRow]) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut live = HashMap::new();
        for row in rows {
            let (widget, bar) = self.download_row(row);
            self.list.append(&widget);
            live.insert(
                row.id,
                Live {
                    row: widget,
                    bar,
                    expected_size: row.expected_size,
                    running: matches!(row.state, DownloadState::Queued | DownloadState::Running),
                },
            );
        }
        self.state.borrow_mut().rows = live;
        self.widget
            .set_visible(!rows.is_empty() || self.status.is_visible());
    }

    fn download_row(&self, row: &DownloadRow) -> (adw::ActionRow, gtk::ProgressBar) {
        let name = clean(&row.filename);
        let widget = adw::ActionRow::builder()
            .title(text(&name))
            .subtitle(state_text(row))
            .use_markup(false)
            .build();
        let bar = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .width_request(120)
            .fraction(fraction(row.bytes_done, row.expected_size))
            .build();
        bar.update_property(&[gtk::accessible::Property::Label(&format!(
            "Progress of {name}"
        ))]);
        widget.add_suffix(&bar);
        let button = |label: &str, command: Box<dyn Fn() -> CoreCommand>| {
            let button = gtk::Button::builder()
                .label(label)
                .valign(gtk::Align::Center)
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "{label} download of {name}"
            ))]);
            let (commands, status) = (self.commands.clone(), self.status.clone());
            button.connect_clicked(move |_| send(&commands, &status, command()));
            button
        };
        match row.state {
            DownloadState::Queued | DownloadState::Running => {
                widget.add_suffix(&button("Pause", Box::new(|| CoreCommand::PauseDownload)));
                widget.add_suffix(&button("Cancel", Box::new(|| CoreCommand::CancelDownload)));
            }
            DownloadState::Paused | DownloadState::Failed | DownloadState::Cancelled => {
                let artifact = artifact_of(row);
                widget.add_suffix(&button(
                    "Resume",
                    // Any finish event updates this panel, so the id is not matched.
                    Box::new(move || CoreCommand::StartDownload {
                        id: 0,
                        artifact: Box::new(artifact.clone()),
                    }),
                ));
                widget.add_suffix(&self.discard_button(row.id, &name));
            }
            DownloadState::Verifying | DownloadState::Completed => {}
        }
        (widget, bar)
    }

    /// Deleting a partial file takes two clicks.
    fn discard_button(&self, download_id: i64, name: &str) -> gtk::Button {
        let button = gtk::Button::builder()
            .label("Discard")
            .valign(gtk::Align::Center)
            .tooltip_text("Deletes the partial file. Click twice to confirm.")
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&format!(
            "Discard partial download of {name}"
        ))]);
        let armed = Rc::new(Cell::new(false));
        let (commands, status, state) = (
            self.commands.clone(),
            self.status.clone(),
            Rc::downgrade(&self.state),
        );
        button.connect_clicked(move |button| {
            if !armed.replace(true) {
                button.set_label("Confirm discard");
                button.add_css_class("destructive-action");
                return;
            }
            let Some(state) = state.upgrade() else {
                return;
            };
            let id = {
                let mut state = state.borrow_mut();
                state.next_id += 1;
                state.list = state.next_id;
                state.next_id
            };
            send(
                &commands,
                &status,
                CoreCommand::DiscardDownload { id, download_id },
            );
        });
        button
    }
}

fn send(commands: &mpsc::Sender<CoreCommand>, status: &gtk::Label, command: CoreCommand) {
    let (commands, status) = (commands.clone(), status.clone());
    gtk::glib::MainContext::default().spawn_local(async move {
        if commands.send(command).await.is_err() {
            status.set_text("Download worker unavailable. Reopen the application to retry.");
            status.set_visible(true);
        }
    });
}

/// Rebuilds what a resume needs from the saved row; the checks run again in the worker.
fn artifact_of(row: &DownloadRow) -> ModelArtifact {
    ModelArtifact {
        artifact_id: row.artifact_id.clone(),
        repo_id: row.repo_id.clone(),
        revision: row.revision.clone(),
        filename: row.filename.clone(),
        format: ArtifactFormat::Gguf,
        quantization_hint: None,
        size_bytes: Some(row.expected_size),
        sha256: Some(row.expected_sha256.clone()),
        shards: None,
    }
}

fn state_text(row: &DownloadRow) -> String {
    let progress = format!(
        "{} of {}",
        size_text(row.bytes_done),
        size_text(row.expected_size)
    );
    match row.state {
        DownloadState::Queued => "waiting to start".into(),
        DownloadState::Running => format!("downloading · {progress}"),
        DownloadState::Paused => format!("paused · {progress}"),
        DownloadState::Verifying => "verifying SHA-256…".into(),
        DownloadState::Completed => format!(
            "verified and stored in the application's model store · {}",
            size_text(row.expected_size)
        ),
        DownloadState::Failed => format!(
            "failed: {} · {progress} kept",
            failure_text(row.failure_code.as_deref())
        ),
        DownloadState::Cancelled => format!("cancelled · {progress} kept"),
    }
}

fn failure_text(code: Option<&str>) -> &'static str {
    match code {
        Some("access_required") => "access needed (the repository is gated)",
        Some("not_found") => "file not found at the pinned commit",
        Some("rate_limited") => "rate limited by Hugging Face",
        Some("network") => "network problem",
        Some("integrity") => "checksum did not match; the partial file was removed",
        Some("too_large") => "server sent more data than published; the partial file was removed",
        Some("disk_full") => "disk full",
        Some("in_use") => "another copy of the application is using the model store",
        _ => "download failed",
    }
}

fn fraction(done: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64).clamp(0.0, 1.0)
    }
}

fn size_text(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// File names and server messages are untrusted text: control characters become `?`.
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn text(value: &str) -> String {
    let mut chars = clean(value).chars().collect::<Vec<_>>().into_iter();
    let head: String = chars.by_ref().take(MAX_TEXT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
#[path = "downloads_smoke.rs"]
mod smoke;
