// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTP client for one managed `llama-server` on loopback: identity-checked readiness,
//! loaded-model facts, busy signals, Prometheus counters and a streaming chat completion.
//!
//! The client can only be built for `127.0.0.1:<port>`, never follows redirects or proxies,
//! bounds every body while streaming, and returns typed errors that carry no response text.

use reqwest::{Client, Response, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const SHORT_TIMEOUT: Duration = Duration::from_secs(2);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SMALL_BODY: usize = 1024 * 1024;
const MAX_METRICS_BODY: usize = 256 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: u64 = 100_000;
const MAX_SLOTS: usize = 256;
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlamaError {
    /// Nothing is listening, or the connection dropped.
    Unreachable,
    Timeout,
    /// An unexpected status; the body is never echoed.
    Status(u16),
    BadResponse,
    TooLarge,
    /// The server does not offer this endpoint (disabled or older build).
    Unsupported,
    Cancelled,
}

impl std::fmt::Display for LlamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(code) => write!(f, "llama-server answered with status {code}."),
            Self::Unreachable => f.write_str("Cannot reach llama-server."),
            Self::Timeout => f.write_str("llama-server did not answer in time."),
            Self::BadResponse => {
                f.write_str("llama-server sent a response the application cannot use.")
            }
            Self::TooLarge => {
                f.write_str("llama-server sent more data than the application accepts.")
            }
            Self::Unsupported => f.write_str("This llama-server does not offer that information."),
            Self::Cancelled => f.write_str("The request was cancelled."),
        }
    }
}
impl std::error::Error for LlamaError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// `/health` is 200 and `/v1/models` lists the expected alias.
    Ready,
    Loading,
    Down,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Props {
    pub n_ctx: Option<u64>,
    pub total_slots: Option<u64>,
    pub build_info: Option<String>,
    pub model_path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub id: u64,
    pub n_ctx: Option<u64>,
    pub is_processing: bool,
}

/// Counters from `/metrics` (needs `--metrics`, which the supervisor always passes).
/// Only the names below are read; anything else in the text is ignored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    values: HashMap<&'static str, f64>,
}

const KNOWN_METRICS: [&str; 10] = [
    "prompt_tokens_total",
    "prompt_seconds_total",
    "tokens_predicted_total",
    "tokens_predicted_seconds_total",
    "n_decode_total",
    "prompt_tokens_seconds",
    "predicted_tokens_seconds",
    "requests_processing",
    "requests_deferred",
    "kv_cache_usage_ratio",
];

impl Metrics {
    /// A value by short name (without the `llamacpp:` prefix); `None` when not reported.
    pub fn get(&self, name: &str) -> Option<f64> {
        self.values.get(name).copied()
    }
}

pub fn parse_metrics(text: &str) -> Metrics {
    let mut values = HashMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("llamacpp:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let (Some(name), Some(raw)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(known) = KNOWN_METRICS.iter().find(|k| **k == name) else {
            continue;
        };
        if let Some(value) = raw.parse::<f64>().ok().filter(|v| v.is_finite()) {
            values.insert(*known, value);
        }
    }
    Metrics { values }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// The server's own timing block from the final stream event.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timings {
    pub prompt_n: Option<u64>,
    pub prompt_ms: Option<f64>,
    pub predicted_n: Option<u64>,
    pub predicted_ms: Option<f64>,
}

pub struct ChatRequest<'a> {
    /// The launch alias the server was started with.
    pub model: &'a str,
    pub prompt: &'a str,
    pub max_tokens: u32,
    pub seed: u32,
    pub temperature: f32,
    /// `false` re-processes the prompt every time (needed for comparable measurements).
    pub cache_prompt: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum StreamEvent<'a> {
    /// A non-empty `delta.content`. Network chunks are not tokens: one event may carry more
    /// than one token, and only the server's counters say how many.
    Content { at: Instant, text: &'a str },
    /// A non-empty reasoning delta.
    Reasoning { at: Instant },
}

#[derive(Clone, Debug, Default)]
pub struct StreamEnd {
    pub content_events: u64,
    pub reasoning_events: u64,
    pub first_content_after: Option<Duration>,
    pub elapsed: Duration,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
    pub timings: Option<Timings>,
}

pub struct LlamaClient {
    client: Client,
    base: String,
}

impl LlamaClient {
    pub fn new(port: u16) -> Result<Self, LlamaError> {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .referer(false)
            .connect_timeout(SHORT_TIMEOUT)
            .read_timeout(IDLE_TIMEOUT)
            .build()
            .map_err(|_| LlamaError::BadResponse)?;
        Ok(Self {
            client,
            base: format!("http://127.0.0.1:{port}"),
        })
    }

    async fn get(&self, path: &str) -> Result<Response, LlamaError> {
        self.client
            .get(format!("{}{path}", self.base))
            .timeout(SHORT_TIMEOUT)
            .send()
            .await
            .map_err(map_error)
    }

    /// Readiness with identity: `/health` 200 **and** `/v1/models` lists `alias`.
    pub async fn identity(&self, alias: &str) -> Health {
        match self.get("/health").await {
            Ok(r) if r.status().as_u16() == 200 => {}
            Ok(r) if r.status().as_u16() == 503 => return Health::Loading,
            _ => return Health::Down,
        }
        let Ok(response) = self.get("/v1/models").await else {
            return Health::Down;
        };
        if response.status().as_u16() != 200 {
            return Health::Loading;
        }
        let Ok(body) = read_bounded(response, MAX_SMALL_BODY).await else {
            return Health::Down;
        };
        let listed = serde_json::from_slice::<Value>(&body)
            .ok()
            .is_some_and(|v| {
                v.get("data")
                    .and_then(Value::as_array)
                    .is_some_and(|models| {
                        models
                            .iter()
                            .any(|m| m.get("id").and_then(Value::as_str) == Some(alias))
                    })
            });
        if listed { Health::Ready } else { Health::Down }
    }

    pub async fn props(&self) -> Result<Props, LlamaError> {
        let response = self.get("/props").await?;
        let value = json_body(response, MAX_SMALL_BODY).await?;
        let text = |v: Option<&Value>, max: usize| {
            v.and_then(Value::as_str)
                .map(|s| s.chars().take(max).collect::<String>())
        };
        Ok(Props {
            n_ctx: value
                .pointer("/default_generation_settings/n_ctx")
                .and_then(Value::as_u64),
            total_slots: value.get("total_slots").and_then(Value::as_u64),
            build_info: text(value.get("build_info"), 128),
            model_path: text(value.get("model_path"), 1024),
        })
    }

    /// `Unsupported` when the server has slot reporting disabled.
    pub async fn slots(&self) -> Result<Vec<Slot>, LlamaError> {
        let response = self.get("/slots").await?;
        let value = json_body(response, MAX_SMALL_BODY).await?;
        let slots = value.as_array().ok_or(LlamaError::BadResponse)?;
        Ok(slots
            .iter()
            .take(MAX_SLOTS)
            .filter_map(|s| {
                Some(Slot {
                    id: s.get("id")?.as_u64()?,
                    n_ctx: s.get("n_ctx").and_then(Value::as_u64),
                    is_processing: s.get("is_processing")?.as_bool()?,
                })
            })
            .collect())
    }

    pub async fn metrics(&self) -> Result<Metrics, LlamaError> {
        let response = self.get("/metrics").await?;
        check_status(&response)?;
        let body = read_bounded(response, MAX_METRICS_BODY).await?;
        Ok(parse_metrics(&String::from_utf8_lossy(&body)))
    }

    /// Whether a request is being processed: from the slots when available, otherwise from
    /// the request counter. `None` when the server reports neither (unknown, never false).
    pub async fn busy(&self) -> Option<bool> {
        if let Ok(slots) = self.slots().await {
            return Some(slots.iter().any(|s| s.is_processing));
        }
        let metrics = self.metrics().await.ok()?;
        Some(metrics.get("requests_processing")? > 0.0)
    }

    /// One streaming chat completion. `on_event` is called as events arrive; `cancel` is
    /// checked between events and every 100 ms while waiting. At most `deadline` in total.
    pub async fn stream_chat(
        &self,
        request: &ChatRequest<'_>,
        deadline: Duration,
        cancel: &AtomicBool,
        mut on_event: impl FnMut(StreamEvent<'_>),
    ) -> Result<StreamEnd, LlamaError> {
        let body = json!({
            "model": request.model,
            "messages": [{"role": "user", "content": request.prompt}],
            "stream": true,
            "stream_options": {"include_usage": true},
            "max_tokens": request.max_tokens,
            "seed": request.seed,
            "temperature": request.temperature,
            "cache_prompt": request.cache_prompt,
        });
        let started = Instant::now();
        let mut response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base))
            .header("content-type", "application/json")
            .body(body.to_string())
            .timeout(deadline)
            .send()
            .await
            .map_err(map_error)?;
        check_status(&response)?;

        let mut end = StreamEnd::default();
        let mut line: Vec<u8> = Vec::new();
        let mut total = 0usize;
        let mut done = false;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        while !done {
            if cancel.load(Ordering::Relaxed) {
                return Err(LlamaError::Cancelled);
            }
            let chunk = tokio::select! {
                chunk = response.chunk() => chunk.map_err(map_error)?,
                _ = tick.tick() => continue,
            };
            let Some(chunk) = chunk else { break };
            total += chunk.len();
            if total > MAX_STREAM_BYTES {
                return Err(LlamaError::TooLarge);
            }
            for &byte in chunk.iter() {
                if byte != b'\n' {
                    if line.len() >= MAX_LINE {
                        return Err(LlamaError::TooLarge);
                    }
                    line.push(byte);
                    continue;
                }
                let text = std::mem::take(&mut line);
                if handle_line(&text, &mut end, started, &mut on_event)? {
                    done = true;
                    break;
                }
            }
        }
        if !line.is_empty() && !done {
            handle_line(&line, &mut end, started, &mut on_event)?;
        }
        end.elapsed = started.elapsed();
        Ok(end)
    }
}

/// Returns true at `[DONE]`.
fn handle_line(
    raw: &[u8],
    end: &mut StreamEnd,
    started: Instant,
    on_event: &mut impl FnMut(StreamEvent<'_>),
) -> Result<bool, LlamaError> {
    let line = std::str::from_utf8(raw).map_err(|_| LlamaError::BadResponse)?;
    let line = line.trim_end_matches('\r');
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false); // comments, blank separators, other SSE fields
    };
    let data = data.trim_start();
    if data == "[DONE]" {
        return Ok(true);
    }
    if end.content_events + end.reasoning_events >= MAX_EVENTS {
        return Err(LlamaError::TooLarge);
    }
    let value: Value = serde_json::from_str(data).map_err(|_| LlamaError::BadResponse)?;
    if value.get("error").is_some() {
        return Err(LlamaError::BadResponse);
    }
    let at = Instant::now();
    let choice = value.pointer("/choices/0");
    if let Some(text) = choice
        .and_then(|c| c.pointer("/delta/content"))
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        end.content_events += 1;
        end.first_content_after
            .get_or_insert(at.duration_since(started));
        on_event(StreamEvent::Content { at, text });
    }
    if choice
        .and_then(|c| c.pointer("/delta/reasoning_content"))
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty())
    {
        end.reasoning_events += 1;
        on_event(StreamEvent::Reasoning { at });
    }
    if let Some(reason) = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
    {
        end.finish_reason = Some(reason.chars().take(32).collect());
    }
    if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
        end.usage = Some(Usage {
            prompt_tokens: usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            completion_tokens: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
    }
    if let Some(t) = value.get("timings").filter(|t| t.is_object()) {
        end.timings = Some(Timings {
            prompt_n: t.get("prompt_n").and_then(Value::as_u64),
            prompt_ms: t.get("prompt_ms").and_then(Value::as_f64),
            predicted_n: t.get("predicted_n").and_then(Value::as_u64),
            predicted_ms: t.get("predicted_ms").and_then(Value::as_f64),
        });
    }
    Ok(false)
}

fn map_error(error: reqwest::Error) -> LlamaError {
    if error.is_timeout() {
        LlamaError::Timeout
    } else {
        LlamaError::Unreachable
    }
}

fn check_status(response: &Response) -> Result<(), LlamaError> {
    match response.status().as_u16() {
        200 => Ok(()),
        404 | 501 => Err(LlamaError::Unsupported),
        code => Err(LlamaError::Status(code)),
    }
}

async fn read_bounded(mut response: Response, max: usize) -> Result<Vec<u8>, LlamaError> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(LlamaError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_error)? {
        if body.len() + chunk.len() > max {
            return Err(LlamaError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn json_body(response: Response, max: usize) -> Result<Value, LlamaError> {
    check_status(&response)?;
    let body = read_bounded(response, max).await?;
    serde_json::from_slice(&body).map_err(|_| LlamaError::BadResponse)
}

#[cfg(test)]
mod tests;
