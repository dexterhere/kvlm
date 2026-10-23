// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed messages keep GTK objects out of Tokio workers.

use tokio::sync::mpsc;

#[derive(Debug)]
pub enum CoreCommand {
    Initialize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UiEvent {
    WorkerReady,
}

pub fn start(
    runtime: &tokio::runtime::Handle,
) -> (
    mpsc::UnboundedSender<CoreCommand>,
    async_channel::Receiver<UiEvent>,
) {
    let (commands, mut command_rx) = mpsc::unbounded_channel();
    let (events, event_rx) = async_channel::bounded(8);

    runtime.spawn(async move {
        while let Some(command) = command_rx.recv().await {
            let event = match command {
                CoreCommand::Initialize => UiEvent::WorkerReady,
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
        commands.send(CoreCommand::Initialize).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event, UiEvent::WorkerReady);

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
        commands.send(CoreCommand::Initialize).unwrap();
        tokio::time::timeout(Duration::from_secs(2), commands.closed())
            .await
            .unwrap();
    }
}
