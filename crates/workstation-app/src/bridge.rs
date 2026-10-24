// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed messages keep GTK objects out of blocking hardware probes.

use std::sync::Arc;
use tokio::sync::mpsc;
use workstation_core::hardware::HardwareProfile;

#[derive(Debug)]
pub enum CoreCommand {
    DetectHardware,
}

#[derive(Debug)]
pub enum UiEvent {
    HardwareDetected(Box<HardwareProfile>),
    HardwareFailed(String),
}

pub fn start(
    runtime: &tokio::runtime::Handle,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_probe(runtime, HardwareProfile::detect)
}

fn start_with_probe(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    let (commands, mut command_rx) = mpsc::channel(1);
    let (events, event_rx) = async_channel::bounded(1);
    let probe = Arc::new(probe);
    runtime.spawn(async move {
        loop {
            let command = tokio::select! {
                _ = events.closed() => break,
                command = command_rx.recv() => match command { Some(command) => command, None => break },
            };
            match command {
                CoreCommand::DetectHardware => {
                    let probe = Arc::clone(&probe);
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = tokio::task::spawn_blocking(move || probe()) => result,
                    };
                    // All queued requests are for the same snapshot; do not build a refresh backlog.
                    while command_rx.try_recv().is_ok() {}
                    let event = match result {
                        Ok(profile) if profile.has_readings() => UiEvent::HardwareDetected(Box::new(profile)),
                        Ok(profile) => UiEvent::HardwareFailed(profile.issues.iter()
                            .map(|issue| format!("{}: {}", issue.component, issue.message))
                            .collect::<Vec<_>>().join("\n")),
                        Err(error) => UiEvent::HardwareFailed(format!("Hardware worker failed: {error}")),
                    };
                    if events.send(event).await.is_err() { break; }
                }
            }
        }
    });
    (commands, event_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use workstation_core::hardware::{MemoryPool, ProbeIssue};

    fn snapshot() -> HardwareProfile {
        HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: Some(MemoryPool {
                total_bytes: 1024,
                available_bytes: Some(512),
            }),
            gpus: Vec::new(),
            issues: vec![ProbeIssue {
                component: "CPU".into(),
                message: "Fixture partial failure".into(),
            }],
            captured_at_unix_ms: 1,
        }
    }

    async fn receive(events: &async_channel::Receiver<UiEvent>) -> UiEvent {
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn delivers_partial_readings_and_closes_with_commands() {
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), snapshot);
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        let UiEvent::HardwareDetected(profile) = receive(&events).await else {
            panic!("Expected usable partial readings");
        };
        assert_eq!(profile.issues.len(), 1);
        assert_eq!(profile.system_ram.unwrap().available_bytes, Some(512));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn empty_readings_and_panics_become_failure_events() {
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), || {
            let mut profile = snapshot();
            profile.system_ram = None;
            profile
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        assert!(
            matches!(receive(&events).await, UiEvent::HardwareFailed(message) if message.contains("Fixture partial"))
        );
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), || {
            panic!("fixture probe panic")
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        assert!(
            matches!(receive(&events).await, UiEvent::HardwareFailed(message) if message.contains("Hardware worker failed"))
        );
    }

    #[tokio::test]
    async fn coalesces_queued_refreshes_without_blocking_the_async_worker() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), move || {
            count.fetch_add(1, Ordering::SeqCst);
            started.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            snapshot()
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        assert!(commands.try_send(CoreCommand::DetectHardware).is_err());
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::HardwareDetected(_)
        ));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn closing_window_releases_command_channel_during_a_probe() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_probe(&tokio::runtime::Handle::current(), move || {
            started.send(()).unwrap();
            let _ = gate.lock().unwrap().recv();
            snapshot()
        });
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        events.close();
        let closed = tokio::time::timeout(Duration::from_secs(2), commands.closed()).await;
        release.send(()).unwrap();
        assert!(closed.is_ok());
    }
}
