// SPDX-License-Identifier: GPL-3.0-or-later
//! Hugging Face access: add, load, check and remove the access token.
//! The desktop keyring is contacted only when a button is pressed, never at startup.

use crate::bridge::{CoreCommand, TokenOutcome, UiEvent};
use adw::prelude::*;
use std::{cell::RefCell, rc::Rc};
use tokio::sync::mpsc;
use workstation_runtimes::secrets::Token;

const MAX_TEXT: usize = 256;
const IDLE: &str = "No token in use. Needed only for gated or private repositories.";

#[derive(Default)]
struct State {
    next_id: u64,
    /// Newest token request; older answers are dropped.
    pending: Option<u64>,
}

pub struct Access {
    pub widget: gtk::ListBox,
    row: adw::ExpanderRow,
    entry: adw::PasswordEntryRow,
    buttons: Vec<gtk::Button>,
    state: Rc<RefCell<State>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl Access {
    pub fn new(commands: mpsc::Sender<CoreCommand>) -> Self {
        let row = adw::ExpanderRow::builder()
            .title("Hugging Face access")
            .subtitle(IDLE)
            .use_markup(false)
            .build();
        let entry = adw::PasswordEntryRow::builder()
            .title("Access token")
            .build();
        row.add_row(&entry);
        row.add_row(
            &adw::ActionRow::builder()
                .title("Where the token goes")
                .subtitle("Saved in your desktop keyring, never in a file. It is sent only to huggingface.co when fetching model files, and never to download links it redirects to. Accept a gated model's terms on huggingface.co yourself; this application never does.")
                .use_markup(false)
                .build(),
        );
        // A flow box wraps the four buttons so the row fits the narrowest window.
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .column_spacing(6)
            .row_spacing(6)
            .valign(gtk::Align::Center)
            .build();
        let buttons_row = adw::ActionRow::builder().title("Actions").build();
        buttons_row.add_suffix(&flow);
        let buttons: Vec<gtk::Button> = ["Save", "Use saved", "Check", "Remove"]
            .into_iter()
            .map(|label| {
                let button = gtk::Button::builder()
                    .label(label)
                    .valign(gtk::Align::Center)
                    .build();
                button.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{label} Hugging Face token"
                ))]);
                flow.append(&button);
                // The button is the tab stop; its wrapper must not be a second one.
                if let Some(wrapper) = flow.last_child() {
                    wrapper.set_focusable(false);
                }
                button
            })
            .collect();
        row.add_row(&buttons_row);
        let widget = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        widget.append(&row);
        let access = Self {
            widget,
            row,
            entry,
            buttons,
            state: Rc::default(),
            commands,
        };
        access.connect();
        access
    }

    fn connect(&self) {
        let (load, check, clear) = (self.handle(), self.handle(), self.handle());
        self.buttons[1].connect_clicked(move |_| load.send(|id| CoreCommand::LoadToken { id }));
        self.buttons[2].connect_clicked(move |_| check.send(|id| CoreCommand::CheckToken { id }));
        self.buttons[3].connect_clicked(move |_| clear.send(|id| CoreCommand::ClearToken { id }));
        let this = self.handle();
        self.buttons[0].connect_clicked(move |_| this.save());
    }

    fn handle(&self) -> Handle {
        Handle {
            row: self.row.downgrade(),
            entry: self.entry.downgrade(),
            buttons: self.buttons.iter().map(|b| b.downgrade()).collect(),
            state: Rc::downgrade(&self.state),
            commands: self.commands.clone(),
        }
    }

    pub fn receive(&self, event: &UiEvent) {
        let UiEvent::TokenDone { id, result } = event else {
            return;
        };
        {
            let mut state = self.state.borrow_mut();
            if state.pending != Some(*id) {
                return;
            }
            state.pending = None;
        }
        for button in &self.buttons {
            button.set_sensitive(true);
        }
        let subtitle = match result {
            Ok(TokenOutcome::Saved) => "Saved to the keyring and in use.".to_owned(),
            Ok(TokenOutcome::Loaded { found: true }) => {
                "Saved token loaded and in use for this session.".to_owned()
            }
            Ok(TokenOutcome::Loaded { found: false }) => {
                "No saved token in the keyring. Paste one and press Save.".to_owned()
            }
            Ok(TokenOutcome::Cleared) => {
                "Token removed from this session and the keyring.".to_owned()
            }
            Ok(TokenOutcome::Valid { name }) => {
                format!("Hugging Face accepted the token for {name}.")
            }
            Err(message) => message.clone(),
        };
        self.row.set_subtitle(&text(&subtitle));
    }
}

#[derive(Clone)]
struct Handle {
    row: gtk::glib::WeakRef<adw::ExpanderRow>,
    entry: gtk::glib::WeakRef<adw::PasswordEntryRow>,
    buttons: Vec<gtk::glib::WeakRef<gtk::Button>>,
    state: std::rc::Weak<RefCell<State>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl Handle {
    fn save(&self) {
        let Some(entry) = self.entry.upgrade() else {
            return;
        };
        let parsed = Token::parse(entry.text().as_str());
        // The pasted text leaves the box either way; it is never echoed in a message.
        entry.set_text("");
        match parsed {
            Ok(token) => self.send(move |id| CoreCommand::SaveToken { id, token }),
            Err(error) => {
                if let Some(row) = self.row.upgrade() {
                    row.set_subtitle(&error.to_string());
                }
            }
        }
    }

    fn send(&self, command: impl FnOnce(u64) -> CoreCommand + 'static) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let id = {
            let mut state = state.borrow_mut();
            if state.pending.is_some() {
                return;
            }
            state.next_id += 1;
            state.pending = Some(state.next_id);
            state.next_id
        };
        for button in self.buttons.iter().filter_map(|b| b.upgrade()) {
            button.set_sensitive(false);
        }
        if let Some(row) = self.row.upgrade() {
            row.set_subtitle("Waiting for the keyring… answer any prompt it shows.");
        }
        let (commands, state, buttons, row) = (
            self.commands.clone(),
            Rc::downgrade(&state),
            self.buttons.clone(),
            self.row.clone(),
        );
        let command = command(id);
        gtk::glib::MainContext::default().spawn_local(async move {
            if commands.send(command).await.is_err() {
                if let Some(state) = state.upgrade() {
                    state.borrow_mut().pending = None;
                }
                for button in buttons.iter().filter_map(|b| b.upgrade()) {
                    button.set_sensitive(true);
                }
                if let Some(row) = row.upgrade() {
                    row.set_subtitle("Token worker unavailable. Reopen the application to retry.");
                }
            }
        });
    }
}

fn text(value: &str) -> String {
    let clean: String = value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    let mut chars = clean.chars();
    let head: String = chars.by_ref().take(MAX_TEXT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
#[path = "access_smoke.rs"]
mod smoke;
