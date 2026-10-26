// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only discovery of LM Studio and vLLM servers the user runs themselves, on its own
//! channels. A check is one explicit user action and makes one burst of loopback requests;
//! nothing polls, and nothing here can start, stop or change those runtimes.

use tokio::sync::mpsc;
use workstation_runtimes::{
    lmstudio::{self, LmStudioAdapter},
    local_http::LocalError,
    vllm::{self, VllmAdapter},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    LmStudio,
    Vllm,
}

impl Engine {
    pub fn default_port(self) -> u16 {
        match self {
            Self::LmStudio => lmstudio::DEFAULT_PORT,
            Self::Vllm => vllm::DEFAULT_PORT,
        }
    }
}

#[derive(Debug)]
pub enum AttachedCommand {
    Check { id: u64, engine: Engine, port: u16 },
}

#[derive(Debug)]
pub enum Found {
    LmStudio(Result<lmstudio::Discovery, String>),
    Vllm(Box<vllm::Discovery>),
}

#[derive(Debug)]
pub enum AttachedEvent {
    Checked {
        id: u64,
        engine: Engine,
        found: Found,
    },
}

pub fn start(
    runtime: &tokio::runtime::Handle,
) -> (
    mpsc::Sender<AttachedCommand>,
    async_channel::Receiver<AttachedEvent>,
) {
    let (commands, mut inbox) = mpsc::channel::<AttachedCommand>(8);
    let (events, outbox) = async_channel::bounded::<AttachedEvent>(8);
    runtime.spawn(async move {
        while let Some(AttachedCommand::Check { id, engine, port }) = inbox.recv().await {
            let events = events.clone();
            tokio::spawn(async move {
                // The window closing closes the events channel; stop asking then.
                let closed = async {
                    // ponytail: poll the flag; a notifier is not worth a dependency for a card.
                    while !events.is_closed() {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                };
                let found = tokio::select! {
                    found = check(engine, port) => found,
                    () = closed => return,
                };
                let _ = events
                    .send(AttachedEvent::Checked { id, engine, found })
                    .await;
            });
        }
    });
    (commands, outbox)
}

async fn check(engine: Engine, port: u16) -> Found {
    match engine {
        Engine::LmStudio => Found::LmStudio(match LmStudioAdapter::new(port) {
            Ok(adapter) => adapter.discover().await.map_err(|e| e.to_string()),
            Err(message) => Err(message),
        }),
        // The UI keeps the port in range, so a refused adapter reads as nothing listening.
        Engine::Vllm => Found::Vllm(Box::new(match VllmAdapter::new(port) {
            Ok(adapter) => adapter.discover().await,
            Err(_) => vllm::Discovery {
                healthy: Err(LocalError::Unreachable),
                models: Err(LocalError::Unreachable),
                metrics: Err(LocalError::Unreachable),
                truncated: false,
            },
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn a_check_reports_nothing_listening_and_keeps_its_id() {
        let (commands, events) = start(&tokio::runtime::Handle::current());
        for (id, engine) in [(3, Engine::LmStudio), (4, Engine::Vllm)] {
            commands
                .send(AttachedCommand::Check {
                    id,
                    engine,
                    port: free_port(),
                })
                .await
                .unwrap();
            match events.recv().await.unwrap() {
                AttachedEvent::Checked {
                    id: got,
                    found: Found::LmStudio(result),
                    ..
                } => {
                    assert_eq!(got, id);
                    assert!(result.unwrap_err().contains("Nothing answered"));
                }
                AttachedEvent::Checked {
                    id: got,
                    found: Found::Vllm(found),
                    ..
                } => {
                    assert_eq!(got, id);
                    assert!(found.healthy.is_err() && found.models.is_err());
                }
            }
        }
    }
}
