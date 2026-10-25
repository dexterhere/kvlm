// SPDX-License-Identifier: GPL-3.0-or-later
//! Save the immutable requested run before inference, then its terminal outcome.

use crate::{
    runner::{self, BenchmarkOutcome},
    workloads::{QUICK_WORKLOAD_ID, quick_request},
};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use workstation_core::{
    db::{
        self, HistoryEntry, HistoryStore, PartialMetrics, RunCompletion, RunMetrics, RunOutcome,
        RunSnapshot, WorkloadSnapshot,
    },
    hardware::HardwareProfile,
};
use workstation_runtimes::ollama::{GenerationProgress, InstalledModel};

#[derive(Debug)]
pub struct PersistedRun {
    pub outcome: BenchmarkOutcome,
    pub saved: Result<i64, String>,
}

pub fn load_default() -> Result<Vec<HistoryEntry>, String> {
    HistoryStore::open(&db::default_database_path()?)?.recent()
}

pub async fn run(model: InstalledModel, cancel: watch::Receiver<bool>) -> PersistedRun {
    let path = match db::default_database_path() {
        Ok(path) => path,
        Err(message) => return storage_failure(message),
    };
    persist(path, model, cancel, HardwareProfile::detect, runner::run).await
}

async fn persist<B: Future<Output = BenchmarkOutcome>>(
    path: PathBuf,
    model: InstalledModel,
    cancel: watch::Receiver<bool>,
    probe: impl FnOnce() -> HardwareProfile + Send + 'static,
    run: impl FnOnce(InstalledModel, watch::Receiver<bool>) -> B,
) -> PersistedRun {
    let request = quick_request();
    let snapshot_model = model.clone();
    let start_path = path.clone();
    let start = tokio::task::spawn_blocking(move || {
        let hardware = probe();
        let snapshot = RunSnapshot {
            started_at_unix_ms: now()?,
            model_name: snapshot_model.name,
            model_digest: snapshot_model.digest,
            model_size_bytes: snapshot_model.size,
            hardware: Some(hardware),
            hardware_notice: None,
            runtime_endpoint: "http://127.0.0.1:11434".into(),
            workload: WorkloadSnapshot {
                id: QUICK_WORKLOAD_ID.into(),
                prompt_sha256: format!("{:x}", Sha256::digest(request.prompt.as_bytes())),
                num_predict: request.num_predict,
                num_ctx: request.num_ctx,
                seed: request.seed,
                temperature: 0,
                think: false,
            },
        };
        HistoryStore::open(&start_path)?.start(&snapshot)
    })
    .await;
    let id = match start {
        Ok(Ok(id)) => id,
        Ok(Err(message)) => return storage_failure(message),
        Err(_) => {
            return storage_failure(
                "History snapshot worker failed. Retry before starting inference.".into(),
            );
        }
    };
    let outcome = run(model, cancel).await;
    let completion = match completion(&outcome) {
        Ok(completion) => completion,
        Err(message) => {
            return PersistedRun {
                outcome,
                saved: Err(message),
            };
        }
    };
    let saved = tokio::task::spawn_blocking(move || {
        HistoryStore::open(&path)?.finish(id, &completion)?;
        Ok(id)
    })
    .await
    .unwrap_or_else(|_| {
        Err("History save worker failed. The current result remains available.".into())
    });
    PersistedRun { outcome, saved }
}

fn storage_failure(message: String) -> PersistedRun {
    PersistedRun {
        outcome: BenchmarkOutcome::Failed {
            message: format!("Benchmark did not start: {message}"),
            partial: GenerationProgress::default(),
        },
        saved: Err(message),
    }
}
fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or_else(|| "System clock cannot supply a benchmark history timestamp.".into())
}
fn nanos(duration: Duration) -> Result<u64, String> {
    u64::try_from(duration.as_nanos())
        .map_err(|_| "Benchmark duration exceeds the history format.".into())
}
fn partial(progress: &GenerationProgress) -> Result<PartialMetrics, String> {
    Ok(PartialMetrics {
        first_response_ns: progress.first_output_after.map(nanos).transpose()?,
        response_chunks: progress.response_chunks,
    })
}
fn completion(outcome: &BenchmarkOutcome) -> Result<RunCompletion, String> {
    let mut runtime_version = None;
    let mut model_digest = None;
    let outcome = match outcome {
        BenchmarkOutcome::Completed(report) => {
            let generation = &report.generation;
            runtime_version = Some(generation.runtime_version.clone());
            model_digest = Some(generation.model_digest.clone());
            RunOutcome::Completed(RunMetrics {
                first_response_ns: generation.first_output_after.map(nanos).transpose()?,
                elapsed_ns: nanos(generation.elapsed)?,
                response_chunks: generation.response_chunks,
                eval_count: generation.eval_count,
                eval_duration_ns: generation.eval_duration_ns,
                load_duration_ns: generation.load_duration_ns,
                done_reason: generation.done_reason.clone(),
            })
        }
        BenchmarkOutcome::Cancelled(progress) => RunOutcome::Cancelled(partial(progress)?),
        BenchmarkOutcome::TimedOut(progress) => RunOutcome::TimedOut(partial(progress)?),
        BenchmarkOutcome::Failed {
            message,
            partial: progress,
        } => RunOutcome::Failed {
            message: message.clone(),
            partial: partial(progress)?,
        },
    };
    Ok(RunCompletion {
        finished_at_unix_ms: now()?,
        runtime_version,
        model_digest,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "kvlm-persist-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn database(&self) -> PathBuf {
            self.0.join("history.db")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn hardware() -> HardwareProfile {
        HardwareProfile {
            profile_hash: None,
            cpu: None,
            system_ram: None,
            gpus: vec![],
            issues: vec![],
            captured_at_unix_ms: 1,
        }
    }
    fn model() -> InstalledModel {
        InstalledModel {
            name: "fixture:latest".into(),
            digest: Some("fixture-digest".into()),
            size: Some(42),
            details: None,
            remote_model: None,
            remote_host: None,
        }
    }
    #[tokio::test]
    async fn saves_snapshot_before_job_and_terminal_outcome_survives_restart() {
        let directory = Directory::new();
        let path = directory.database();
        let check_path = path.clone();
        let (_cancel, cancel) = watch::channel(false);
        let result = persist(
            path.clone(),
            model(),
            cancel,
            hardware,
            move |_, _| async move {
                let rows = tokio::task::spawn_blocking(move || {
                    HistoryStore::open(&check_path).unwrap().recent().unwrap()
                })
                .await
                .unwrap();
                assert_eq!(rows.len(), 1);
                assert!(rows[0].completion.is_none());
                assert_eq!(rows[0].snapshot.workload.num_ctx, 2048);
                assert_eq!(rows[0].snapshot.workload.prompt_sha256.len(), 64);
                assert_eq!(
                    rows[0].snapshot.model_digest.as_deref(),
                    Some("fixture-digest")
                );
                BenchmarkOutcome::TimedOut(GenerationProgress {
                    first_output_after: Some(Duration::from_millis(3)),
                    response_chunks: 7,
                })
            },
        )
        .await;
        assert!(result.saved.is_ok());
        let rows = HistoryStore::open(&path).unwrap().recent().unwrap();
        assert!(matches!(
            rows[0].completion.as_ref().unwrap().outcome,
            RunOutcome::TimedOut(PartialMetrics {
                first_response_ns: Some(3_000_000),
                response_chunks: 7
            })
        ));
    }
    #[tokio::test]
    async fn initial_storage_failure_prevents_inference() {
        let (_cancel, cancel) = watch::channel(false);
        let result = persist(
            PathBuf::from("relative-invalid.db"),
            model(),
            cancel,
            hardware,
            |_, _| async { panic!("inference must not start") },
        )
        .await;
        assert!(result.saved.is_err());
        assert!(
            matches!(result.outcome, BenchmarkOutcome::Failed { message, .. } if message.contains("did not start"))
        );
    }
    #[tokio::test]
    async fn final_storage_failure_preserves_the_original_outcome() {
        let directory = Directory::new();
        let path = directory.database();
        let sabotage = path.clone();
        let (_cancel, cancel) = watch::channel(false);
        let result = persist(path, model(), cancel, hardware, move |_, _| async move {
            tokio::task::spawn_blocking(move || {
                let store = HistoryStore::open(&sabotage).unwrap();
                let id = store.recent().unwrap()[0].id;
                store
                    .finish(
                        id,
                        &RunCompletion {
                            finished_at_unix_ms: 1,
                            runtime_version: None,
                            model_digest: None,
                            outcome: RunOutcome::Cancelled(PartialMetrics {
                                first_response_ns: None,
                                response_chunks: 0,
                            }),
                        },
                    )
                    .unwrap();
            })
            .await
            .unwrap();
            BenchmarkOutcome::Failed {
                message: "original stream failure".into(),
                partial: GenerationProgress {
                    first_output_after: None,
                    response_chunks: 9,
                },
            }
        })
        .await;
        assert!(result.saved.is_err());
        assert!(
            matches!(result.outcome, BenchmarkOutcome::Failed { message, partial } if message == "original stream failure" && partial.response_chunks == 9)
        );
    }
    #[tokio::test]
    async fn aborting_worker_leaves_an_honest_unfinished_record() {
        let directory = Directory::new();
        let path = directory.database();
        let (_cancel, cancel) = watch::channel(false);
        let (started, ready) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(persist(
            path.clone(),
            model(),
            cancel,
            hardware,
            move |_, _| async move {
                started.send(()).unwrap();
                std::future::pending().await
            },
        ));
        ready.await.unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        let rows = HistoryStore::open(&path).unwrap().recent().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].completion.is_none());
    }
}
