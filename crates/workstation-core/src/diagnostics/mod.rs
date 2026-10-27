// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic advisor: four rules over a finished experiment. Each rule lists the measured
//! inputs it used and says what was missing when it could not run. A triggered rule never
//! claims a cause: it says what the measurements are consistent with and names the
//! controlled change that would test it.
//!
//! A rule only combines inputs that belong to the same pass. Passes missing any input of a
//! rule are left out of that rule, and the figure used is the median over the rest.

use crate::experiments::{
    Experiment, Finding, FindingStatus, Ownership, PassKind, PassMetrics, Spread, State, Variable,
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
    /// `None` when the machine's memory topology is not known.
    pub unified_memory: Option<bool>,
}

const RULES: [&str; 4] = ["DIAG-01", "DIAG-02", "DIAG-03", "DIAG-04"];

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
            .map(|id| not_evaluated(id, vec!["a completed measured pass".into()]))
            .collect();
    }
    vec![
        spill(experiment, context, &ok),
        cpu_fallback(experiment, &ok),
        slow_prompt(&ok),
        jitter(&ok),
    ]
}

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

/// Median of the finite values; `None` when there are none.
fn median(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    Spread::of(&values.into_iter().collect::<Vec<_>>()).map(|s| s.median)
}

fn finite(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
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

fn spill(experiment: &Experiment, context: &Context, ok: &[&PassMetrics]) -> Finding {
    const RULE: &str = "DIAG-01";
    if gpu_requested(experiment) == Some(false) {
        return not_applicable(RULE, "the profile does not use a GPU.");
    }
    match context.unified_memory {
        Some(true) => {
            return not_applicable(
                RULE,
                "this machine shares one memory pool between CPU and GPU.",
            );
        }
        None => return not_evaluated(RULE, vec!["whether CPU and GPU share memory".into()]),
        Some(false) => {}
    }

    let mut evidence = Vec::new();
    let mut unchecked = Vec::new();
    let mut triggered = false;
    let mut checked = false;

    // Signal A: video memory in use and model memory held in system RAM, in the same pass.
    let pairs: Vec<(f64, f64)> = ok
        .iter()
        .filter_map(|m| {
            let vram = m.gpu_memory_bytes.map(|s| s.peak).and_then(finite)?;
            let ram = m.model_ram_bytes? as f64;
            Some((vram, ram))
        })
        .collect();
    if let (Some(vram), Some(ram)) = (
        median(pairs.iter().map(|p| p.0)),
        median(pairs.iter().map(|p| p.1)),
    ) {
        checked = true;
        evidence.push(format!("gpu_memory_peak: {}", mib(vram)));
        evidence.push(format!("model_memory_in_system_ram: {}", mib(ram)));
        triggered |= vram > 0.0 && ram > SPILL_RAM_BYTES;
    } else {
        unchecked.push("model memory held in system RAM (as reported by the runtime)".to_string());
    }

    // Signal B: layers the runtime says it offloaded against the layers the model can offload.
    let layers =
        median(ok.iter().filter_map(|m| m.offloaded_layers).map(f64::from)).or_else(|| {
            experiment
                .observed
                .as_ref()
                .and_then(|o| o.gpu_layers)
                .map(f64::from)
        });
    match (layers, context.offloadable_layers) {
        (Some(layers), Some(offloadable)) => {
            checked = true;
            evidence.push(format!("offloaded_layers: {layers:.0} of {offloadable}"));
            triggered |= layers < f64::from(offloadable);
        }
        (layers, offloadable) => {
            if layers.is_none() {
                unchecked.push("offloaded layers reported by the runtime".to_string());
            }
            if offloadable.is_none() {
                unchecked.push("the model's layer count".to_string());
            }
        }
    }

    if !checked {
        return not_evaluated(RULE, unchecked);
    }
    if triggered {
        return finding(
            RULE,
            FindingStatus::Triggered,
            evidence,
            unchecked,
            Some(Variable::CacheTypeQ8),
            "Consistent with part of the model or its working memory being held in system RAM while the GPU is also in use. A smaller K/V cache would test this.".into(),
        );
    }
    let message = if unchecked.is_empty() {
        "The readings do not suggest memory spill between GPU and system RAM.".to_string()
    } else {
        format!(
            "The readings available do not suggest memory spill. Not checked: {}.",
            unchecked.join(", ")
        )
    };
    finding(
        RULE,
        FindingStatus::Clear,
        evidence,
        unchecked,
        None,
        message,
    )
}

fn cpu_fallback(experiment: &Experiment, ok: &[&PassMetrics]) -> Finding {
    const RULE: &str = "DIAG-02";
    match gpu_requested(experiment) {
        Some(false) => return not_applicable(RULE, "the profile does not use a GPU."),
        None => return not_evaluated(RULE, vec!["requested backend".into()]),
        Some(true) => {}
    }
    if experiment.spec.ownership == Ownership::Attached {
        return not_evaluated(
            RULE,
            vec!["owned process CPU time (needs a managed runtime)".into()],
        );
    }
    // Per pass: GPU utilization and the process's CPU share of that pass's own wall time.
    let passes: Vec<(f64, f64)> = ok
        .iter()
        .filter_map(|m| {
            let gpu = m.gpu_utilization_percent.map(|s| s.mean).and_then(finite)?;
            let cpu_ms = m.process_cpu_ms? as f64;
            (m.wall_ms > 0).then_some((gpu, cpu_ms / m.wall_ms as f64))
        })
        .collect();
    let (Some(gpu), Some(share)) = (
        median(passes.iter().map(|p| p.0)),
        median(passes.iter().map(|p| p.1)),
    ) else {
        let mut missing = Vec::new();
        if !ok.iter().any(|m| m.gpu_utilization_percent.is_some()) {
            missing.push("GPU utilization reading".to_string());
        }
        if !ok.iter().any(|m| m.process_cpu_ms.is_some()) {
            missing.push("owned process CPU time".to_string());
        }
        if missing.is_empty() {
            missing.push("a pass with both GPU utilization and process CPU time".into());
        }
        return not_evaluated(RULE, missing);
    };
    let evidence = vec![
        format!("gpu_utilization_mean: {gpu:.0}%"),
        format!("process_cpu_share_of_wall_time: {:.0}%", share * 100.0),
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
    let passes: Vec<(f64, f64)> = ok
        .iter()
        .filter_map(|m| {
            let ttft = m.ttft_ms.and_then(finite)?;
            Some((ttft, m.prompt_tokens? as f64))
        })
        .collect();
    let (Some(ttft), Some(tokens)) = (
        median(passes.iter().map(|p| p.0)),
        median(passes.iter().map(|p| p.1)),
    ) else {
        let mut missing = Vec::new();
        if !ok.iter().any(|m| m.ttft_ms.is_some()) {
            missing.push("time to first token".to_string());
        }
        if !ok.iter().any(|m| m.prompt_tokens.is_some()) {
            missing.push("prompt token count".to_string());
        }
        if missing.is_empty() {
            missing.push("a pass with both time to first token and prompt token count".into());
        }
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
    let valid: Vec<(f64, f64)> = ok
        .iter()
        .filter_map(|m| m.itl)
        .filter(|i| i.median_ms.is_finite() && i.p95_ms.is_finite() && i.median_ms >= 0.0)
        .map(|i| (i.median_ms, i.p95_ms))
        .collect();
    let (Some(median_ms), Some(p95_ms)) = (
        median(valid.iter().map(|p| p.0)),
        median(valid.iter().map(|p| p.1)),
    ) else {
        let reason = ok
            .iter()
            .find_map(|m| m.itl_unknown_reason.clone())
            .unwrap_or_else(|| "none reported".into());
        return not_evaluated(RULE, vec![format!("valid inter-token latency ({reason})")]);
    };
    let evidence = vec![format!(
        "itl_p95: {p95_ms:.1} ms against median {median_ms:.1} ms"
    )];
    // Compared without dividing, so a zero median with a slow tail still counts.
    if p95_ms > JITTER_RATIO * median_ms {
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
