// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded system/device memory sampling around one benchmark operation.

use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio::{
    runtime::Handle,
    time::{self, MissedTickBehavior},
};
use workstation_core::hardware::resources::{
    GpuMemoryPeak, MAX_GPUS, MemorySample, ResourceSummary,
};

pub const SAMPLE_INTERVAL_MS: u64 = 100;
pub const MAX_SAMPLES: u64 = 2_000;

struct ProbeGuard<P: Send + 'static> {
    probe: Option<P>,
    runtime: Handle,
}

impl<P: Send + 'static> ProbeGuard<P> {
    fn new(probe: P) -> Self {
        Self {
            probe: Some(probe),
            runtime: Handle::current(),
        }
    }
}

impl<P> ProbeGuard<P>
where
    P: FnMut() -> MemorySample + Send + 'static,
{
    fn sample(&mut self) -> MemorySample {
        (self.probe.as_mut().expect("probe guard is populated"))()
    }
}

impl<P: Send + 'static> Drop for ProbeGuard<P> {
    fn drop(&mut self) {
        if let Some(probe) = self.probe.take() {
            drop(self.runtime.spawn_blocking(move || drop(probe)));
        }
    }
}

/// Run a job while sampling system-wide memory from a blocking probe.
pub async fn measure<F, P>(job: F, probe: P) -> (F::Output, ResourceSummary)
where
    F: Future,
    P: FnMut() -> MemorySample + Send + 'static,
{
    measure_at_interval(job, probe, Duration::from_millis(SAMPLE_INTERVAL_MS)).await
}

async fn measure_at_interval<F, P>(
    job: F,
    probe: P,
    interval: Duration,
) -> (F::Output, ResourceSummary)
where
    F: Future,
    P: FnMut() -> MemorySample + Send + 'static,
{
    measure_with_limit(job, probe, interval, MAX_SAMPLES).await
}

async fn measure_with_limit<F, P>(
    job: F,
    probe: P,
    interval: Duration,
    sample_limit: u64,
) -> (F::Output, ResourceSummary)
where
    F: Future,
    P: FnMut() -> MemorySample + Send + 'static,
{
    let start = Instant::now();
    let mut job = Box::pin(job);
    let mut probe = Some(ProbeGuard::new(probe));
    let mut pending = None;
    let mut ticker = time::interval_at(time::Instant::now(), interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut summary = ResourceSummary {
        interval_ms: interval.as_millis().min(u64::MAX as u128) as u64,
        ..ResourceSummary::default()
    };

    loop {
        tokio::select! {
            biased;
            output = &mut job => {
                summary.window_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
                summary.probe_in_flight = pending.is_some();
                return (output, summary);
            }
            _ = ticker.tick(), if pending.is_none() && probe.is_some() && summary.samples < sample_limit => {
                let mut next_probe = probe.take().expect("probe is present when no probe is pending");
                pending = Some(tokio::task::spawn_blocking(move || {
                    let reading = next_probe.sample();
                    (next_probe, reading)
                }));
                summary.probe_in_flight = true;
            }
            result = async { pending.as_mut().expect("pending probe").await }, if pending.is_some() => {
                pending = None;
                summary.probe_in_flight = false;
                match result {
                    Ok((next_probe, reading)) => {
                        probe = Some(next_probe);
                        summary.samples += 1;
                        accumulate(&mut summary, reading);
                        if summary.samples == sample_limit {
                            summary.limit_reached = true;
                        }
                    }
                    Err(_) => {
                        summary.probe_failed = true;
                        probe = None;
                    }
                }
            }
        }
    }
}

fn accumulate(summary: &mut ResourceSummary, reading: MemorySample) {
    if let Some(value) = reading.system_ram_nonavailable_bytes {
        summary.system_ram_samples += 1;
        summary.peak_system_ram_nonavailable_bytes = Some(
            summary
                .peak_system_ram_nonavailable_bytes
                .map_or(value, |peak| peak.max(value)),
        );
    }

    let batch = &reading.gpus[..reading.gpus.len().min(MAX_GPUS)];
    for (index, gpu) in batch.iter().enumerate() {
        if gpu.id.is_empty()
            || batch
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.id == gpu.id)
        {
            continue;
        }
        let index = match summary.gpus.iter().position(|peak| peak.id == gpu.id) {
            Some(index) => index,
            None => {
                if summary.gpus.len() >= MAX_GPUS {
                    continue;
                }
                summary.gpus.push(GpuMemoryPeak {
                    id: gpu.id.clone(),
                    name: gpu.name.clone(),
                    vram_samples: 0,
                    peak_vram_used_bytes: None,
                    gtt_samples: 0,
                    peak_gtt_used_bytes: None,
                });
                summary.gpus.len() - 1
            }
        };
        let peak = &mut summary.gpus[index];
        if let Some(value) = gpu.vram_used_bytes {
            peak.vram_samples += 1;
            peak.peak_vram_used_bytes = Some(
                peak.peak_vram_used_bytes
                    .map_or(value, |current| current.max(value)),
            );
        }
        if let Some(value) = gpu.gtt_used_bytes {
            peak.gtt_samples += 1;
            peak.peak_gtt_used_bytes = Some(
                peak.peak_gtt_used_bytes
                    .map_or(value, |current| current.max(value)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct BlockingDrop {
        started: Option<tokio::sync::oneshot::Sender<()>>,
        released: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl Drop for BlockingDrop {
        fn drop(&mut self) {
            if let Some(tx) = self.started.take() {
                let _ = tx.send(());
            }
            std::thread::sleep(Duration::from_millis(150));
            if let Some(tx) = self.released.take() {
                let _ = tx.send(());
            }
        }
    }

    fn gpu(
        id: &str,
        name: &str,
        vram: Option<u64>,
        gtt: Option<u64>,
    ) -> workstation_core::hardware::resources::GpuMemorySample {
        workstation_core::hardware::resources::GpuMemorySample {
            id: id.into(),
            name: name.into(),
            vram_used_bytes: vram,
            gtt_used_bytes: gtt,
        }
    }

    #[test]
    fn aggregates_unknown_zero_peaks_duplicates_and_gpu_cap() {
        let mut summary = ResourceSummary::default();
        accumulate(
            &mut summary,
            MemorySample {
                system_ram_nonavailable_bytes: Some(0),
                gpus: vec![
                    gpu("stable-1", "first name", Some(0), None),
                    gpu("stable-1", "duplicate", Some(99), Some(20)),
                    gpu("", "invalid", Some(5), Some(5)),
                    gpu("stable-2", "second", None, Some(0)),
                ],
            },
        );
        accumulate(
            &mut summary,
            MemorySample {
                system_ram_nonavailable_bytes: None,
                gpus: (0..MAX_GPUS + 3)
                    .map(|index| gpu(&format!("device-{index}"), "gpu", None, None))
                    .collect(),
            },
        );

        assert_eq!(summary.system_ram_samples, 1);
        assert_eq!(summary.peak_system_ram_nonavailable_bytes, Some(0));
        assert_eq!(summary.gpus.len(), MAX_GPUS);
        assert!(!summary.gpus.iter().any(|item| item.id == "stable-1"));
        let second = summary
            .gpus
            .iter()
            .find(|item| item.id == "stable-2")
            .unwrap();
        assert_eq!(second.peak_gtt_used_bytes, Some(0));
        assert_eq!(second.gtt_samples, 1);
    }

    #[tokio::test]
    async fn unknown_reading_counts_completed_probe_without_inventing_values() {
        let (_, summary) = measure_at_interval(
            async { time::sleep(Duration::from_millis(20)).await },
            MemorySample::default,
            Duration::from_secs(1),
        )
        .await;
        assert!(summary.samples >= 1);
        assert_eq!(summary.system_ram_samples, 0);
        assert_eq!(summary.peak_system_ram_nonavailable_bytes, None);
        assert!(summary.gpus.is_empty());
        assert!(!summary.probe_failed);
    }

    #[tokio::test]
    async fn cpu_only_reading_preserves_ram_without_a_gpu_placeholder() {
        let (_, summary) = measure_at_interval(
            async { time::sleep(Duration::from_millis(20)).await },
            || MemorySample {
                system_ram_nonavailable_bytes: Some(42),
                gpus: Vec::new(),
            },
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(summary.peak_system_ram_nonavailable_bytes, Some(42));
        assert_eq!(summary.system_ram_samples, summary.samples);
        assert!(summary.gpus.is_empty());
    }

    #[tokio::test]
    async fn panicked_probe_stops_sampling_but_does_not_fail_job() {
        let (result, summary) = measure_at_interval(
            async {
                time::sleep(Duration::from_millis(20)).await;
                7
            },
            || -> MemorySample { panic!("probe failed") },
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(result, 7);
        assert!(summary.probe_failed);
        assert_eq!(summary.samples, 0);
        assert!(!summary.probe_in_flight);
    }

    #[tokio::test]
    async fn slow_probe_does_not_block_job_completion_or_burst_ticks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = calls.clone();
        let start = Instant::now();
        let (_, summary) = measure_at_interval(
            async { time::sleep(Duration::from_millis(30)).await },
            move || {
                probe_calls.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(500));
                MemorySample::default()
            },
            Duration::from_millis(2),
        )
        .await;
        assert!(start.elapsed() < Duration::from_millis(250));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(summary.probe_in_flight);
    }

    #[tokio::test]
    async fn dropping_measure_prevents_any_later_probe_launch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = calls.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut started_tx = Some(started_tx);
        let task = tokio::spawn(measure_at_interval(
            std::future::pending::<()>(),
            move || {
                probe_calls.fetch_add(1, Ordering::SeqCst);
                if let Some(tx) = started_tx.take() {
                    let _ = tx.send(());
                }
                std::thread::sleep(Duration::from_millis(50));
                MemorySample::default()
            },
            Duration::from_millis(2),
        ));
        started_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        time::sleep(Duration::from_millis(20)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn blocking_probe_drop_does_not_delay_job_completion() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (released_tx, released_rx) = tokio::sync::oneshot::channel();
        let blocker = BlockingDrop {
            started: Some(started_tx),
            released: Some(released_tx),
        };
        let probe = move || {
            let _keep_drop_guard = &blocker;
            MemorySample::default()
        };
        let start = Instant::now();
        let (_, _) = time::timeout(
            Duration::from_millis(300),
            measure_at_interval(async {}, probe, Duration::from_secs(1)),
        )
        .await
        .expect("job completion must not wait for probe cleanup");
        assert!(start.elapsed() < Duration::from_millis(100));
        started_rx.await.expect("probe cleanup worker started");
        time::timeout(Duration::from_secs(1), released_rx)
            .await
            .expect("probe cleanup worker must finish")
            .expect("probe cleanup worker released");
    }

    #[tokio::test]
    async fn sample_limit_stops_sampling_but_still_awaits_job() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = calls.clone();
        let (result, summary) = measure_with_limit(
            async {
                time::sleep(Duration::from_millis(20)).await;
                9
            },
            move || {
                probe_calls.fetch_add(1, Ordering::SeqCst);
                MemorySample::default()
            },
            Duration::from_millis(1),
            3,
        )
        .await;
        assert_eq!(result, 9);
        assert_eq!(summary.samples, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(summary.limit_reached);
    }
}
