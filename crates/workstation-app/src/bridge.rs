// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed messages keep GTK objects out of blocking hardware probes.

use crate::fit_workflow::{FitReport, FitRequest, estimate};
use std::sync::Arc;
use tokio::sync::mpsc;
use workstation_core::hardware::HardwareProfile;
use workstation_runtimes::ollama::{Discovery, ModelInspection, OllamaAdapter};

#[derive(Debug)]
pub enum CoreCommand {
    DetectHardware,
    DiscoverOllama,
    InspectOllama { id: u64, model: String },
    EstimateFit(FitRequest),
}

#[derive(Debug)]
pub enum UiEvent {
    OllamaDiscovered(Box<Discovery>),
    OllamaFailed(String),
    OllamaInspected {
        id: u64,
        result: Result<Box<ModelInspection>, String>,
    },
    HardwareDetected(Box<HardwareProfile>),
    HardwareFailed(String),
    FitFinished {
        id: u64,
        result: Result<Box<FitReport>, String>,
    },
}

pub fn start(
    runtime: &tokio::runtime::Handle,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_workers(runtime, HardwareProfile::detect, estimate)
}

#[cfg(test)]
fn start_with_probe(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_workers(runtime, probe, estimate)
}

fn start_with_workers(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
    fit: impl Fn(FitRequest) -> Result<FitReport, String> + Send + Sync + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>) {
    start_with_runtime_workers(
        runtime,
        probe,
        fit,
        || async { OllamaAdapter::new()?.discover().await },
        |model| async move { OllamaAdapter::new()?.inspect_model(&model).await },
    )
}

fn start_with_runtime_workers<D, I>(
    runtime: &tokio::runtime::Handle,
    probe: impl Fn() -> HardwareProfile + Send + Sync + 'static,
    fit: impl Fn(FitRequest) -> Result<FitReport, String> + Send + Sync + 'static,
    discover: impl Fn() -> D + Send + 'static,
    inspect: impl Fn(String) -> I + Send + 'static,
) -> (mpsc::Sender<CoreCommand>, async_channel::Receiver<UiEvent>)
where
    D: std::future::Future<Output = Result<Discovery, String>> + Send,
    I: std::future::Future<Output = Result<ModelInspection, String>> + Send,
{
    let (commands, mut command_rx) = mpsc::channel(1);
    let (events, event_rx) = async_channel::bounded(1);
    let probe = Arc::new(probe);
    let fit = Arc::new(fit);
    runtime.spawn(async move {
        let mut pending = None;
        loop {
            if events.is_closed() { break; }
            let command = if let Some(command) = pending.take() { command } else { tokio::select! {
                _ = events.closed() => break,
                command = command_rx.recv() => match command { Some(command) => command, None => break },
            }};
            match command {
                CoreCommand::DetectHardware => {
                    let probe = Arc::clone(&probe);
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = tokio::task::spawn_blocking(move || probe()) => result,
                    };
                    // Coalesce only refreshes; a queued fit request must not be discarded.
                    while let Ok(command) = command_rx.try_recv() {
                        if matches!(command, CoreCommand::DetectHardware) { continue; }
                        pending = Some(command);
                        break;
                    }
                    let event = match result {
                        Ok(profile) if profile.has_readings() => UiEvent::HardwareDetected(Box::new(profile)),
                        Ok(profile) => UiEvent::HardwareFailed(profile.issues.iter()
                            .map(|issue| format!("{}: {}", issue.component, issue.message))
                            .collect::<Vec<_>>().join("\n")),
                        Err(error) => UiEvent::HardwareFailed(format!("Hardware worker failed: {error}")),
                    };
                    if events.send(event).await.is_err() { break; }
                }
                CoreCommand::DiscoverOllama => {
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = discover() => result,
                    };
                    let event = match result {
                        Ok(discovery) => UiEvent::OllamaDiscovered(Box::new(discovery)),
                        Err(message) => UiEvent::OllamaFailed(message),
                    };
                    if events.send(event).await.is_err() { break; }
                }
                CoreCommand::InspectOllama { id, model } => {
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = inspect(model) => result.map(Box::new),
                    };
                    if events.send(UiEvent::OllamaInspected { id, result }).await.is_err() { break; }
                }
                CoreCommand::EstimateFit(request) => {
                    let id = request.id;
                    let fit = Arc::clone(&fit);
                    let result = tokio::select! {
                        _ = events.closed() => break,
                        result = tokio::task::spawn_blocking(move || fit(request)) => result,
                    };
                    let result = result
                        .map_err(|_| "Model inspection worker failed. Try again with a local GGUF file.".into())
                        .and_then(|result| result)
                        .map(Box::new);
                    if events.send(UiEvent::FitFinished { id, result }).await.is_err() { break; }
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

    fn fit_request(id: u64) -> CoreCommand {
        use workstation_core::fit::{gguf_fit::LayoutAssumption, memory_math::KvCacheType};
        CoreCommand::EstimateFit(FitRequest {
            id,
            path: "unused-fixture-path".into(),
            scenario: crate::fit_workflow::parse_scenario(
                "4096",
                "512",
                KvCacheType::F16,
                KvCacheType::F16,
            )
            .unwrap(),
            layout: LayoutAssumption::Unconfirmed,
        })
    }

    #[tokio::test]
    async fn hardware_coalescing_preserves_a_queued_fit_request() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_workers(
            &tokio::runtime::Handle::current(),
            move || {
                started.send(()).unwrap();
                gate.lock().unwrap().recv().unwrap();
                snapshot()
            },
            |_| Err("Fixture inspection failure".into()),
        );
        commands.try_send(CoreCommand::DetectHardware).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        commands.try_send(fit_request(42)).unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::HardwareDetected(_)
        ));
        assert!(matches!(receive(&events).await,
            UiEvent::FitFinished { id: 42, result: Err(message) } if message == "Fixture inspection failure"));
    }

    #[tokio::test]
    async fn fit_jobs_are_not_coalesced_and_do_not_block_the_async_executor() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) = start_with_workers(
            &tokio::runtime::Handle::current(),
            snapshot,
            move |request| {
                started.send(request.id).unwrap();
                gate.lock().unwrap().recv().unwrap();
                Err("Fixture fit result".into())
            },
        );
        commands.try_send(fit_request(1)).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
                .await
                .unwrap(),
            Some(1)
        );
        commands.try_send(fit_request(2)).unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::FitFinished { id: 1, .. }
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
                .await
                .unwrap(),
            Some(2)
        );
        release.send(()).unwrap();
        assert!(matches!(
            receive(&events).await,
            UiEvent::FitFinished { id: 2, .. }
        ));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn closing_window_releases_commands_during_fit_and_panics_are_reported() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let (commands, events) =
            start_with_workers(&tokio::runtime::Handle::current(), snapshot, move |_| {
                started.send(()).unwrap();
                let _ = gate.lock().unwrap().recv();
                Err("Fixture ended".into())
            });
        commands.try_send(fit_request(9)).unwrap();
        tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        events.close();
        let closed = tokio::time::timeout(Duration::from_secs(2), commands.closed()).await;
        release.send(()).unwrap();
        assert!(closed.is_ok());

        let (commands, events) =
            start_with_workers(&tokio::runtime::Handle::current(), snapshot, |_| {
                panic!("Fixture inspection panic")
            });
        commands.try_send(fit_request(10)).unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::FitFinished { id: 10, result: Err(message) } if message.contains("Model inspection worker failed")));
    }
    #[tokio::test]
    async fn preserves_partial_runtime_results_and_inspection_request_identity() {
        let (commands, events) = start_with_runtime_workers(
            &tokio::runtime::Handle::current(),
            snapshot,
            |_| Err("unused fit".into()),
            || async {
                Ok(Discovery {
                    version: "fixture".into(),
                    installed: Ok(vec![]),
                    running: Err("fixture inventory failure".into()),
                })
            },
            |model| async move {
                if model == "missing" {
                    return Err("fixture model removed".into());
                }
                assert_eq!(model, "chosen:latest");
                Ok(ModelInspection {
                    metadata: Err("fixture metadata failure".into()),
                    running: Ok(vec![]),
                })
            },
        );
        commands.try_send(CoreCommand::DiscoverOllama).unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::OllamaDiscovered(snapshot) if snapshot.installed.is_ok() && snapshot.running.is_err()));
        commands
            .try_send(CoreCommand::InspectOllama {
                id: 17,
                model: "chosen:latest".into(),
            })
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::OllamaInspected { id: 17, result: Ok(report) }
                if report.metadata.is_err() && report.running.is_ok()));
        commands
            .try_send(CoreCommand::InspectOllama {
                id: 18,
                model: "missing".into(),
            })
            .unwrap();
        assert!(matches!(receive(&events).await,
            UiEvent::OllamaInspected { id: 18, result: Err(message) } if message == "fixture model removed"));
        drop(commands);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn closing_window_cancels_pending_discovery_and_model_inspection() {
        for inspection in [false, true] {
            let (started, mut started_rx) = mpsc::unbounded_channel();
            let inspection_started = started.clone();
            let (commands, events) = start_with_runtime_workers(
                &tokio::runtime::Handle::current(),
                snapshot,
                |_| Err("unused fit".into()),
                move || {
                    started.send(()).unwrap();
                    std::future::pending()
                },
                move |_| {
                    inspection_started.send(()).unwrap();
                    std::future::pending()
                },
            );
            commands
                .try_send(if inspection {
                    CoreCommand::InspectOllama {
                        id: 1,
                        model: "fixture".into(),
                    }
                } else {
                    CoreCommand::DiscoverOllama
                })
                .unwrap();
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
}
