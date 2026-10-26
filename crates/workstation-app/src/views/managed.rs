// SPDX-License-Identifier: GPL-3.0-or-later
//! The "llama.cpp (Managed)" card on the Runtimes page: install, launch profiles, one
//! supervised server. All work goes through `CoreCommand::Managed`; nothing here blocks.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use adw::prelude::*;
use tokio::sync::mpsc::Sender;
use workstation_core::runtime::{
    RuntimeLifecycleState,
    profile::{LOOPBACK, LaunchProfile},
};
use workstation_runtimes::llamacpp::{
    install::Progress,
    release::{Backend, Candidate},
};

use super::managed_editor;
use crate::{
    bridge::{CoreCommand, UiEvent},
    managed::{Live, ManagedCommand, ManagedEvent, Snapshot, holds_process},
};

const MAX_TEXT: usize = 300;
const LOG_REFRESH_MS: u32 = 1000;
const LOAD_RETRIES: u32 = 20;

/// Which actions the current state allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Controls {
    pub start: bool,
    pub stop: bool,
    pub restart: bool,
}

pub(super) fn controls(live: Option<&Live>, has_build: bool) -> Controls {
    use RuntimeLifecycleState as S;
    let Some(live) = live else {
        return Controls {
            start: has_build,
            stop: false,
            restart: false,
        };
    };
    let state = live.state();
    // A child that a failed stop could not reap is still ours: offer Stop again, never Start.
    let stuck = holds_process(&live.status) && !state.is_live();
    Controls {
        start: has_build && matches!(state, S::Stopped | S::Failed) && !stuck,
        stop: matches!(state, S::Starting | S::Ready | S::Busy) || stuck,
        restart: matches!(state, S::Ready | S::Busy | S::Failed) && !stuck,
    }
}

/// Title and detail for the server row. Nothing is claimed that was not reported.
pub(super) fn server_text(live: Option<&Live>) -> (String, String) {
    let Some(live) = live else {
        return ("Stopped".into(), "No managed server is running.".into());
    };
    let state = live.state();
    let mut detail = Vec::new();
    if let Some(port) = live.status.port.filter(|_| state.is_live()) {
        detail.push(format!("http://{LOOPBACK}:{port}/v1"));
    }
    if let Some(ctx) = live.props.as_ref().and_then(|p| p.n_ctx) {
        detail.push(format!("{ctx} tokens context"));
    }
    if live.status.restarts > 0 {
        detail.push(format!("restarted {} times", live.status.restarts));
    }
    if let Some(failure) = &live.status.failure {
        detail.push(failure.kind.describe().to_owned());
        if let Some(code) = failure.exit_code {
            detail.push(format!("exit code {code}"));
        }
    }
    (state.label().to_owned(), detail.join(" · "))
}

pub(super) fn candidate_text(candidate: &Candidate) -> (String, String) {
    let size = candidate.total_size() as f64 / (1024.0 * 1024.0);
    let backend = match candidate.backend {
        Backend::Cpu => "CPU",
        Backend::Vulkan => "Vulkan (GPU)",
        Backend::Cuda => "CUDA (NVIDIA GPU)",
    };
    (
        format!("{} · {backend}", candidate.tag),
        format!("{size:.0} MiB download, SHA-256 checked"),
    )
}

pub(super) fn profile_text(profile: &LaunchProfile) -> String {
    format!(
        "Port {} · {} tokens context · {} GPU layers",
        profile.port, profile.context_length, profile.gpu_layers
    )
}

pub(super) fn progress_fraction(progress: &Progress) -> f64 {
    if progress.total == 0 {
        return 0.0;
    }
    (progress.done as f64 / progress.total as f64).clamp(0.0, 1.0)
}

/// Buttons that wrap onto further lines instead of widening the page.
fn suffix_buttons(row: &adw::ActionRow, buttons: &[&gtk::Button]) {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(1)
        .max_children_per_line(3)
        .column_spacing(6)
        .row_spacing(6)
        .valign(gtk::Align::Center)
        .build();
    for button in buttons {
        flow.append(*button);
    }
    row.add_suffix(&flow);
}

fn clip(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(MAX_TEXT)
        .collect()
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// Highest snapshot revision shown; an older one arriving later is dropped.
    applied: u64,
    load: Option<u64>,
    builds_request: Option<u64>,
    install_request: Option<u64>,
    action: Option<u64>,
    logs_request: Option<u64>,
    snapshot: Option<Snapshot>,
    live: Option<Live>,
    builds: Option<Vec<Candidate>>,
    progress: Option<Progress>,
    notice: Option<String>,
    /// Directory name of the build to run profiles with.
    selected_build: Option<String>,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn holds_process(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|live| holds_process(&live.status))
    }

    fn build(&self) -> Option<&str> {
        let installs = &self.snapshot.as_ref()?.installs;
        self.selected_build
            .as_deref()
            .filter(|name| installs.iter().any(|r| r.dir_name == *name))
            .or_else(|| installs.last().map(|r| r.dir_name.as_str()))
    }
}

struct Inner {
    commands: Sender<CoreCommand>,
    state: RefCell<State>,
    notice: gtk::Label,
    server: adw::PreferencesGroup,
    builds: adw::PreferencesGroup,
    profiles: adw::PreferencesGroup,
    log_view: RefCell<Option<LogWindow>>,
    /// Rows currently added to the server, builds and profiles groups.
    rows: RefCell<[Vec<gtk::Widget>; 3]>,
}

struct LogWindow {
    window: gtk::Window,
    text: gtk::TextBuffer,
}

pub struct ManagedCard {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
}

impl ManagedCard {
    pub fn new(commands: Sender<CoreCommand>) -> Self {
        let title = gtk::Label::builder()
            .label("llama.cpp (Managed)")
            .xalign(0.0)
            .css_classes(["title-2"])
            .build();
        let intro = gtk::Label::builder()
            .label("The official portable build, run as your user and reachable only from this computer. Closing the application stops it.")
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let notice = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .visible(false)
            .css_classes(["error"])
            .selectable(true)
            .build();
        let server = adw::PreferencesGroup::builder().title("Server").build();
        let builds = adw::PreferencesGroup::builder()
            .title("Installed builds")
            .build();
        let profiles = adw::PreferencesGroup::builder()
            .title("Launch profiles")
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        for widget in [
            title.upcast_ref::<gtk::Widget>(),
            intro.upcast_ref(),
            notice.upcast_ref(),
            server.upcast_ref(),
            builds.upcast_ref(),
            profiles.upcast_ref(),
        ] {
            root.append(widget);
        }
        let inner = Rc::new(Inner {
            commands,
            state: RefCell::default(),
            notice,
            server,
            builds,
            profiles,
            log_view: RefCell::new(None),
            rows: RefCell::default(),
        });
        render(&inner);
        Self {
            widget: root,
            inner,
        }
    }

    /// Reads saved state and starts supervision (local files only; no network).
    pub fn load(&self) {
        try_load(&self.inner, 0);
    }

    pub fn receive(&self, event: &UiEvent) {
        let UiEvent::Managed(event) = event else {
            return;
        };
        let inner = &self.inner;
        {
            let mut state = inner.state.borrow_mut();
            match event {
                ManagedEvent::Snapshot { id, result } => {
                    if state.load == Some(*id) {
                        state.load = None;
                    }
                    match result {
                        Ok(snapshot) if snapshot.revision > state.applied => {
                            state.applied = snapshot.revision;
                            state.notice = snapshot.supervisor_error.clone();
                            state.snapshot = Some((**snapshot).clone());
                        }
                        Ok(_) => {}
                        Err(message) => state.notice = Some(message.clone()),
                    }
                }
                ManagedEvent::Builds { id, result } if state.builds_request == Some(*id) => {
                    state.builds_request = None;
                    match result {
                        Ok(list) => {
                            state.notice = list.is_empty().then(|| {
                                "No verified build is available for this computer.".to_owned()
                            });
                            state.builds = Some(list.clone());
                        }
                        Err(message) => state.notice = Some(message.clone()),
                    }
                }
                ManagedEvent::InstallProgress(progress) if state.install_request.is_some() => {
                    state.progress = Some(*progress);
                }
                ManagedEvent::Installed { id, result } if state.install_request == Some(*id) => {
                    state.install_request = None;
                    state.progress = None;
                    match result {
                        Ok(snapshot) => {
                            state.notice = None;
                            state.builds = None;
                            if snapshot.revision > state.applied {
                                state.applied = snapshot.revision;
                                state.snapshot = Some((**snapshot).clone());
                            }
                        }
                        Err(message) => state.notice = Some(message.clone()),
                    }
                }
                ManagedEvent::Action { id, result } if state.action == Some(*id) => {
                    state.action = None;
                    state.notice = result.as_ref().err().cloned();
                }
                ManagedEvent::Live(live) => state.live = Some((**live).clone()),
                ManagedEvent::Logs { id, lines } if state.logs_request == Some(*id) => {
                    state.logs_request = None;
                    if let Some(view) = inner.log_view.borrow().as_ref() {
                        view.text.set_text(&clip_lines(lines));
                    }
                }
                _ => {}
            }
        }
        render(inner);
    }
}

/// The command queue holds one request, and the page opens while other startup work is
/// queued, so a refused Load is retried shortly instead of being lost.
fn try_load(inner: &Rc<Inner>, attempt: u32) {
    let id = inner.state.borrow_mut().id();
    if inner
        .commands
        .try_send(CoreCommand::Managed(ManagedCommand::Load { id }))
        .is_ok()
    {
        inner.state.borrow_mut().load = Some(id);
        return;
    }
    if attempt >= LOAD_RETRIES {
        inner.state.borrow_mut().notice =
            Some("The runtime state could not be loaded. Restart the application.".into());
        render(inner);
        return;
    }
    let weak = Rc::downgrade(inner);
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
        if let Some(inner) = weak.upgrade() {
            try_load(&inner, attempt + 1);
        }
    });
}

fn clip_lines(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| clip(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sends one command; a full or closed channel is shown, never silently dropped.
fn send(inner: &Rc<Inner>, command: ManagedCommand) -> bool {
    match inner.commands.try_send(CoreCommand::Managed(command)) {
        Ok(()) => true,
        Err(_) => {
            inner.state.borrow_mut().notice =
                Some("The application is busy or closing. Try again.".into());
            render(inner);
            false
        }
    }
}

/// Rebuilds the three groups from state; lists are short and bounded by the database caps.
fn render(inner: &Rc<Inner>) {
    let groups = [&inner.server, &inner.builds, &inner.profiles];
    for (group, rows) in groups.iter().zip(inner.rows.borrow_mut().iter_mut()) {
        for row in rows.drain(..) {
            group.remove(&row);
        }
    }
    let mut server_rows = Vec::new();
    let mut build_rows = Vec::new();
    let mut profile_rows = Vec::new();

    let state = inner.state.borrow();
    match &state.notice {
        Some(text) => {
            inner.notice.set_text(&clip(text));
            inner.notice.set_visible(true);
        }
        None => inner.notice.set_visible(false),
    }

    // Server row.
    let (title, detail) = server_text(state.live.as_ref());
    let row = adw::ActionRow::builder()
        .title(&title)
        .subtitle(clip(&detail))
        .use_markup(false)
        .subtitle_lines(0)
        .build();
    let allowed = controls(state.live.as_ref(), state.build().is_some());
    let busy = state.action.is_some();
    let mut server_buttons = Vec::new();
    for (label, on, command) in [
        ("_Stop", allowed.stop, 0u8),
        ("_Restart", allowed.restart, 1),
        ("_Logs", true, 2),
    ] {
        let button = gtk::Button::with_mnemonic(label);
        button.set_valign(gtk::Align::Center);
        button.set_sensitive(on && (command == 2 || !busy));
        let weak = Rc::downgrade(inner);
        button.connect_clicked(move |button| {
            if let Some(inner) = weak.upgrade() {
                match command {
                    0 => action(&inner, |id| ManagedCommand::Stop { id }),
                    1 => action(&inner, |id| ManagedCommand::Restart { id }),
                    _ => open_logs(&inner, button),
                }
            }
        });
        server_buttons.push(button);
    }
    suffix_buttons(&row, &server_buttons.iter().collect::<Vec<_>>());
    inner.server.add(&row);
    server_rows.push(row.upcast());

    // Builds group.
    let installs = state
        .snapshot
        .as_ref()
        .map(|s| s.installs.clone())
        .unwrap_or_default();
    let installing = state.install_request.is_some();
    for install in &installs {
        let row = adw::ActionRow::builder()
            .title(format!(
                "{} · {}",
                clip(&install.version),
                clip(&install.backend)
            ))
            .subtitle(clip(&install.dir_name))
            .use_markup(false)
            .build();
        let selected = state.build() == Some(install.dir_name.as_str());
        let pick = gtk::CheckButton::new();
        pick.set_active(selected);
        pick.set_tooltip_text(Some("Run profiles with this build"));
        pick.set_valign(gtk::Align::Center);
        let weak = Rc::downgrade(inner);
        let dir = install.dir_name.clone();
        pick.connect_toggled(move |button| {
            if let (true, Some(inner)) = (button.is_active(), weak.upgrade()) {
                inner.state.borrow_mut().selected_build = Some(dir.clone());
                render_later(&inner);
            }
        });
        row.add_prefix(&pick);
        let remove = gtk::Button::with_mnemonic("Re_move");
        remove.set_valign(gtk::Align::Center);
        remove.set_sensitive(!installing && !state.holds_process() && state.action.is_none());
        remove.add_css_class("destructive-action");
        let weak = Rc::downgrade(inner);
        let dir = install.dir_name.clone();
        remove.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let id = inner.state.borrow_mut().id();
                send(
                    &inner,
                    ManagedCommand::RemoveInstall {
                        id,
                        dir_name: dir.clone(),
                    },
                );
            }
        });
        row.add_suffix(&remove);
        inner.builds.add(&row);
        build_rows.push(row.upcast());
    }
    if installs.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No runtime installed")
            .subtitle("Find a verified build to download. Nothing is fetched until you choose one.")
            .build();
        inner.builds.add(&row);
        build_rows.push(row.upcast());
    }
    let backends = state
        .snapshot
        .as_ref()
        .map(|s| s.backends.clone())
        .unwrap_or_default();
    let find = adw::ActionRow::builder()
        .title("Find builds")
        .subtitle("Lists the newest official llama.cpp release for this computer")
        .build();
    let button = gtk::Button::with_mnemonic("_Find");
    button.set_valign(gtk::Align::Center);
    button.set_sensitive(state.builds_request.is_none() && !installing && state.snapshot.is_some());
    let weak = Rc::downgrade(inner);
    button.connect_clicked(move |_| {
        if let Some(inner) = weak.upgrade() {
            let id = inner.state.borrow_mut().id();
            if send(
                &inner,
                ManagedCommand::Builds {
                    id,
                    backends: backends.clone(),
                },
            ) {
                inner.state.borrow_mut().builds_request = Some(id);
                render_later(&inner);
            }
        }
    });
    find.add_suffix(&button);
    inner.builds.add(&find);
    build_rows.push(find.upcast());
    for candidate in state.builds.iter().flatten() {
        let (title, subtitle) = candidate_text(candidate);
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .use_markup(false)
            .build();
        let button = gtk::Button::with_mnemonic("_Install");
        button.set_valign(gtk::Align::Center);
        button.set_sensitive(!installing);
        let (weak, candidate) = (Rc::downgrade(inner), candidate.clone());
        button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let id = inner.state.borrow_mut().id();
                if send(
                    &inner,
                    ManagedCommand::Install {
                        id,
                        candidate: Box::new(candidate.clone()),
                    },
                ) {
                    let mut state = inner.state.borrow_mut();
                    state.install_request = Some(id);
                    state.progress = None;
                    state.notice = None;
                    drop(state);
                    render_later(&inner);
                }
            }
        });
        row.add_suffix(&button);
        inner.builds.add(&row);
        build_rows.push(row.upcast());
    }
    if installing {
        let bar = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .hexpand(true)
            .show_text(true)
            .build();
        match &state.progress {
            Some(p) => {
                bar.set_fraction(progress_fraction(p));
                bar.set_text(Some(&format!(
                    "Archive {} of {}",
                    p.archive + 1,
                    p.archives
                )));
            }
            None => bar.set_text(Some("Starting")),
        }
        let row = adw::ActionRow::builder().title("Installing").build();
        row.add_suffix(&bar);
        let cancel = gtk::Button::with_mnemonic("_Cancel");
        cancel.set_valign(gtk::Align::Center);
        let weak = Rc::downgrade(inner);
        cancel.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                send(&inner, ManagedCommand::CancelInstall);
            }
        });
        row.add_suffix(&cancel);
        inner.builds.add(&row);
        build_rows.push(row.upcast());
    }

    // Profiles group.
    let profiles = state
        .snapshot
        .as_ref()
        .map(|s| s.profiles.clone())
        .unwrap_or_default();
    let build = state.build().map(str::to_owned);
    for profile in &profiles {
        let row = adw::ActionRow::builder()
            .title(clip(&profile.name))
            .subtitle(profile_text(profile))
            .use_markup(false)
            .build();
        let start = gtk::Button::with_mnemonic("_Start");
        start.set_valign(gtk::Align::Center);
        start.add_css_class("suggested-action");
        start.set_sensitive(allowed.start && !busy);
        let (weak, name, dir) = (Rc::downgrade(inner), profile.name.clone(), build.clone());
        start.connect_clicked(move |_| {
            if let (Some(inner), Some(dir_name)) = (weak.upgrade(), dir.clone()) {
                let profile = name.clone();
                action(&inner, move |id| ManagedCommand::Start {
                    id,
                    profile: profile.clone(),
                    dir_name: dir_name.clone(),
                });
            }
        });
        let edit = gtk::Button::with_mnemonic("_Edit");
        edit.set_valign(gtk::Align::Center);
        let (weak, existing) = (Rc::downgrade(inner), profile.clone());
        edit.connect_clicked(move |button| {
            if let Some(inner) = weak.upgrade() {
                open_editor(&inner, button, Some(existing.clone()));
            }
        });
        let delete = gtk::Button::with_mnemonic("De_lete");
        delete.set_valign(gtk::Align::Center);
        let (weak, name) = (Rc::downgrade(inner), profile.name.clone());
        delete.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let id = inner.state.borrow_mut().id();
                send(
                    &inner,
                    ManagedCommand::DeleteProfile {
                        id,
                        name: name.clone(),
                    },
                );
            }
        });
        suffix_buttons(&row, &[&start, &edit, &delete]);
        inner.profiles.add(&row);
        profile_rows.push(row.upcast());
    }
    let new = adw::ActionRow::builder()
        .title("New profile")
        .subtitle("Model file, port, context length and GPU layers")
        .build();
    let button = gtk::Button::with_mnemonic("_New");
    button.set_valign(gtk::Align::Center);
    let weak = Rc::downgrade(inner);
    button.connect_clicked(move |button| {
        if let Some(inner) = weak.upgrade() {
            open_editor(&inner, button, None);
        }
    });
    new.add_suffix(&button);
    inner.profiles.add(&new);
    profile_rows.push(new.upcast());
    *inner.rows.borrow_mut() = [server_rows, build_rows, profile_rows];
}

/// Re-render after the current handler has released its borrows.
fn render_later(inner: &Rc<Inner>) {
    let weak: Weak<Inner> = Rc::downgrade(inner);
    gtk::glib::idle_add_local_once(move || {
        if let Some(inner) = weak.upgrade() {
            render(&inner);
        }
    });
}

fn action(inner: &Rc<Inner>, make: impl Fn(u64) -> ManagedCommand) {
    let id = inner.state.borrow_mut().id();
    if send(inner, make(id)) {
        inner.state.borrow_mut().action = Some(id);
        render_later(inner);
    }
}

fn open_editor(inner: &Rc<Inner>, from: &gtk::Button, existing: Option<LaunchProfile>) {
    let parent = from
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    let weak = Rc::downgrade(inner);
    managed_editor::open(parent.as_ref(), existing, move |profile| {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let id = inner.state.borrow_mut().id();
        send(
            &inner,
            ManagedCommand::SaveProfile {
                id,
                profile: Box::new(profile),
            },
        );
    });
}

fn open_logs(inner: &Rc<Inner>, from: &gtk::Button) {
    if let Some(view) = inner.log_view.borrow().as_ref() {
        view.window.present();
        return;
    }
    let text = gtk::TextBuffer::new(None);
    let view = gtk::TextView::builder()
        .buffer(&text)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .hexpand(true)
        .build();
    let window = gtk::Window::builder()
        .title("llama.cpp log")
        .default_width(720)
        .default_height(420)
        .child(&scroll)
        .build();
    if let Some(parent) = from
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    {
        window.set_transient_for(Some(&parent));
    }
    let weak = Rc::downgrade(inner);
    // Ask for new lines once a second, never while an answer is still pending.
    let source = gtk::glib::timeout_add_local(
        std::time::Duration::from_millis(u64::from(LOG_REFRESH_MS)),
        move || {
            let Some(inner) = weak.upgrade() else {
                return gtk::glib::ControlFlow::Break;
            };
            if inner.log_view.borrow().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            if inner.state.borrow().logs_request.is_none() {
                let id = inner.state.borrow_mut().id();
                if inner
                    .commands
                    .try_send(CoreCommand::Managed(ManagedCommand::Logs { id }))
                    .is_ok()
                {
                    inner.state.borrow_mut().logs_request = Some(id);
                }
            }
            gtk::glib::ControlFlow::Continue
        },
    );
    let weak = Rc::downgrade(inner);
    let source = RefCell::new(Some(source));
    window.connect_close_request(move |_| {
        if let Some(source) = source.borrow_mut().take() {
            source.remove();
        }
        if let Some(inner) = weak.upgrade() {
            inner.log_view.borrow_mut().take();
            inner.state.borrow_mut().logs_request = None;
        }
        gtk::glib::Propagation::Proceed
    });
    window.present();
    *inner.log_view.borrow_mut() = Some(LogWindow { window, text });
}

#[cfg(test)]
mod tests {
    use super::*;
    use workstation_runtimes::{
        llamacpp::release::Asset,
        supervisor::{Failure, FailureKind, Status},
    };

    fn live(state: RuntimeLifecycleState) -> Live {
        Live {
            status: Status {
                state,
                generation: Some("abcdef01".into()),
                pid: Some(10),
                port: Some(8081),
                failure: None,
                restarts: 0,
            },
            busy: None,
            props: None,
        }
    }

    #[test]
    fn controls_follow_the_lifecycle() {
        use RuntimeLifecycleState as S;
        let c = |state, build| controls(Some(&live(state)), build);
        assert_eq!(
            c(S::Stopped, true),
            Controls {
                start: true,
                stop: false,
                restart: false
            }
        );
        assert!(!c(S::Stopped, false).start);
        assert!(c(S::Starting, true).stop && !c(S::Starting, true).start);
        assert!(c(S::Ready, true).restart && c(S::Ready, true).stop);
        assert!(c(S::Failed, true).start && c(S::Failed, true).restart);
        // While stopping nothing else is allowed.
        assert_eq!(
            c(S::Stopping, true),
            Controls {
                start: false,
                stop: false,
                restart: false
            }
        );
        // Before the supervisor has reported anything, a build is enough to start.
        assert!(controls(None, true).start && !controls(None, false).start);
    }

    #[test]
    fn a_child_kept_after_a_failed_stop_can_only_be_stopped_again() {
        let mut stuck = live(RuntimeLifecycleState::Failed);
        stuck.status.failure = Some(Failure {
            kind: FailureKind::StuckOnStop,
            exit_code: None,
            last_lines: vec![],
        });
        assert_eq!(
            controls(Some(&stuck), true),
            Controls {
                start: false,
                stop: true,
                restart: false
            }
        );
        assert!(holds_process(&stuck.status));
    }

    #[test]
    fn server_text_reports_only_what_is_known() {
        assert_eq!(server_text(None).0, "Stopped");
        let ready = live(RuntimeLifecycleState::Ready);
        let (title, detail) = server_text(Some(&ready));
        assert_eq!(title, "Ready");
        assert!(detail.contains("http://127.0.0.1:8081/v1"));
        assert!(!detail.contains("context"));
        let mut failed = live(RuntimeLifecycleState::Failed);
        failed.status.failure = Some(Failure {
            kind: FailureKind::PortInUse,
            exit_code: Some(1),
            last_lines: vec![],
        });
        let (title, detail) = server_text(Some(&failed));
        assert_eq!(title, "Failed");
        assert!(detail.contains("already in use") && detail.contains("exit code 1"));
        assert!(
            !detail.contains("127.0.0.1"),
            "a dead server has no endpoint"
        );
    }

    #[test]
    fn a_ready_server_reporting_work_shows_as_busy() {
        let mut busy = live(RuntimeLifecycleState::Ready);
        busy.busy = Some(true);
        assert_eq!(server_text(Some(&busy)).0, "Busy");
        busy.busy = None;
        assert_eq!(server_text(Some(&busy)).0, "Ready");
    }

    #[test]
    fn candidates_and_progress_are_described_plainly() {
        let candidate = Candidate {
            tag: "b11905".into(),
            backend: Backend::Cuda,
            archives: vec![Asset {
                name: "a".into(),
                size: 100 * 1024 * 1024,
                sha256: "0".repeat(64),
            }],
        };
        let (title, subtitle) = candidate_text(&candidate);
        assert!(title.contains("b11905") && title.contains("CUDA"));
        assert!(subtitle.contains("100 MiB"));
        let p = Progress {
            archive: 0,
            archives: 2,
            done: 50,
            total: 100,
        };
        assert_eq!(progress_fraction(&p), 0.5);
        assert_eq!(progress_fraction(&Progress { total: 0, ..p }), 0.0);
        assert_eq!(progress_fraction(&Progress { done: 500, ..p }), 1.0);
    }

    #[test]
    fn displayed_text_loses_control_characters_and_is_bounded() {
        assert_eq!(clip("a\u{1b}[31mb\nc"), "a[31mbc");
        assert_eq!(clip(&"x".repeat(1000)).len(), MAX_TEXT);
    }
}

#[cfg(test)]
#[path = "managed_smoke.rs"]
mod managed_smoke;
