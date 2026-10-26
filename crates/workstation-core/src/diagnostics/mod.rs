// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic advisor: four rules over a finished experiment. Each rule lists the measured
//! inputs it used and says what was missing when it could not run. A triggered rule never
//! claims a cause: it says what the measurements are consistent with and names the
//! controlled change that would test it.

use crate::experiments::{
    Experiment, Finding, FindingStatus, Ownership, PassKind, PassMetrics, Sampled, Settings,
    Spread, State, Variable,
};

/// Model memory held in system RAM, beyond which spill is suspected while video memory is in use.
pub const SPILL_RAM_BYTES: f64 = 256.0 * 1024.0 * 1024.0;
pub const FALLBACK_GPU_UTILIZATION_PERCENT: f64 = 15.0;
pub const FALLBACK_CPU_SHARE: f64 = 0.70;
pub const SLOW_PROMPT_TTFT_MS: f64 = 3000.0;
pub const SLOW_PROMPT_TOKENS: f64 = 2048.0;
pub const JITTER_RATIO: f64 = 2.5;

/// Facts about the model and machine that the experiment record does not hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// Layers the model can place on a GPU: its block count plus the output layer.
    pub offloadable_layers: Option<u32>,
    /// Shared CPU/GPU memory (an APU); spill between pools is not meaningful there.
    pub unified_memory: Option<bool>,
}

/// One finding per rule, always in the order DIAG-01 to DIAG-04.
pub fn evaluate(experiment: &Experiment, context: &Context) -> Vec<Finding> {
    let ok: Vec<&PassMetrics> = experiment
        .passes
        .iter()
        .filter(|p| p.kind == PassKind::Measured && p.state == State::Completed)
        .filter_map(|p| p.metrics.as_ref())
        .collect();
    if ok.is_empty() {
        return RULES
            .iter()
            .map(|id| {
                finding(
                    id,
                    FindingStatus::NotEvaluated,
                    vec![],
                    vec!["a completed measured pass".into()],
                    None,
                    "Not evaluated: no measured pass completed.".into(),
                )
            })
            .collect();
    }
    vec![
        spill(experiment, context, &ok),
        cpu_fallback(experiment, &ok),
        slow_prompt(&ok),
        jitter(&ok),
    ]
}

const RULES: [&str; 4] = ["DIAG-01", "DIAG-02", "DIAG-03", "DIAG-04"];

fn finding(
    rule: &str,
    status: FindingStatus,
    evidence: Vec<String>,
    missing: Vec<String>,
    follow_up: Option<Variable>,
    message: String,
) -> Finding {
    Finding {
        rule_id: rule.into(),
        status,
        warning: status == FindingStatus::Triggered,
        evidence,
        missing,
        follow_up,
        message,
    }
}

fn not_applicable(rule: &str, why: &str) -> Finding {
    finding(
        rule,
        FindingStatus::NotApplicable,
        vec![],
        vec![],
        None,
        format!("Not applicable: {why}"),
    )
}

fn not_evaluated(rule: &str, missing: Vec<String>) -> Finding {
    let message = format!("Not evaluated: missing {}.", missing.join(", "));
    finding(
        rule,
        FindingStatus::NotEvaluated,
        vec![],
        missing,
        None,
        message,
    )
}

fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    Spread::of(&values.collect::<Vec<_>>()).map(|s| s.median)
}

fn sampled_peak(
    metrics: &[&PassMetrics],
    pick: fn(&PassMetrics) -> Option<Sampled>,
) -> Option<f64> {
    median(metrics.iter().filter_map(|m| pick(m).map(|s| s.peak)))
}

fn sampled_mean(
    metrics: &[&PassMetrics],
    pick: fn(&PassMetrics) -> Option<Sampled>,
) -> Option<f64> {
    median(metrics.iter().filter_map(|m| pick(m).map(|s| s.mean)))
}

fn mib(bytes: f64) -> String {
    format!("{:.0} MiB", bytes / (1024.0 * 1024.0))
}

/// Backend the profile asked for: `Some(false)` for CPU only, `None` when not recorded.
fn gpu_requested(experiment: &Experiment) -> Option<bool> {
    if experiment.spec.requested.gpu_layers == Some(0) {
        return Some(false);
    }
    experiment
        .spec
        .backend
        .as_deref()
        .map(|b| !b.eq_ignore_ascii_case("cpu"))
}

fn observed(experiment: &Experiment) -> Option<&Settings> {
    experiment.observed.as_ref()
}

fn spill(experiment: &Experiment, context: &Context, ok: &[&PassMetrics]) -> Finding {
    const RULE: &str = "DIAG-01";
    if gpu_requested(experiment) == Some(false) {
        return not_applicable(RULE, "the profile does not use a GPU.");
    }
    if context.unified_memory == Some(true) {
        return not_applicable(
            RULE,
            "this machine shares one memory pool between CPU and GPU.",
        );
    }
    let vram = sampled_peak(ok, |m| m.gpu_memory_bytes);
    let rss = sampled_peak(ok, |m| m.process_rss_bytes);
    let layers = observed(experiment).and_then(|o| o.gpu_layers);
    let mut evidence = Vec::new();
    let mut triggered = false;
    let mut evaluated = false;

    if let (Some(vram), Some(rss)) = (vram, rss) {
        evaluated = true;
        evidence.push(format!("gpu_memory_peak: {}", mib(vram)));
        evidence.push(format!("process_rss_peak: {}", mib(rss)));
        triggered |= vram > 0.0 && rss > SPILL_RAM_BYTES;
    }
    if let (Some(layers), Some(offloadable)) = (layers, context.offloadable_layers) {
        evaluated = true;
        evidence.push(format!("offloaded_layers: {layers} of {offloadable}"));
        triggered |= layers < offloadable;
    }
    if !evaluated {
        let mut missing = Vec::new();
        if vram.is_none() {
            missing.push("GPU memory reading".to_string());
        }
        if rss.is_none() {
            missing.push(match experiment.spec.ownership {
                Ownership::Attached => "process memory (attached runtime)".to_string(),
                Ownership::Managed => "process memory reading".to_string(),
            });
        }
        if layers.is_none() {
            missing.push("offloaded layers reported by the runtime".into());
        }
        if context.offloadable_layers.is_none() {
            missing.push("model layer count".into());
        }
        return not_evaluated(RULE, missing);
    }
    let (status, follow_up, message) = if triggered {
        (
            FindingStatus::Triggered,
            Some(Variable::CacheTypeQ8),
            "Consistent with part of the model or its working memory being held in system RAM while the GPU is also in use. A smaller K/V cache would test this.".to_string(),
        )
    } else {
        (
            FindingStatus::Clear,
            None,
            "The readings do not suggest memory spill between GPU and system RAM.".to_string(),
        )
    };
    finding(RULE, status, evidence, vec![], follow_up, message)
}

fn cpu_fallback(experiment: &Experiment, ok: &[&PassMetrics]) -> Finding {
    const RULE: &str = "DIAG-02";
    match gpu_requested(experiment) {
        Some(false) => return not_applicable(RULE, "the profile does not use a GPU."),
        None => return not_evaluated(RULE, vec!["requested backend".into()]),
        Some(true) => {}
    }
    let gpu = sampled_mean(ok, |m| m.gpu_utilization_percent);
    let cpu_ms = median(ok.iter().filter_map(|m| m.process_cpu_ms.map(|v| v as f64)));
    let wall = median(ok.iter().map(|m| m.wall_ms as f64));
    let mut missing = Vec::new();
    if gpu.is_none() {
        missing.push("GPU utilization reading".to_string());
    }
    if cpu_ms.is_none() || experiment.spec.ownership == Ownership::Attached {
        missing.push("owned process CPU time (needs a managed runtime)".to_string());
    }
    let (Some(gpu), Some(cpu_ms), Some(wall), true) =
        (gpu, cpu_ms, wall.filter(|w| *w > 0.0), missing.is_empty())
    else {
        return not_evaluated(RULE, missing);
    };
    let share = cpu_ms / wall;
    let evidence = vec![
        format!("gpu_utilization_mean: {gpu:.0}%"),
        format!(
            "process_cpu_time: {cpu_ms:.0} ms of {wall:.0} ms wall ({:.0}%)",
            share * 100.0
        ),
    ];
    if gpu < FALLBACK_GPU_UTILIZATION_PERCENT && share > FALLBACK_CPU_SHARE {
        finding(
            RULE,
            FindingStatus::Triggered,
            evidence,
            vec![],
            None,
            "Consistent with the work running on the CPU although a GPU backend was requested: the GPU was mostly idle while the runtime process was busy. Check that this llama.cpp build includes your GPU backend and that layers are offloaded.".into(),
        )
    } else {
        finding(
            RULE,
            FindingStatus::Clear,
            evidence,
            vec![],
            None,
            "The GPU was in use, or the runtime process was not CPU-bound.".into(),
        )
    }
}

fn slow_prompt(ok: &[&PassMetrics]) -> Finding {
    const RULE: &str = "DIAG-03";
    let ttft = median(ok.iter().filter_map(|m| m.ttft_ms));
    let tokens = median(ok.iter().filter_map(|m| m.prompt_tokens.map(|t| t as f64)));
    let mut missing = Vec::new();
    if ttft.is_none() {
        missing.push("time to first token".to_string());
    }
    if tokens.is_none() {
        missing.push("prompt token count".to_string());
    }
    let (Some(ttft), Some(tokens)) = (ttft, tokens) else {
        return not_evaluated(RULE, missing);
    };
    if tokens <= SLOW_PROMPT_TOKENS {
        return not_applicable(
            RULE,
            &format!(
                "the prompt has {tokens:.0} tokens; the rule needs more than {SLOW_PROMPT_TOKENS:.0}."
            ),
        );
    }
    let evidence = vec![
        format!("warm_ttft: {ttft:.0} ms"),
        format!("prompt_tokens: {tokens:.0}"),
    ];
    if ttft > SLOW_PROMPT_TTFT_MS {
        finding(
            RULE,
            FindingStatus::Triggered,
            evidence,
            vec![],
            Some(Variable::FlashAttentionOn),
            "Consistent with slow prompt processing on a long prompt. Flash attention would test whether attention cost is the cause.".into(),
        )
    } else {
        finding(
            RULE,
            FindingStatus::Clear,
            evidence,
            vec![],
            None,
            "Prompt processing time is within the expected range for this prompt length.".into(),
        )
    }
}

fn jitter(ok: &[&PassMetrics]) -> Finding {
    const RULE: &str = "DIAG-04";
    let ratios: Vec<f64> = ok
        .iter()
        .filter_map(|m| m.itl)
        .filter(|i| i.median_ms > 0.0)
        .map(|i| i.p95_ms / i.median_ms)
        .collect();
    let Some(ratio) = median(ratios.into_iter()) else {
        let reason = ok
            .iter()
            .find_map(|m| m.itl_unknown_reason.clone())
            .unwrap_or_else(|| "inter-token latency".into());
        return not_evaluated(RULE, vec![format!("valid inter-token latency ({reason})")]);
    };
    let evidence = vec![format!("itl_p95_over_median: {ratio:.2}")];
    if ratio > JITTER_RATIO {
        finding(
            RULE,
            FindingStatus::Triggered,
            evidence,
            vec![],
            None,
            "Consistent with uneven token generation: the slowest tokens take much longer than a typical one. Other programs, memory pressure or thermal limits can each produce this; the measurements alone do not say which.".into(),
        )
    } else {
        finding(
            RULE,
            FindingStatus::Clear,
            evidence,
            vec![],
            None,
            "Token generation was steady.".into(),
        )
    }
}

#[cfg(test)]
mod tests;
