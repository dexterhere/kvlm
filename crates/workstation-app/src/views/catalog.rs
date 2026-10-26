// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only Hugging Face model explorer: repositories, then their GGUF files.
//! Nothing here touches the network until the user searches or expands a row.

use super::{access::Access, downloads::Downloads};
use crate::bridge::{ArtifactDetails, CoreCommand, UiEvent};
use adw::prelude::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use workstation_core::models::{
    artifact::ModelArtifact,
    dedup::{RootKind, RootState, Stop, Wanted},
    release::ModelRelease,
};
use workstation_runtimes::catalog_service::{CatalogLoad, LoadSource, ReleaseLoad};

const MAX_TEXT: usize = 512;

struct RepoRow {
    row: adw::ExpanderRow,
    /// Rows added under the expander, removed when the content is replaced.
    children: Vec<gtk::Widget>,
    /// A release request was sent and has not finished.
    loading: bool,
    loaded: bool,
}

struct ArtifactRow {
    row: adw::ExpanderRow,
    children: Vec<gtk::Widget>,
    artifact: ModelArtifact,
    /// Summary line without the local-copy note.
    base_subtitle: String,
    loading: bool,
    loaded: bool,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// Newest search request; older answers are dropped.
    search: u64,
    /// Query of the results on screen; used to continue them.
    query: String,
    /// Query of the newest request, adopted only when it succeeds.
    pending_query: String,
    next_cursor: Option<String>,
    /// The newest search asked for the next page, so results append.
    continuing: bool,
    release: Option<(u64, String)>,
    /// Newest header inspection: request id and artifact id.
    inspect: Option<(u64, String)>,
    /// Newest local-file check; older answers are dropped.
    scan: u64,
    artifacts: HashMap<String, ArtifactRow>,
    rows: HashMap<String, RepoRow>,
}

pub struct Explorer {
    pub widget: gtk::Box,
    entry: gtk::SearchEntry,
    status: gtk::Label,
    list: gtk::ListBox,
    more: gtk::Button,
    access: Access,
    downloads: Downloads,
    state: Rc<RefCell<State>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl Explorer {
    pub fn new(commands: mpsc::Sender<CoreCommand>) -> Self {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Search Hugging Face GGUF models")
            .hexpand(true)
            .build();
        let search = gtk::Button::with_label("Search");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bar.append(&entry);
        bar.append(&search);
        let status = gtk::Label::builder()
            .label("Search runs only when you ask. Nothing has been loaded yet.")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let more = gtk::Button::with_label("Load more");
        more.set_visible(false);
        let access = Access::new(commands.clone());
        let downloads = Downloads::new(commands.clone());
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        for child in [
            bar.upcast_ref::<gtk::Widget>(),
            access.widget.upcast_ref(),
            downloads.widget.upcast_ref(),
            status.upcast_ref(),
            list.upcast_ref(),
            more.upcast_ref(),
        ] {
            content.append(child);
        }
        let scroll = gtk::ScrolledWindow::builder()
            .child(&content)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_margin_top(12);
        widget.set_margin_bottom(12);
        widget.set_margin_start(12);
        widget.set_margin_end(12);
        widget.append(&scroll);

        let explorer = Self {
            widget,
            entry,
            status,
            list,
            more,
            access,
            downloads,
            state: Rc::new(RefCell::new(State::default())),
            commands,
        };
        let this = explorer.handle();
        search.connect_clicked({
            let this = this.clone();
            move |_| this.search(false)
        });
        explorer.entry.connect_activate({
            let this = this.clone();
            move |_| this.search(false)
        });
        explorer.more.connect_clicked(move |_| this.search(true));
        explorer
    }

    /// Cheap clone of the pieces the callbacks need.
    fn handle(&self) -> Handle {
        Handle {
            entry: self.entry.downgrade(),
            status: self.status.downgrade(),
            more: self.more.downgrade(),
            state: Rc::downgrade(&self.state),
            commands: self.commands.clone(),
        }
    }

    /// The search field, for the quick-search shortcut.
    pub fn search_entry(&self) -> gtk::SearchEntry {
        self.entry.clone()
    }

    /// Startup read of the saved first page. Never reaches the network.
    pub fn load_saved(&self) {
        let id = next_id(&self.state);
        {
            let mut state = self.state.borrow_mut();
            state.search = id;
        }
        self.status.set_text("Loading saved results…");
        self.handle().send(CoreCommand::LoadCachedCatalog {
            id,
            query: String::new(),
        });
    }

    /// Startup read of saved downloads. Local only.
    pub fn load_downloads(&self) {
        self.downloads.load();
    }

    pub fn receive(&self, event: &UiEvent) {
        self.access.receive(event);
        self.downloads.receive(event);
        match event {
            UiEvent::DownloadFinished { outcome }
                if outcome.end == workstation_runtimes::downloader::DownloadEnd::Completed =>
            {
                // The new file is now a verified local copy of what is listed.
                self.handle().scan_local();
            }
            UiEvent::CatalogLoaded { id, result } => {
                if self.state.borrow().search != *id {
                    return;
                }
                match result {
                    Ok(Some(load)) => self.show_page(load),
                    Ok(None) => self
                        .status
                        .set_text("No saved results yet. Press Search to load from Hugging Face."),
                    Err(message) => {
                        // Results on screen stay; a failed next page can be retried.
                        let retry_more = {
                            let state = self.state.borrow();
                            state.continuing && state.next_cursor.is_some()
                        };
                        self.more.set_sensitive(true);
                        self.status.set_text(&text(&format!(
                            "Could not load models: {message} {}",
                            if retry_more {
                                "Press Load more to retry."
                            } else {
                                "Press Search to retry."
                            }
                        )));
                    }
                }
            }
            UiEvent::ReleaseLoaded { id, result } => {
                let Some(repo_id) = ({
                    let mut state = self.state.borrow_mut();
                    match &state.release {
                        Some((pending, repo)) if pending == id => {
                            let repo = repo.clone();
                            state.release = None;
                            Some(repo)
                        }
                        _ => None,
                    }
                }) else {
                    return;
                };
                self.show_release(&repo_id, result);
            }
            UiEvent::ArtifactInspected { id, result } => {
                let Some(artifact_id) = ({
                    let mut state = self.state.borrow_mut();
                    match &state.inspect {
                        Some((pending, artifact)) if pending == id => {
                            let artifact = artifact.clone();
                            state.inspect = None;
                            Some(artifact)
                        }
                        _ => None,
                    }
                }) else {
                    return;
                };
                let mut state = self.state.borrow_mut();
                let Some(entry) = state.artifacts.get_mut(&artifact_id) else {
                    return;
                };
                entry.loading = false;
                let rows = match result {
                    Ok(details) => {
                        entry.loaded = true;
                        details_rows(details)
                    }
                    Err(message) => {
                        entry.loaded = false;
                        vec![info_row(
                            "Could not read the file header",
                            &format!("{message} Collapse and expand to retry."),
                        )]
                    }
                };
                set_children(&entry.row, &mut entry.children, rows);
            }
            UiEvent::LocalScanned { id, result } => {
                if self.state.borrow().scan != *id {
                    return;
                }
                match result {
                    Ok(report) => self.show_local(report),
                    Err(_) => {
                        self.note_local("local check failed; try expanding the repository again")
                    }
                }
            }
            _ => {}
        }
    }

    /// Replaces the local-copy note on every file that was checked.
    fn note_local(&self, note: &str) {
        for entry in self.state.borrow().artifacts.values() {
            if wanted_of(&entry.artifact).is_some() {
                entry
                    .row
                    .set_subtitle(&text(&format!("{} · {note}", entry.base_subtitle)));
            }
        }
    }

    /// Marks each checked file with where an identical copy already is, if anywhere.
    fn show_local(&self, report: &workstation_core::models::dedup::ScanReport) {
        let state = self.state.borrow();
        for entry in state.artifacts.values() {
            if wanted_of(&entry.artifact).is_none() {
                continue;
            }
            let id = &entry.artifact.artifact_id;
            let places: Vec<String> = report
                .matches
                .iter()
                .filter(|found| &found.artifact_id == id)
                .map(|found| {
                    format!(
                        "{} ({})",
                        clean(&found.path.to_string_lossy()),
                        root_name(found.root)
                    )
                })
                .collect();
            let note = if !places.is_empty() {
                format!("already on disk, SHA-256 verified: {}", places.join("; "))
            } else {
                let unreadable = report.skipped > 0
                    || report
                        .roots
                        .iter()
                        .any(|root| root.state == RootState::Unreadable);
                match report.stopped {
                    None if unreadable => {
                        "some folders or files could not be read; a copy may exist".to_owned()
                    }
                    None => "no identical local copy found".to_owned(),
                    Some(stop) => format!(
                        "local check stopped ({}); a copy may exist",
                        match stop {
                            Stop::Cancelled => "replaced by a newer check",
                            Stop::EntryLimit => "too many files",
                            Stop::ByteLimit => "read limit reached",
                            Stop::TimeLimit => "time limit reached",
                        }
                    ),
                }
            };
            entry
                .row
                .set_subtitle(&text(&format!("{} · {note}", entry.base_subtitle)));
        }
    }

    fn show_page(&self, load: &CatalogLoad) {
        if !self.state.borrow().continuing {
            while let Some(child) = self.list.first_child() {
                self.list.remove(&child);
            }
            let mut state = self.state.borrow_mut();
            state.rows.clear();
            state.artifacts.clear();
            state.release = None;
            state.inspect = None;
            state.scan = 0;
        }
        for repo in &load.page.repos {
            if self.state.borrow().rows.contains_key(&repo.repo_id) {
                continue;
            }
            let row = adw::ExpanderRow::builder()
                .title(text(&repo.repo_id))
                .subtitle(repo_subtitle(repo.downloads, repo.gated.as_str()))
                .use_markup(false)
                .build();
            let this = self.handle();
            let repo_id = repo.repo_id.clone();
            row.connect_expanded_notify(move |row| {
                if row.is_expanded() {
                    this.expand(&repo_id);
                }
            });
            self.list.append(&row);
            self.state.borrow_mut().rows.insert(
                repo.repo_id.clone(),
                RepoRow {
                    row,
                    children: Vec::new(),
                    loading: false,
                    loaded: false,
                },
            );
        }
        {
            let mut state = self.state.borrow_mut();
            state.query = state.pending_query.clone();
            state.next_cursor = load.page.next_cursor.clone();
        }
        self.more.set_visible(load.page.next_cursor.is_some());
        self.more.set_sensitive(true);
        let count = self.state.borrow().rows.len();
        let summary = match load.source {
            LoadSource::Live => format!("{count} repositories from Hugging Face."),
            LoadSource::Cached { fetched_at_unix_ms } => format!(
                "{count} repositories saved {}. Press Search to refresh.",
                age(fetched_at_unix_ms)
            ),
        };
        self.status.set_text(&match &load.notice {
            Some(notice) => format!("{summary} {notice}"),
            None => summary,
        });
    }

    fn show_release(&self, repo_id: &str, result: &Result<Box<ReleaseLoad>, String>) {
        let mut new_artifacts = Vec::new();
        {
            let mut state = self.state.borrow_mut();
            let Some(entry) = state.rows.get_mut(repo_id) else {
                return;
            };
            entry.loading = false;
            let rows = match result {
                Err(message) => {
                    entry.loaded = false;
                    vec![info_row(
                        "Could not load files",
                        &format!("{message} Collapse and expand to retry."),
                    )]
                }
                Ok(load) => {
                    entry.loaded = true;
                    let release = &load.release;
                    let mut rows = vec![info_row(&release_title(release), &release_subtitle(load))];
                    if release.artifacts.is_empty() {
                        rows.push(info_row(
                            "No GGUF files",
                            "This repository lists no GGUF weight files.",
                        ));
                    }
                    for artifact in &release.artifacts {
                        let row = artifact_row(artifact);
                        let this = self.handle();
                        let artifact_id = artifact.artifact_id.clone();
                        row.connect_expanded_notify(move |row| {
                            if row.is_expanded() {
                                this.inspect(&artifact_id);
                            }
                        });
                        let download = gtk::Button::builder()
                            .label("Download")
                            .valign(gtk::Align::Center)
                            .build();
                        download.update_property(&[gtk::accessible::Property::Label(&format!(
                            "Download {}",
                            clean(&artifact.filename)
                        ))]);
                        if wanted_of(artifact).is_some() {
                            download.set_tooltip_text(Some(&format!(
                                "Downloads {} and checks its SHA-256 before keeping it.",
                                artifact.size_bytes.map_or(String::new(), size_text)
                            )));
                            let (this, artifact) = (self.handle(), artifact.clone());
                            download.connect_clicked(move |_| this.download(artifact.clone()));
                        } else {
                            download.set_sensitive(false);
                            download.set_tooltip_text(Some(
                                "Needs a published size and checksum, and a single file.",
                            ));
                        }
                        row.add_suffix(&download);
                        rows.push(row.clone().upcast());
                        new_artifacts.push(ArtifactRow {
                            base_subtitle: row.subtitle().to_string(),
                            row,
                            children: Vec::new(),
                            artifact: artifact.clone(),
                            loading: false,
                            loaded: false,
                        });
                    }
                    rows
                }
            };
            set_children(&entry.row, &mut entry.children, rows);
            state
                .artifacts
                .retain(|_, row| row.artifact.repo_id != repo_id);
            for row in new_artifacts {
                state
                    .artifacts
                    .insert(row.artifact.artifact_id.clone(), row);
            }
        }
        if result.is_ok() {
            self.handle().scan_local();
        }
    }
}

/// Weak handle used by widget callbacks.
#[derive(Clone)]
struct Handle {
    entry: gtk::glib::WeakRef<gtk::SearchEntry>,
    status: gtk::glib::WeakRef<gtk::Label>,
    more: gtk::glib::WeakRef<gtk::Button>,
    state: std::rc::Weak<RefCell<State>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl Handle {
    fn search(&self, continue_page: bool) {
        let (Some(state), Some(entry), Some(status)) = (
            self.state.upgrade(),
            self.entry.upgrade(),
            self.status.upgrade(),
        ) else {
            return;
        };
        let (query, cursor) = {
            let state = state.borrow();
            if continue_page {
                (state.query.clone(), state.next_cursor.clone())
            } else {
                (entry.text().trim().to_owned(), None)
            }
        };
        if continue_page && cursor.is_none() {
            return;
        }
        let id = next_id(&state);
        {
            let mut state = state.borrow_mut();
            state.search = id;
            state.pending_query = query.clone();
            state.continuing = continue_page;
        }
        if let Some(more) = self.more.upgrade() {
            more.set_sensitive(false);
        }
        status.set_text("Searching Hugging Face…");
        self.send(CoreCommand::SearchCatalog { id, query, cursor });
    }

    fn expand(&self, repo_id: &str) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let id = next_id(&state);
        {
            let mut state = state.borrow_mut();
            let Some(entry) = state.rows.get_mut(repo_id) else {
                return;
            };
            if entry.loading || entry.loaded {
                return;
            }
            entry.loading = true;
            set_children(
                &entry.row,
                &mut entry.children,
                vec![info_row(
                    "Loading files…",
                    "Fetching the file list from Hugging Face.",
                )],
            );
            // The worker keeps one release fetch; an older one will never report.
            if let Some((_, older)) = state.release.replace((id, repo_id.to_owned()))
                && older != repo_id
                && let Some(entry) = state.rows.get_mut(&older)
            {
                entry.loading = false;
                set_children(
                    &entry.row,
                    &mut entry.children,
                    vec![info_row(
                        "Loading was replaced by another request",
                        "Collapse and expand to retry.",
                    )],
                );
            }
        }
        self.send(CoreCommand::ExpandRelease {
            id,
            repo_id: repo_id.to_owned(),
            pinned: None,
        });
    }

    /// Local-only check of every loaded file that has a published size and checksum.
    fn download(&self, artifact: ModelArtifact) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let id = next_id(&state);
        self.send(CoreCommand::StartDownload {
            id,
            artifact: Box::new(artifact),
        });
    }

    fn scan_local(&self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let wanted: Vec<Wanted> = state
            .borrow()
            .artifacts
            .values()
            .filter_map(|entry| wanted_of(&entry.artifact))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let id = next_id(&state);
        state.borrow_mut().scan = id;
        self.send(CoreCommand::ScanLocal { id, wanted });
    }

    fn inspect(&self, artifact_id: &str) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let id = next_id(&state);
        let artifact = {
            let mut state = state.borrow_mut();
            let Some(entry) = state.artifacts.get_mut(artifact_id) else {
                return;
            };
            // Expanding again re-reads the saved header and refreshes the memory budgets.
            if entry.loading {
                return;
            }
            entry.loading = true;
            set_children(
                &entry.row,
                &mut entry.children,
                vec![info_row(
                    "Reading file header…",
                    "Downloads at most the first 16 MiB of this file, once; then it is saved.",
                )],
            );
            let artifact = entry.artifact.clone();
            // The worker keeps one inspection; an older one will never report.
            if let Some((_, older)) = state.inspect.replace((id, artifact_id.to_owned()))
                && older != artifact_id
                && let Some(entry) = state.artifacts.get_mut(&older)
            {
                entry.loading = false;
                set_children(
                    &entry.row,
                    &mut entry.children,
                    vec![info_row(
                        "Reading was replaced by another request",
                        "Collapse and expand to retry.",
                    )],
                );
            }
            artifact
        };
        self.send(CoreCommand::InspectArtifact {
            id,
            artifact: Box::new(artifact),
        });
    }

    fn send(&self, command: CoreCommand) {
        let commands = self.commands.clone();
        let status = self.status.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            if commands.send(command).await.is_err()
                && let Some(status) = status.upgrade()
            {
                status.set_text("Catalog worker unavailable. Reopen the application to retry.");
            }
        });
    }
}

/// Only a file with a published size and checksum, and not split, can be matched.
fn wanted_of(artifact: &ModelArtifact) -> Option<Wanted> {
    if artifact.shards.is_some() {
        return None;
    }
    Some(Wanted {
        artifact_id: artifact.artifact_id.clone(),
        size: artifact.size_bytes.filter(|size| *size > 0)?,
        sha256: artifact.sha256.clone()?,
    })
}

fn root_name(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Ollama => "Ollama",
        RootKind::HuggingFace => "Hugging Face cache",
        RootKind::Application => "this application",
    }
}

/// File names are untrusted text: control characters become `?`.
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn next_id(state: &Rc<RefCell<State>>) -> u64 {
    let mut state = state.borrow_mut();
    state.next_id = state.next_id.wrapping_add(1);
    state.next_id
}

fn text(value: &str) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(MAX_TEXT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn info_row(title: &str, subtitle: &str) -> gtk::Widget {
    adw::ActionRow::builder()
        .title(text(title))
        .subtitle(text(subtitle))
        .use_markup(false)
        .subtitle_selectable(true)
        .build()
        .upcast()
}

fn repo_subtitle(downloads: Option<u64>, gated: &str) -> String {
    let downloads = downloads.map_or("downloads unknown".to_owned(), |n| format!("{n} downloads"));
    match gated {
        "open" => downloads,
        other => format!("{downloads} · access: {other}"),
    }
}

fn release_title(release: &ModelRelease) -> String {
    format!("Family: {}", release.family.name)
}

fn release_subtitle(load: &ReleaseLoad) -> String {
    let release = &load.release;
    let mut parts = vec![
        format!(
            "license {}",
            release.license.as_deref().unwrap_or("unknown")
        ),
        format!("access {}", release.gated.as_str()),
        format!(
            "revision {}",
            &release.revision[..release.revision.len().min(12)]
        ),
    ];
    if release.other_weight_files > 0 {
        parts.push(format!(
            "{} non-GGUF weight files not shown",
            release.other_weight_files
        ));
    }
    if release.dropped_invalid > 0 {
        parts.push(format!(
            "{} entries skipped as invalid",
            release.dropped_invalid
        ));
    }
    if let LoadSource::Cached { fetched_at_unix_ms } = load.source {
        parts.push(format!("saved {}", age(fetched_at_unix_ms)));
    }
    if let Some(notice) = &load.notice {
        parts.push(notice.clone());
    }
    parts.join(" · ")
}

fn artifact_row(artifact: &ModelArtifact) -> adw::ExpanderRow {
    let size = artifact
        .size_bytes
        .map_or("size unknown".to_owned(), size_text);
    let mut parts = vec![
        artifact
            .quantization_hint
            .clone()
            .map_or("quantization unknown".to_owned(), |q| {
                format!("{q} (from filename)")
            }),
        size,
    ];
    if let Some(shards) = artifact.shards {
        parts.push(if shards.complete {
            format!("{} shards (not downloadable yet)", shards.count)
        } else {
            format!("{} shards, some missing", shards.count)
        });
    }
    parts.push(if artifact.sha256.is_some() {
        "checksum listed".into()
    } else {
        "no checksum: not downloadable".to_owned()
    });
    adw::ExpanderRow::builder()
        .title(text(&artifact.filename))
        .subtitle(text(&parts.join(" · ")))
        .use_markup(false)
        .build()
}

/// Replace the rows under an expander.
fn set_children(row: &adw::ExpanderRow, children: &mut Vec<gtk::Widget>, new: Vec<gtk::Widget>) {
    for child in children.drain(..) {
        row.remove(&child);
    }
    for widget in new {
        row.add_row(&widget);
        children.push(widget);
    }
}

/// What the file's header declares and the conditional fit for the default scenario.
fn details_rows(details: &ArtifactDetails) -> Vec<gtk::Widget> {
    let report = &details.report;
    let facts = &report.metadata;
    let mut rows = vec![info_row(
        &format!(
            "Architecture {}",
            facts.architecture.as_deref().unwrap_or("unknown")
        ),
        &format!(
            "{} tensors · weights {} · context length {}",
            facts.tensor_count,
            size_text(facts.tensor_bytes),
            facts
                .attention
                .as_ref()
                .and_then(|a| a.context_length)
                .map_or("not declared".to_owned(), |n| n.to_string()),
        ),
    )];
    if !facts.warnings.is_empty() {
        rows.push(info_row(
            "Layout notes",
            &facts
                .warnings
                .iter()
                .map(|w| format!("{w:?}"))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let memory = report
        .estimate
        .assessment
        .breakdown
        .map_or("no total available".to_owned(), |b| {
            format!("estimated total {}", size_text(b.total_bytes))
        });
    rows.push(info_row(
        super::model_fit::memory_class_text(report.estimate.assessment.memory_class()),
        &format!(
            "{} {memory}. Scenario: 8192-token context, F16 K/V cache, microbatch 512, uniform attention assumed; this computer's RAM available now: {}.",
            super::model_fit::status_text(report.estimate.status),
            report
                .available_ram_bytes
                .map_or("unknown".to_owned(), size_text),
        ),
    ));
    for gpu in &report.gpu_estimates {
        rows.push(info_row(
            &format!(
                "{}: {}",
                gpu.name,
                super::model_fit::memory_class_text(gpu.estimate.assessment.memory_class())
            ),
            &format!(
                "VRAM available now: {}.",
                gpu.available_vram_bytes
                    .map_or("unknown".to_owned(), size_text)
            ),
        ));
    }
    rows.push(info_row(
        "Coverage",
        "Fit is estimated only for llama and qwen2 architectures without special layouts; anything else shows Unknown.",
    ));
    let source = match details.source {
        LoadSource::Live => "Read from Hugging Face just now.".to_owned(),
        LoadSource::Cached { fetched_at_unix_ms } => {
            format!("Header saved {}.", age(fetched_at_unix_ms))
        }
    };
    rows.push(info_row(
        "Source",
        &match &details.notice {
            Some(notice) => format!("{source} {notice}"),
            None => source,
        },
    ));
    rows
}

fn size_text(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn age(fetched_at_unix_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let seconds = now.saturating_sub(fetched_at_unix_ms) / 1000;
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

#[cfg(test)]
#[path = "catalog_smoke.rs"]
mod smoke;
