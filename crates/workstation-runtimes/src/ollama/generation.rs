// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded local-only generation; declarations alone never authorize execution.

use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::watch;

use super::{InstalledModel, ModelMetadata, Models, OllamaAdapter, Version};

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 8192;
const STREAM_TIMEOUT: Duration = Duration::from_secs(180);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default)]
pub struct GenerationProgress {
    /// Client time to the first nonempty response text, not a per-token measurement.
    pub first_output_after: Option<Duration>,
    pub response_chunks: u64,
}

#[derive(Clone, Debug)]
pub struct GenerationSummary {
    pub model_name: String,
    pub model_digest: String,
    pub runtime_version: String,
    pub first_output_after: Option<Duration>,
    pub elapsed: Duration,
    pub response_chunks: u64,
    pub eval_count: Option<u64>,
    pub eval_duration_ns: Option<u64>,
    pub load_duration_ns: Option<u64>,
    pub done_reason: String,
}

pub struct GenerationRequest<'a> {
    pub prompt: &'a str,
    pub num_predict: u32,
    pub num_ctx: u32,
    pub seed: u32,
}

#[derive(Deserialize)]
struct Status {
    cloud: CloudStatus,
}

#[derive(Deserialize)]
struct CloudStatus {
    disabled: bool,
}

#[derive(Deserialize)]
struct Record {
    model: Option<String>,
    done: Option<bool>,
    response: Option<String>,
    done_reason: Option<String>,
    error: Option<serde_json::Value>,
    remote_model: Option<String>,
    remote_host: Option<String>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
    load_duration: Option<u64>,
}

impl OllamaAdapter {
    pub async fn generate_local(
        &self,
        selected: &InstalledModel,
        request: GenerationRequest<'_>,
        progress: watch::Sender<GenerationProgress>,
    ) -> Result<GenerationSummary, String> {
        self.generate_local_with_timeouts(selected, request, progress, STREAM_TIMEOUT, IDLE_TIMEOUT)
            .await
    }

    async fn generate_local_with_timeouts(
        &self,
        selected: &InstalledModel,
        request: GenerationRequest<'_>,
        progress: watch::Sender<GenerationProgress>,
        total_timeout: Duration,
        idle_timeout: Duration,
    ) -> Result<GenerationSummary, String> {
        let digest = canonical_digest(selected.digest.as_deref()).ok_or(
            "Local-only check: a full SHA-256 manifest digest is required. Refresh Runtimes.",
        )?;
        if selected.name.trim().is_empty()
            || selected.name.len() > 1024
            || selected.name.chars().any(char::is_control)
        {
            return Err("Local-only check: invalid model name.".into());
        }
        if selected.cloud_routed() || cloud_name(&selected.name) {
            return Err("Local-only check: cloud-routed models cannot be benchmarked.".into());
        }
        if request.prompt.is_empty()
            || request.prompt.len() > 4096
            || !(1..=128).contains(&request.num_predict)
            || !(1..=2048).contains(&request.num_ctx)
        {
            return Err("Invalid quick benchmark workload.".into());
        }
        let status: Status = self.get("/api/status").await.map_err(|_| {
            "Local-only check unavailable. Use an Ollama version supporting cloud status and local digest references, with cloud features disabled separately.".to_string()
        })?;
        if !status.cloud.disabled {
            return Err("Local-only check: Ollama cloud features must be disabled separately before benchmarking. The application will not change your service.".into());
        }
        let version: Version = self.get("/api/version").await?;
        if version.version.trim().is_empty() {
            return Err("/api/version: empty version".into());
        }
        let installed: Models<InstalledModel> = self.get("/api/tags").await?;
        let mut matches = installed
            .models
            .iter()
            .filter(|model| model.name == selected.name);
        let current = matches
            .next()
            .ok_or("Model is no longer installed. Refresh Runtimes.")?;
        if matches.next().is_some()
            || canonical_digest(current.digest.as_deref()).as_ref() != Some(&digest)
            || current.cloud_routed()
            || cloud_name(&current.name)
        {
            return Err(
                "Model identity or routing changed. Refresh Runtimes before benchmarking.".into(),
            );
        }
        // The server resolves this immutable manifest locally; never retry with a mutable tag.
        let local_reference = format!("{digest}:local");
        let show = self
            .client
            .post(self.url("/api/show"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::json!({"model":local_reference,"verbose":false}).to_string());
        let metadata: ModelMetadata = self.request("/api/show", show).await.map_err(|_| {
            "Local-only model inspection failed. This Ollama must support local digest references; no tag fallback is allowed.".to_string()
        })?;
        if metadata.cloud_routed()
            || !metadata
                .capabilities
                .as_ref()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "completion"))
        {
            return Err(
                "Local-only check: model routing or text-completion support is unresolved.".into(),
            );
        }
        let body = serde_json::json!({
            "model":local_reference,"prompt":request.prompt,"stream":true,"think":false,
            "options":{"num_predict":request.num_predict,"num_ctx":request.num_ctx,
                "seed":request.seed,"temperature":0}
        });
        let started = Instant::now();
        let mut response = tokio::time::timeout(
            idle_timeout,
            self.client
                .post(self.url("/api/generate"))
                .timeout(total_timeout)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string())
                .send(),
        )
        .await
        .map_err(|_| "/api/generate: response headers timed out")?
        .map_err(|_| "/api/generate: request failed or timed out")?;
        if !response.status().is_success() {
            return Err(format!(
                "/api/generate: HTTP {}",
                response.status().as_u16()
            ));
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| {
                !value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/x-ndjson"))
            })
        {
            return Err("/api/generate: expected an NDJSON stream".into());
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_STREAM_BYTES as u64)
        {
            return Err("/api/generate: stream exceeds 4 MiB limit".into());
        }
        let mut parser = StreamParser::new(&local_reference);
        while let Some(chunk) = tokio::time::timeout(idle_timeout, response.chunk())
            .await
            .map_err(|_| "/api/generate: stream idle timeout")?
            .map_err(|_| "/api/generate: stream failed or timed out")?
        {
            parser.feed(&chunk, started.elapsed(), &progress)?;
        }
        parser.finish(
            started.elapsed(),
            selected.name.clone(),
            digest,
            version.version,
            &progress,
        )
    }
}

fn canonical_digest(digest: Option<&str>) -> Option<String> {
    workstation_core::models::artifact::canonical_manifest_digest(digest?)
}

fn cloud_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(":cloud") || name.ends_with("-cloud")
}

struct StreamParser<'a> {
    reference: &'a str,
    line: Vec<u8>,
    bytes: usize,
    records: usize,
    progress: GenerationProgress,
    terminal: Option<Record>,
}

impl<'a> StreamParser<'a> {
    fn new(reference: &'a str) -> Self {
        Self {
            reference,
            line: Vec::new(),
            bytes: 0,
            records: 0,
            progress: GenerationProgress::default(),
            terminal: None,
        }
    }

    fn feed(
        &mut self,
        bytes: &[u8],
        elapsed: Duration,
        progress: &watch::Sender<GenerationProgress>,
    ) -> Result<(), String> {
        if bytes.len() > MAX_STREAM_BYTES - self.bytes {
            return Err("/api/generate: stream exceeds 4 MiB limit".into());
        }
        self.bytes += bytes.len();
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if part.len() > MAX_LINE_BYTES - self.line.len() {
                return Err("/api/generate: record exceeds 64 KiB limit".into());
            }
            self.line.extend_from_slice(part);
            if part.last() == Some(&b'\n') {
                self.record(elapsed, progress)?;
                self.line.clear();
            }
        }
        Ok(())
    }

    fn record(
        &mut self,
        elapsed: Duration,
        progress: &watch::Sender<GenerationProgress>,
    ) -> Result<(), String> {
        if self.line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        self.records += 1;
        if self.records > MAX_RECORDS {
            return Err("/api/generate: too many stream records".into());
        }
        if self.terminal.is_some() {
            return Err("/api/generate: data after terminal record".into());
        }
        let record: Record = serde_json::from_slice(&self.line)
            .map_err(|_| "/api/generate: invalid stream record")?;
        if record.error.is_some() {
            return Err("/api/generate: runtime reported a generation error".into());
        }
        if record.model.as_deref() != Some(self.reference)
            || [&record.remote_host, &record.remote_model]
                .into_iter()
                .flatten()
                .any(|value| !value.trim().is_empty())
        {
            return Err(
                "/api/generate: response identity or local routing could not be established".into(),
            );
        }
        let done = record
            .done
            .ok_or("/api/generate: missing completion state")?;
        if record
            .response
            .as_ref()
            .is_some_and(|text| !text.is_empty())
        {
            self.progress.first_output_after.get_or_insert(elapsed);
            self.progress.response_chunks += 1;
            progress.send_replace(self.progress);
        }
        if done {
            if !matches!(record.done_reason.as_deref(), Some("stop" | "length")) {
                return Err("/api/generate: unrecognized completion reason".into());
            }
            self.terminal = Some(record);
        }
        Ok(())
    }

    fn finish(
        mut self,
        elapsed: Duration,
        model_name: String,
        model_digest: String,
        runtime_version: String,
        progress: &watch::Sender<GenerationProgress>,
    ) -> Result<GenerationSummary, String> {
        // A final JSON record without a newline is valid NDJSON at EOF.
        if !self.line.is_empty() {
            self.record(elapsed, progress)?;
        }
        let terminal = self
            .terminal
            .ok_or("/api/generate: stream ended before completion")?;
        if self.progress.first_output_after.is_none() {
            return Err("/api/generate: completed without response text".into());
        }
        Ok(GenerationSummary {
            model_name,
            model_digest,
            runtime_version,
            first_output_after: self.progress.first_output_after,
            elapsed,
            response_chunks: self.progress.response_chunks,
            eval_count: terminal.eval_count,
            eval_duration_ns: terminal.eval_duration,
            load_duration_ns: terminal.load_duration,
            done_reason: terminal.done_reason.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests;
