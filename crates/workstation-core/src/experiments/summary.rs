// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-experiment summaries: median with min and max across the measured passes that report a
//! value. A failed pass is counted and excluded, never averaged in.

use serde::{Deserialize, Serialize};

use super::{Pass, PassKind, PassMetrics, Sampled, State};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    /// How many passes reported this value.
    pub count: u32,
}

impl Spread {
    pub fn of(values: &[f64]) -> Option<Self> {
        let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        Some(Self {
            median,
            min: sorted[0],
            max: sorted[n - 1],
            count: n as u32,
        })
    }

    /// Whether two spreads leave room to say one is higher: their ranges must not overlap.
    pub fn distinguishable_from(&self, other: &Self) -> bool {
        self.max < other.min || other.max < self.min
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Measured passes that completed and are summarized.
    pub measured_ok: u32,
    /// Measured passes that failed, were cancelled or were interrupted.
    pub excluded: u32,
    pub wall_ms: Option<Spread>,
    pub ttft_ms: Option<Spread>,
    pub prompt_tokens_per_s: Option<Spread>,
    pub decode_tokens_per_s: Option<Spread>,
    pub itl_median_ms: Option<Spread>,
    pub itl_p95_ms: Option<Spread>,
    pub process_rss_peak_bytes: Option<Spread>,
    pub gpu_memory_peak_bytes: Option<Spread>,
    pub gpu_utilization_mean_percent: Option<Spread>,
    pub cpu_utilization_mean_percent: Option<Spread>,
    pub load_ms: Option<Spread>,
}

fn spread(metrics: &[&PassMetrics], pick: impl Fn(&PassMetrics) -> Option<f64>) -> Option<Spread> {
    let values: Vec<f64> = metrics.iter().filter_map(|m| pick(m)).collect();
    Spread::of(&values)
}

fn peak(s: &Option<Sampled>) -> Option<f64> {
    s.as_ref().map(|s| s.peak)
}

fn mean(s: &Option<Sampled>) -> Option<f64> {
    s.as_ref().map(|s| s.mean)
}

pub fn summarize(passes: &[Pass]) -> Summary {
    let measured: Vec<&Pass> = passes
        .iter()
        .filter(|p| p.kind == PassKind::Measured)
        .collect();
    let ok: Vec<&PassMetrics> = measured
        .iter()
        .filter(|p| p.state == State::Completed)
        .filter_map(|p| p.metrics.as_ref())
        .collect();
    let cold: Vec<&PassMetrics> = passes
        .iter()
        .filter(|p| p.kind == PassKind::ColdLoad && p.state == State::Completed)
        .filter_map(|p| p.metrics.as_ref())
        .collect();
    Summary {
        measured_ok: ok.len() as u32,
        excluded: (measured.len() - ok.len()) as u32,
        wall_ms: spread(&ok, |m| Some(m.wall_ms as f64)),
        ttft_ms: spread(&ok, |m| m.ttft_ms),
        prompt_tokens_per_s: spread(&ok, |m| m.prompt_tokens_per_s),
        decode_tokens_per_s: spread(&ok, |m| m.decode_tokens_per_s),
        itl_median_ms: spread(&ok, |m| m.itl.map(|i| i.median_ms)),
        itl_p95_ms: spread(&ok, |m| m.itl.map(|i| i.p95_ms)),
        process_rss_peak_bytes: spread(&ok, |m| peak(&m.process_rss_bytes)),
        gpu_memory_peak_bytes: spread(&ok, |m| peak(&m.gpu_memory_bytes)),
        gpu_utilization_mean_percent: spread(&ok, |m| mean(&m.gpu_utilization_percent)),
        cpu_utilization_mean_percent: spread(&ok, |m| mean(&m.cpu_utilization_percent)),
        load_ms: spread(&cold, |m| m.load_ms),
    }
}
