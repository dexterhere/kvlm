// SPDX-License-Identifier: GPL-3.0-or-later
//! One request with client response-text latency and separate server metrics.

use crate::workloads::{QUICK_WORKLOAD_ID, quick_request};
use std::{future::Future, time::Duration};
use tokio::sync::watch;
use workstation_runtimes::ollama::{
    GenerationProgress, GenerationSummary, InstalledModel, OllamaAdapter,
};

#[derive(Clone, Debug)]
pub struct QuickBenchmarkReport {
    pub workload_id: &'static str,
    pub generation: GenerationSummary,
    pub generation_tokens_per_second: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum BenchmarkOutcome {
    Completed(QuickBenchmarkReport),
    Cancelled(GenerationProgress),
    TimedOut(GenerationProgress),
    Failed {
        message: String,
        partial: GenerationProgress,
    },
}

pub async fn run(model: InstalledModel, cancel: watch::Receiver<bool>) -> BenchmarkOutcome {
    let adapter = match OllamaAdapter::new() {
        Ok(adapter) => adapter,
        Err(message) => {
            return BenchmarkOutcome::Failed {
                message,
                partial: GenerationProgress::default(),
            };
        }
    };
    let (progress, readings) = watch::channel(GenerationProgress::default());
    let job = adapter.generate_local(&model, quick_request(), progress);
    run_job(job, cancel, readings, Duration::from_secs(180)).await
}

async fn run_job(
    job: impl Future<Output = Result<GenerationSummary, String>>,
    mut cancel: watch::Receiver<bool>,
    readings: watch::Receiver<GenerationProgress>,
    deadline: Duration,
) -> BenchmarkOutcome {
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return BenchmarkOutcome::Cancelled(*readings.borrow());
    }
    tokio::select! {
        biased;
        _ = async {
            loop {
                if cancel.changed().await.is_err() || *cancel.borrow() { break; }
            }
        } => BenchmarkOutcome::Cancelled(*readings.borrow()),
        _ = tokio::time::sleep(deadline) => BenchmarkOutcome::TimedOut(*readings.borrow()),
        result = job => match result {
            Ok(generation) => {
                let generation_tokens_per_second = decode_rate(generation.eval_count, generation.eval_duration_ns);
                BenchmarkOutcome::Completed(QuickBenchmarkReport { workload_id: QUICK_WORKLOAD_ID, generation, generation_tokens_per_second })
            }
            Err(message) => BenchmarkOutcome::Failed { message, partial: *readings.borrow() },
        }
    }
}

fn decode_rate(count: Option<u64>, nanoseconds: Option<u64>) -> Option<f64> {
    let (count, nanoseconds) = (count?, nanoseconds?);
    (count > 0 && nanoseconds > 0).then(|| count as f64 * 1_000_000_000.0 / nanoseconds as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> GenerationSummary {
        GenerationSummary {
            model_name: "fixture".into(),
            model_digest: "fixture".into(),
            runtime_version: "fixture".into(),
            first_output_after: Some(Duration::from_millis(15)),
            elapsed: Duration::from_secs(1),
            response_chunks: 2,
            eval_count: Some(12),
            eval_duration_ns: Some(500_000_000),
            load_duration_ns: None,
            done_reason: "stop".into(),
        }
    }

    #[test]
    fn rate_uses_only_positive_server_counters_and_nanoseconds() {
        assert_eq!(decode_rate(Some(12), Some(500_000_000)), Some(24.0));
        for (count, duration) in [
            (None, Some(1)),
            (Some(1), None),
            (Some(0), Some(1)),
            (Some(1), Some(0)),
        ] {
            assert_eq!(decode_rate(count, duration), None);
        }
        assert!(decode_rate(Some(u64::MAX), Some(1)).unwrap().is_finite());
    }

    #[tokio::test]
    async fn completed_and_failed_jobs_preserve_independent_measurements() {
        let (_cancel, cancel) = watch::channel(false);
        let (progress, readings) = watch::channel(GenerationProgress {
            first_output_after: Some(Duration::from_millis(15)),
            response_chunks: 2,
        });
        let BenchmarkOutcome::Completed(report) = run_job(
            async { Ok(summary()) },
            cancel.clone(),
            readings.clone(),
            Duration::from_secs(1),
        )
        .await
        else {
            panic!("expected completion");
        };
        assert_eq!(report.generation_tokens_per_second, Some(24.0));
        assert_eq!(report.generation.response_chunks, 2);
        assert_eq!(report.workload_id, QUICK_WORKLOAD_ID);
        let BenchmarkOutcome::Failed { message, partial } = run_job(
            async { Err("fixture truncated stream".into()) },
            cancel,
            readings,
            Duration::from_secs(1),
        )
        .await
        else {
            panic!("expected failure");
        };
        assert_eq!(message, "fixture truncated stream");
        assert_eq!(partial.response_chunks, 2);
        drop(progress);
    }

    #[tokio::test]
    async fn cancellation_sender_loss_and_deadline_preserve_partial_observations() {
        let (progress, readings) = watch::channel(GenerationProgress {
            first_output_after: None,
            response_chunks: 3,
        });
        let (cancel, cancelled) = watch::channel(true);
        assert!(
            matches!(run_job(std::future::pending(), cancelled, readings.clone(), Duration::from_secs(1)).await, BenchmarkOutcome::Cancelled(partial) if partial.response_chunks == 3)
        );
        drop(cancel);
        let (cancel, cancelled) = watch::channel(false);
        let job = run_job(
            std::future::pending(),
            cancelled,
            readings.clone(),
            Duration::from_secs(1),
        );
        drop(cancel);
        assert!(matches!(job.await, BenchmarkOutcome::Cancelled(_)));
        let (_cancel, cancelled) = watch::channel(false);
        assert!(
            matches!(run_job(std::future::pending(), cancelled, readings, Duration::from_millis(1)).await, BenchmarkOutcome::TimedOut(partial) if partial.response_chunks == 3)
        );
        drop(progress);
    }

    #[tokio::test]
    async fn live_cancellation_drops_the_active_request_future() {
        let (cancel, cancelled) = watch::channel(false);
        let (_progress, readings) = watch::channel(GenerationProgress::default());
        let (started, start) = tokio::sync::oneshot::channel();
        let job = async move {
            let _ = started.send(());
            std::future::pending().await
        };
        let worker = tokio::spawn(run_job(job, cancelled, readings, Duration::from_secs(1)));
        start.await.unwrap();
        cancel.send(true).unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(200), worker)
                .await
                .unwrap()
                .unwrap(),
            BenchmarkOutcome::Cancelled(_)
        ));
    }
}
