// SPDX-License-Identifier: GPL-3.0-or-later
//! vLLM as an attached, externally owned runtime: read-only health, model list and a bounded
//! reading of its Prometheus counters. Nothing here starts, stops or reconfigures it, and no
//! key is stored.
//!
//! Local execution cannot be proven for this runtime, so no benchmark may treat it as
//! provably local without the user confirming it.

use std::collections::HashMap;

use serde::Deserialize;

use crate::local_http::{DEFAULT_TIMEOUT, LocalError, LoopbackClient, clean};

pub const DEFAULT_PORT: u16 = 8000;
pub const MAX_MODELS: usize = 100;
const MAX_LABEL: usize = 200;
const MAX_METRICS_BODY: usize = 512 * 1024;
const MAX_METRIC_LINES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    /// The longest sequence the server was started to accept.
    pub max_model_len: Option<u64>,
}

/// The counters read from `/metrics`. A name that is absent stays `None`; it is never zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    pub requests_running: Option<f64>,
    pub requests_waiting: Option<f64>,
    /// Fraction of the KV cache in use, 0 to 1 (the highest across engines).
    pub kv_cache_usage: Option<f64>,
    pub prompt_tokens_total: Option<f64>,
    pub generation_tokens_total: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Discovery {
    pub healthy: Result<(), LocalError>,
    pub models: Result<Vec<Model>, LocalError>,
    pub metrics: Result<Metrics, LocalError>,
    /// More models existed than `MAX_MODELS`.
    pub truncated: bool,
}

#[derive(Deserialize)]
struct List {
    data: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    max_model_len: Option<u64>,
}

#[derive(Clone, Copy)]
enum Field {
    Running,
    Waiting,
    Kv,
    Prompt,
    Generation,
}

/// Names vLLM has used for the values above; newer builds renamed the cache gauge.
const KNOWN: [(&str, Field); 6] = [
    ("vllm:num_requests_running", Field::Running),
    ("vllm:num_requests_waiting", Field::Waiting),
    ("vllm:kv_cache_usage_perc", Field::Kv),
    ("vllm:gpu_cache_usage_perc", Field::Kv),
    ("vllm:prompt_tokens_total", Field::Prompt),
    ("vllm:generation_tokens_total", Field::Generation),
];

/// Split one exposition line into its series name and the text after the labels. Labels may
/// contain spaces, braces and escaped quotes inside quoted values.
fn split_series(line: &str) -> Option<(&str, &str)> {
    let name_end = line.find(|c: char| c == '{' || c.is_whitespace())?;
    let (name, rest) = line.split_at(name_end);
    if !rest.starts_with('{') {
        return Some((name, rest));
    }
    let (mut quoted, mut escaped) = (false, false);
    for (index, c) in rest.char_indices() {
        match (quoted, escaped, c) {
            (_, true, _) => escaped = false,
            (true, false, '\\') => escaped = true,
            (_, false, '"') => quoted = !quoted,
            (false, false, '}') => return Some((name, &rest[index + 1..])),
            _ => {}
        }
    }
    None
}

/// Parse the exposition text. Only the names above are read; series that differ by label
/// (one per model or engine) are summed, except the cache gauge, where the highest wins.
/// Non-finite and negative values are ignored, a cache fraction outside 0 to 1 is ignored,
/// and a sum that overflows is not reported. A body with more lines than the cap is refused
/// rather than read partly, because a partial sum would pass for a total.
pub fn parse_metrics(text: &str) -> Result<Metrics, LocalError> {
    if text.lines().take(MAX_METRIC_LINES + 1).count() > MAX_METRIC_LINES {
        return Err(LocalError::TooLarge);
    }
    let mut sums: HashMap<&'static str, f64> = HashMap::new();
    let mut seen: HashMap<&'static str, Field> = HashMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some((name, rest)) = split_series(line) else {
            continue;
        };
        let Some((known, field)) = KNOWN.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        let Some(value) = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .filter(|v| !matches!(field, Field::Kv) || *v <= 1.0)
        else {
            continue;
        };
        let slot = sums.entry(known).or_insert(0.0);
        match field {
            Field::Kv => *slot = slot.max(value),
            _ => *slot += value,
        }
        seen.insert(known, *field);
    }
    let mut metrics = Metrics::default();
    for (name, field) in seen {
        let Some(value) = sums.get(name).copied().filter(|v| v.is_finite()) else {
            continue;
        };
        match field {
            Field::Running => metrics.requests_running = Some(value),
            Field::Waiting => metrics.requests_waiting = Some(value),
            // Two names can feed one field; keep the highest.
            Field::Kv => {
                metrics.kv_cache_usage =
                    Some(metrics.kv_cache_usage.map_or(value, |a| a.max(value)))
            }
            Field::Prompt => metrics.prompt_tokens_total = Some(value),
            Field::Generation => metrics.generation_tokens_total = Some(value),
        }
    }
    Ok(metrics)
}

pub struct VllmAdapter {
    client: LoopbackClient,
}

impl VllmAdapter {
    pub fn new(port: u16) -> Result<Self, String> {
        Ok(Self {
            client: LoopbackClient::new(port, DEFAULT_TIMEOUT)?,
        })
    }

    /// One health check, one model list and one metrics scrape, all concurrently. The caller
    /// decides when; this adapter never polls.
    pub async fn discover(&self) -> Discovery {
        let (health, models, metrics) = tokio::join!(
            self.client.get_bytes("/health", 1024),
            self.client.get_json::<List>("/v1/models"),
            self.client.get_bytes("/metrics", MAX_METRICS_BODY),
        );
        let mut truncated = false;
        let models = models.map(|list| {
            truncated = list.data.len() > MAX_MODELS;
            list.data
                .into_iter()
                .take(MAX_MODELS)
                .filter_map(|raw| {
                    let id = clean(&raw.id, MAX_LABEL);
                    (!id.is_empty()).then_some(Model {
                        id,
                        max_model_len: raw.max_model_len.filter(|n| *n > 0),
                    })
                })
                .collect()
        });
        Discovery {
            healthy: health.map(|_| ()),
            models,
            metrics: metrics.and_then(|body| parse_metrics(&String::from_utf8_lossy(&body))),
            truncated,
        }
    }
}

#[cfg(test)]
mod tests;
