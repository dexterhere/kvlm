// SPDX-License-Identifier: GPL-3.0-or-later
//! Benchmark Lab records: what was asked for, what the runtime reported, and what was
//! measured, with the scope and coverage of every measurement. Unmeasured values stay `None`.

use serde::{Deserialize, Serialize};

pub mod follow_up;
pub mod summary;
pub use summary::{Spread, Summary, summarize};

/// Longest accepted text field, so a record cannot grow without bound.
pub const MAX_TEXT: usize = 512;
pub const MAX_PASSES: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Completed,
    Failed,
    Cancelled,
    /// Found still `Running` at the next start: the application ended mid-run.
    Interrupted,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }
    pub fn is_terminal(self) -> bool {
        self != Self::Running
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassKind {
    /// Unload then load, timed until the runtime reports ready.
    ColdLoad,
    /// Fixed prompt, discarded.
    Warmup,
    Measured,
}

impl PassKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ColdLoad => "cold_load",
            Self::Warmup => "warmup",
            Self::Measured => "measured",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "cold_load" => Self::ColdLoad,
            "warmup" => Self::Warmup,
            "measured" => Self::Measured,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Llamacpp,
    Ollama,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Llamacpp => "llamacpp",
            Self::Ollama => "ollama",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "llamacpp" => Self::Llamacpp,
            "ollama" => Self::Ollama,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Managed,
    Attached,
}

impl Ownership {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Attached => "attached",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "managed" => Self::Managed,
            "attached" => Self::Attached,
            _ => return None,
        })
    }
}

/// What a measurement covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// One GPU or other device.
    Device,
    /// The whole system, other programs included.
    System,
    /// Only the processes this application started.
    OwnedProcessGroup,
}

/// How well a sampled series covers the pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub scope: Scope,
    pub samples: u32,
    pub covered_ms: u64,
    pub longest_gap_ms: u64,
    /// The sample cap was reached, so the end of the pass may be missing.
    pub capped: bool,
}

/// A sampled quantity: peak and mean over the pass, with its coverage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sampled {
    pub peak: f64,
    pub mean: f64,
    pub coverage: Coverage,
}

/// Inter-token latency, present only when the stream proved one token per event.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Itl {
    pub intervals: u32,
    pub median_ms: f64,
    pub p95_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PassMetrics {
    pub wall_ms: u64,
    pub ttft_ms: Option<f64>,
    pub prompt_tokens: Option<u64>,
    pub prompt_tokens_per_s: Option<f64>,
    pub generated_tokens: Option<u64>,
    /// From the server's own counters, never chunks divided by seconds.
    pub decode_tokens_per_s: Option<f64>,
    pub itl: Option<Itl>,
    /// Why ITL is unknown, when it is.
    pub itl_unknown_reason: Option<String>,
    /// Resident memory of the owned process group (managed runtimes only), in bytes.
    pub process_rss_bytes: Option<Sampled>,
    pub gpu_memory_bytes: Option<Sampled>,
    pub gpu_utilization_percent: Option<Sampled>,
    pub cpu_utilization_percent: Option<Sampled>,
    /// CPU time of the owned process group during the pass.
    pub process_cpu_ms: Option<u64>,
    pub offloaded_layers: Option<u32>,
    /// Model memory the runtime reports as held in system RAM (not the process's resident
    /// size, which also counts mapped files and the runtime itself), in bytes.
    pub model_ram_bytes: Option<u64>,
    /// Time from unload to ready, for cold-load passes.
    pub load_ms: Option<f64>,
}

/// Settings the profile asked for, as stored text; parsed by the layer that made them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub context_length: Option<u32>,
    pub gpu_layers: Option<u32>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub flash_attention: Option<bool>,
    pub ubatch_size: Option<u32>,
    pub threads: Option<u32>,
    pub parallel: Option<u32>,
    pub no_mmap: Option<bool>,
    pub mlock: Option<bool>,
}

impl Settings {
    /// Names of the settings that differ where both sides know the value. A value only one
    /// side reports is not a difference; it is unknown.
    pub fn differences(&self, other: &Self) -> Vec<&'static str> {
        fn differ<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
            matches!((a, b), (Some(x), Some(y)) if x != y)
        }
        let mut out = Vec::new();
        if differ(&self.context_length, &other.context_length) {
            out.push("context_length");
        }
        if differ(&self.gpu_layers, &other.gpu_layers) {
            out.push("gpu_layers");
        }
        if differ(&self.cache_type_k, &other.cache_type_k) {
            out.push("cache_type_k");
        }
        if differ(&self.cache_type_v, &other.cache_type_v) {
            out.push("cache_type_v");
        }
        if differ(&self.flash_attention, &other.flash_attention) {
            out.push("flash_attention");
        }
        if differ(&self.ubatch_size, &other.ubatch_size) {
            out.push("ubatch_size");
        }
        if differ(&self.threads, &other.threads) {
            out.push("threads");
        }
        if differ(&self.parallel, &other.parallel) {
            out.push("parallel");
        }
        if differ(&self.no_mmap, &other.no_mmap) {
            out.push("no_mmap");
        }
        if differ(&self.mlock, &other.mlock) {
            out.push("mlock");
        }
        out
    }

    /// Names of the settings that are not identical, counting a value known on one side only
    /// as a difference. A controlled comparison needs this: a flag that appears or vanishes
    /// is a change, not an unknown.
    pub fn all_differences(&self, other: &Self) -> Vec<&'static str> {
        let mut out = self.differences(other);
        let one_sided = [
            (
                "context_length",
                self.context_length.is_some() != other.context_length.is_some(),
            ),
            (
                "gpu_layers",
                self.gpu_layers.is_some() != other.gpu_layers.is_some(),
            ),
            (
                "cache_type_k",
                self.cache_type_k.is_some() != other.cache_type_k.is_some(),
            ),
            (
                "cache_type_v",
                self.cache_type_v.is_some() != other.cache_type_v.is_some(),
            ),
            (
                "flash_attention",
                self.flash_attention.is_some() != other.flash_attention.is_some(),
            ),
            (
                "ubatch_size",
                self.ubatch_size.is_some() != other.ubatch_size.is_some(),
            ),
            ("threads", self.threads.is_some() != other.threads.is_some()),
            (
                "parallel",
                self.parallel.is_some() != other.parallel.is_some(),
            ),
            ("no_mmap", self.no_mmap.is_some() != other.no_mmap.is_some()),
            ("mlock", self.mlock.is_some() != other.mlock.is_some()),
        ];
        out.extend(one_sided.into_iter().filter(|(_, d)| *d).map(|(n, _)| n));
        out
    }
}

/// The one setting a controlled comparison changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variable {
    CacheTypeQ8,
    HalfContext,
    FlashAttentionOn,
}

impl Variable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CacheTypeQ8 => "cache_type_q8",
            Self::HalfContext => "half_context",
            Self::FlashAttentionOn => "flash_attention_on",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "cache_type_q8" => Self::CacheTypeQ8,
            "half_context" => Self::HalfContext,
            "flash_attention_on" => Self::FlashAttentionOn,
            _ => return None,
        })
    }
}

/// Everything fixed when an experiment starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewExperiment {
    pub engine: Engine,
    pub ownership: Ownership,
    pub cold: bool,
    pub passes_requested: u32,
    pub workload_id: String,
    pub prompt_sha256: String,
    pub seed: u32,
    pub max_tokens: u32,
    pub model_name: String,
    pub model_sha256: Option<String>,
    pub runtime_version: Option<String>,
    pub backend: Option<String>,
    /// `HardwareProfile::profile_hash`; comparisons need it on both sides.
    pub hardware_hash: Option<String>,
    pub requested: Settings,
    pub variable: Option<Variable>,
    /// The experiment this one is a controlled change of.
    pub baseline_id: Option<i64>,
}

impl NewExperiment {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_PASSES).contains(&self.passes_requested) {
            return Err("Passes must be between 1 and 20.".into());
        }
        if self.prompt_sha256.len() != 64
            || !self.prompt_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("The prompt digest must be 64 hex characters.".into());
        }
        for text in [&self.workload_id, &self.model_name] {
            if text.is_empty() || text.len() > MAX_TEXT || text.chars().any(char::is_control) {
                return Err("Workload and model names must be short plain text.".into());
            }
        }
        for text in [
            self.model_sha256.as_deref(),
            self.runtime_version.as_deref(),
            self.backend.as_deref(),
            self.hardware_hash.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if text.is_empty() || text.len() > MAX_TEXT || text.chars().any(char::is_control) {
                return Err("A stored field is empty, too long or has control characters.".into());
            }
        }
        if self.variable.is_some() != self.baseline_id.is_some() {
            return Err("A controlled change needs both its variable and its baseline.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pass {
    pub id: i64,
    pub kind: PassKind,
    pub ordinal: u32,
    pub state: State,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
    pub metrics: Option<PassMetrics>,
    pub failure: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    Triggered,
    Clear,
    /// An input was missing; the finding names it.
    NotEvaluated,
    NotApplicable,
}

impl FindingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Triggered => "triggered",
            Self::Clear => "clear",
            Self::NotEvaluated => "not_evaluated",
            Self::NotApplicable => "not_applicable",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "triggered" => Self::Triggered,
            "clear" => Self::Clear,
            "not_evaluated" => Self::NotEvaluated,
            "not_applicable" => Self::NotApplicable,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// `DIAG-01` to `DIAG-04`.
    pub rule_id: String,
    pub status: FindingStatus,
    pub warning: bool,
    /// Measured inputs the rule used, as `name: value` lines.
    pub evidence: Vec<String>,
    /// Inputs that were missing, when the rule could not be evaluated.
    pub missing: Vec<String>,
    pub follow_up: Option<Variable>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub id: i64,
    pub state: State,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
    pub spec: NewExperiment,
    /// What the runtime reported back; `None` when it reported nothing.
    pub observed: Option<Settings>,
    pub summary: Option<Summary>,
    pub failure: Option<String>,
    pub passes: Vec<Pass>,
    pub findings: Vec<Finding>,
}

#[cfg(test)]
mod tests;
