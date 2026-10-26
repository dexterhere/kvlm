// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-pass latency and speed figures from a token stream. Nothing here invents a token:
//! inter-token latency exists only when the event count matches the server's token count exactly, and speeds
//! come from the server's own counters.

use std::time::Duration;
use workstation_core::experiments::Itl;

/// Fewest intervals for a percentile to mean anything.
pub const MIN_INTERVALS: usize = 8;
/// Arrival times kept per pass; a longer stream cannot be checked event by event.
pub const MAX_EVENTS: usize = 8_192;

/// Everything the adapter saw of one streamed answer.
#[derive(Clone, Debug, Default)]
pub struct StreamRecord {
    /// Arrival of each non-empty content event, as time since the request was sent.
    pub content_arrivals: Vec<Duration>,
    /// Reasoning deltas. They are tokens too, so any of them breaks the one-to-one proof.
    pub reasoning_events: u64,
    /// `true` when the client combined events (a transport chunk holding several).
    pub events_merged: bool,
    /// The server's own generated-token count (`predicted_n` / `eval_count`).
    pub reported_tokens: Option<u64>,
    /// More events arrived than `MAX_EVENTS`; the excess was not kept.
    pub truncated: bool,
}

impl StreamRecord {
    pub fn push_content(&mut self, since_request: Duration) {
        if self.content_arrivals.len() < MAX_EVENTS {
            self.content_arrivals.push(since_request);
        } else {
            self.truncated = true;
        }
    }

    /// Time to the first content event.
    pub fn ttft_ms(&self) -> Option<f64> {
        self.content_arrivals.first().map(|d| ms(*d))
    }

    /// Inter-token latency, or the reason it is unknown.
    pub fn itl(&self) -> Result<Itl, String> {
        let Some(reported) = self.reported_tokens else {
            return Err("the server did not report how many tokens it generated".into());
        };
        if self.truncated {
            return Err("the stream was longer than can be checked event by event".into());
        }
        if self.events_merged {
            return Err("some stream events were combined, so they may hold several tokens".into());
        }
        if self.reasoning_events > 0 {
            return Err("reasoning tokens were streamed, which this check does not count".into());
        }
        let events = self.content_arrivals.len() as u64;
        if events != reported {
            return Err(format!(
                "{events} stream events for {reported} generated tokens, so events are not single tokens"
            ));
        }
        if self.content_arrivals.windows(2).any(|w| w[1] < w[0]) {
            return Err("event times were not in order".into());
        }
        let mut gaps: Vec<f64> = self
            .content_arrivals
            .windows(2)
            .map(|w| ms(w[1] - w[0]))
            .collect();
        if gaps.len() < MIN_INTERVALS {
            return Err(format!(
                "only {} intervals; at least {MIN_INTERVALS} are needed",
                gaps.len()
            ));
        }
        gaps.sort_by(f64::total_cmp);
        Ok(Itl {
            intervals: gaps.len() as u32,
            median_ms: nearest_rank(&gaps, 50),
            p95_ms: nearest_rank(&gaps, 95),
        })
    }
}

/// Nearest-rank percentile of an ascending, non-empty slice.
pub fn nearest_rank(sorted: &[f64], percent: usize) -> f64 {
    let rank = (percent * sorted.len())
        .div_ceil(100)
        .clamp(1, sorted.len());
    sorted[rank - 1]
}

/// Tokens per second from the server's own count and time. Unknown when either is missing
/// or not positive, so a zero-token or zero-time answer never reads as a speed.
pub fn tokens_per_second(tokens: Option<u64>, server_ms: Option<f64>) -> Option<f64> {
    let (tokens, server_ms) = (tokens?, server_ms?);
    (tokens > 0 && server_ms.is_finite() && server_ms > 0.0)
        .then(|| tokens as f64 / (server_ms / 1000.0))
        .filter(|rate| rate.is_finite())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests;
