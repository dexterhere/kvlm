// SPDX-License-Identifier: GPL-3.0-or-later
//! "Other runtimes" on the Runtimes page: LM Studio and vLLM that the user runs themselves.
//! Each card has a port, one Check button and a read-only result. Nothing is checked until the
//! button is pressed, and nothing here can start, stop or change those runtimes.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use tokio::sync::mpsc;
use workstation_runtimes::{
    lmstudio::{self, ModelState},
    vllm,
};

use crate::attached::{AttachedCommand, AttachedEvent, Engine, Found};

const NOTICE: &str = "Read-only: these runtimes belong to you and are never started, stopped or changed here. Requests to them are not benchmarked, because local execution cannot be proven for a runtime this application does not own.";

struct Card {
    engine: Engine,
    port: gtk::SpinButton,
    check: gtk::Button,
    result: gtk::Label,
    pending: RefCell<Option<u64>>,
}

pub struct Attached {
    pub widget: gtk::Box,
    cards: Vec<Rc<Card>>,
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(2000)
        .collect()
}

/// What an LM Studio check found, in plain words.
pub(super) fn lmstudio_text(found: &Result<lmstudio::Discovery, String>) -> String {
    let discovery = match found {
        Ok(discovery) => discovery,
        Err(message) => return message.clone(),
    };
    if discovery.models.is_empty() {
        return "LM Studio answered, but lists no models.".into();
    }
    let mut lines = vec![format!(
        "{} model{}{}",
        discovery.models.len(),
        if discovery.models.len() == 1 { "" } else { "s" },
        if discovery.source == lmstudio::Source::Compatible {
            " (names only: this LM Studio does not report load state)"
        } else {
            ""
        }
    )];
    for model in &discovery.models {
        let state = match model.state {
            ModelState::Loaded => "loaded",
            ModelState::NotLoaded => "not loaded",
            ModelState::Unknown => "load state unknown",
        };
        let mut facts = vec![state.to_string()];
        facts.extend(model.quantization.clone());
        if let Some(ctx) = model.loaded_context_length {
            facts.push(format!("{ctx} tokens loaded context"));
        } else if let Some(max) = model.max_context_length {
            facts.push(format!("up to {max} tokens context"));
        }
        lines.push(format!(
            "\u{2022} {} \u{b7} {}",
            model.id,
            facts.join(" \u{b7} ")
        ));
    }
    if discovery.truncated {
        lines.push(format!(
            "Only the first {} are shown.",
            lmstudio::MAX_MODELS
        ));
    }
    lines.join("\n")
}

fn number(value: Option<f64>, unit: &str) -> String {
    value.map_or_else(|| "not reported".into(), |v| format!("{v:.0}{unit}"))
}

/// What a vLLM check found; a part that failed says why instead of reading as zero.
pub(super) fn vllm_text(found: &vllm::Discovery) -> String {
    let mut lines = vec![match &found.healthy {
        Ok(()) => "Health: ok".to_string(),
        Err(error) => format!("Health: {error}"),
    }];
    match &found.models {
        Ok(models) if models.is_empty() => lines.push("Models: none listed".into()),
        Ok(models) => {
            for model in models {
                lines.push(format!(
                    "\u{2022} {}{}",
                    model.id,
                    model
                        .max_model_len
                        .map(|n| format!(" \u{b7} up to {n} tokens"))
                        .unwrap_or_default()
                ));
            }
            if found.truncated {
                lines.push(format!("Only the first {} are shown.", vllm::MAX_MODELS));
            }
        }
        Err(error) => lines.push(format!("Models: {error}")),
    }
    lines.push(match &found.metrics {
        Ok(m) => format!(
            "Requests: {} running, {} waiting \u{b7} KV cache in use: {}",
            number(m.requests_running, ""),
            number(m.requests_waiting, ""),
            m.kv_cache_usage
                .map_or_else(|| "not reported".into(), |v| format!("{:.0}%", v * 100.0)),
        ),
        Err(error) => format!("Metrics: {error}"),
    });
    lines.join("\n")
}

impl Attached {
    pub fn new(commands: mpsc::Sender<AttachedCommand>) -> Self {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        widget.append(
            &gtk::Label::builder()
                .label("Other runtimes (attached)")
                .xalign(0.0)
                .css_classes(["title-2"])
                .build(),
        );
        widget.append(
            &gtk::Label::builder()
                .label(NOTICE)
                .wrap(true)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build(),
        );
        let next_id = Rc::new(RefCell::new(0u64));
        let mut cards = Vec::new();
        for (engine, name) in [(Engine::LmStudio, "LM Studio"), (Engine::Vllm, "vLLM")] {
            let port = gtk::SpinButton::with_range(1.0, 65535.0, 1.0);
            port.set_value(f64::from(engine.default_port()));
            port.set_tooltip_text(Some("Loopback port the runtime listens on"));
            let check = gtk::Button::with_label("Check");
            check.set_tooltip_text(Some(&format!("Ask {name} on this machine what it serves")));
            let result = gtk::Label::builder()
                .label("Not checked yet.")
                .wrap(true)
                .xalign(0.0)
                .selectable(true)
                .build();
            let title = gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .css_classes(["heading"])
                .build();
            let port_label = gtk::Label::new(Some("Port"));
            port_label.set_mnemonic_widget(Some(&port));
            let row = gtk::Box::builder().spacing(8).build();
            row.append(&port_label);
            row.append(&port);
            row.append(&check);
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(8)
                .css_classes(["card"])
                .margin_top(4)
                .build();
            let inner = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(8)
                .margin_top(12)
                .margin_bottom(12)
                .margin_start(12)
                .margin_end(12)
                .build();
            inner.append(&title);
            inner.append(&row);
            inner.append(&result);
            card.append(&inner);
            widget.append(&card);
            let card = Rc::new(Card {
                engine,
                port,
                check,
                result,
                pending: RefCell::new(None),
            });
            let weak = Rc::downgrade(&card);
            let commands = commands.clone();
            let next_id = next_id.clone();
            card.check.connect_clicked(move |_| {
                let Some(card) = weak.upgrade() else { return };
                if card.pending.borrow().is_some() {
                    return;
                }
                let id = {
                    let mut next = next_id.borrow_mut();
                    *next = next.wrapping_add(1);
                    *next
                };
                let port = card.port.value_as_int().clamp(1, 65535) as u16;
                if commands
                    .try_send(AttachedCommand::Check {
                        id,
                        engine: card.engine,
                        port,
                    })
                    .is_err()
                {
                    card.result.set_text("The check worker is not available.");
                    return;
                }
                *card.pending.borrow_mut() = Some(id);
                card.check.set_sensitive(false);
                card.result.set_text("Checking…");
            });
            cards.push(card);
        }
        Self { widget, cards }
    }

    pub fn receive(&self, event: &AttachedEvent) {
        let AttachedEvent::Checked { id, engine, found } = event;
        let Some(card) = self.cards.iter().find(|c| c.engine == *engine) else {
            return;
        };
        if *card.pending.borrow() != Some(*id) {
            return;
        }
        *card.pending.borrow_mut() = None;
        card.check.set_sensitive(true);
        let text = match found {
            Found::LmStudio(result) => lmstudio_text(result),
            Found::Vllm(found) => vllm_text(found),
        };
        card.result.set_text(&clean(&text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workstation_runtimes::{local_http::LocalError, vllm::Metrics};

    #[test]
    fn lm_studio_text_states_load_state_and_unknowns() {
        let found = Ok(lmstudio::Discovery {
            source: lmstudio::Source::Compatible,
            models: vec![lmstudio::Model {
                id: "a".into(),
                kind: None,
                architecture: None,
                quantization: None,
                format: None,
                state: ModelState::Unknown,
                max_context_length: Some(8192),
                loaded_context_length: None,
            }],
            truncated: false,
        });
        let text = lmstudio_text(&found);
        assert!(text.contains("names only"));
        assert!(text.contains("load state unknown"));
        assert!(text.contains("up to 8192 tokens context"));
        assert_eq!(
            lmstudio_text(&Err("Nothing answered on that port.".into())),
            "Nothing answered on that port."
        );
    }

    #[test]
    fn vllm_text_never_turns_a_failed_part_into_zero() {
        let found = vllm::Discovery {
            healthy: Err(LocalError::Status(503)),
            models: Ok(vec![]),
            metrics: Ok(Metrics {
                requests_running: Some(2.0),
                ..Metrics::default()
            }),
            truncated: false,
        };
        let text = vllm_text(&found);
        assert!(text.contains("Health: The runtime answered with status 503."));
        assert!(text.contains("Models: none listed"));
        assert!(text.contains("2 running, not reported waiting"));
        assert!(text.contains("KV cache in use: not reported"));
    }
}

#[cfg(test)]
#[path = "attached_smoke.rs"]
mod attached_smoke;
