// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed messages keep GTK objects out of Tokio workers.

use tokio::sync::mpsc;
use workstation_runtimes::ollama::{Discovery, OllamaAdapter};

#[derive(Debug)]
pub enum CoreCommand {
    Initialize,
    DiscoverOllama,
}

#[derive(Debug)]
pub enum UiEvent {
    WorkerReady,
    OllamaDiscovered(Box<Discovery>),
    OllamaFailed(String),
}

pub fn start(
    runtime: &tokio::runtime::Handle,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_discovery(runtime, || async { OllamaAdapter::new()?.discover().await })
}

fn start_with_discovery<F: std::future::Future<Output = Result<Discovery, String>> + Send>(
    runtime: &tokio::runtime::Handle,
    discover: impl Fn() -> F + Send + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    let (commands, mut command_rx) = mpsc::channel(2);
    let (events, event_rx) = async_channel::bounded(2);

    runtime.spawn(async move {
        loop {
            let command = tokio::select! {
                _ = events.closed() => break,
                command = command_rx.recv() => match command { Some(command) => command, None => break },
            };
            let event = match command {
                CoreCommand::Initialize => UiEvent::WorkerReady,
                CoreCommand::DiscoverOllama => {
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = discover() => result,
                    };
                    match result {
                        Ok(discovery) => UiEvent::OllamaDiscovered(Box::new(discovery)),
                        Err(message) => UiEvent::OllamaFailed(message),
                    }
                }
            };
            if events.send(event).await.is_err() {
                break;
            }
        }
    });

    (commands, event_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn delivers_event_and_closes_after_commands_are_dropped() {
        let (commands, events) = start(&tokio::runtime::Handle::current());
        commands.try_send(CoreCommand::Initialize).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, UiEvent::WorkerReady));

        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn closed_event_channel_does_not_panic_worker() {
        let (commands, events) = start(&tokio::runtime::Handle::current());
        events.close();
        commands.try_send(CoreCommand::Initialize).unwrap();
        tokio::time::timeout(Duration::from_secs(2), commands.closed())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delivers_partial_discovery_and_failure() {
        let (commands, events) =
            start_with_discovery(&tokio::runtime::Handle::current(), || async {
                Ok(Discovery {
                    version: "test".into(),
                    installed: Ok(vec![]),
                    running: Err("fixture partial failure".into()),
                })
            });
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, UiEvent::OllamaDiscovered(discovery) if discovery.running.is_err())
        );
        let (commands, events) =
            start_with_discovery(&tokio::runtime::Handle::current(), || async {
                Err("fixture unavailable".into())
            });
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, UiEvent::OllamaFailed(message) if message == "fixture unavailable")
        );
    }

    #[tokio::test]
    async fn closing_window_cancels_pending_discovery() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (commands, events) =
            start_with_discovery(&tokio::runtime::Handle::current(), move || {
                started.send(()).unwrap();
                std::future::pending()
            });
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        events.close();
        tokio::time::timeout(Duration::from_secs(2), commands.closed())
            .await
            .unwrap();
    }
}
