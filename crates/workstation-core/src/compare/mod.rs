// SPDX-License-Identifier: GPL-3.0-or-later
//! A/B comparison of two finished experiments. A comparison is made only when the two runs
//! measured the same thing; otherwise it refuses and says why. Differences are reported per
//! metric with each side's spread, and called distinguishable only when the ranges do not
//! overlap.

use std::fmt;

use crate::experiments::{
    Engine, Experiment, Ownership, Settings, Spread, State, Summary, Variable, summarize,
};

/// Fewest measured passes a side needs before its spread can say anything.
pub const MIN_PASSES_FOR_SPREAD: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotFinished(i64),
    NoMeasuredPass(i64),
    DifferentEngine,
    /// A field that must match differs or is unknown on either side.
    Mismatch(&'static str),
    NotAFollowUp,
    NotTheBaseline,
    NotManaged,
    /// Other settings changed besides the declared variable.
    OtherSettingsChanged(Vec<&'static str>),
    /// The runtime's own report does not show the declared change.
    ChangeNotObserved(&'static str),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinished(id) => write!(f, "Experiment {id} has not finished."),
            Self::NoMeasuredPass(id) => write!(f, "Experiment {id} has no completed measured pass."),
            Self::DifferentEngine => f.write_str("The experiments used different runtimes."),
            Self::Mismatch(what) => write!(
                f,
                "These runs cannot be compared: {what} differs or is not recorded on one side."
            ),
            Self::NotAFollowUp => f.write_str("This experiment is not a controlled follow-up."),
            Self::NotTheBaseline => {
                f.write_str("The follow-up was not made from this baseline experiment.")
            }
            Self::NotManaged => f.write_str(
                "A controlled comparison needs a managed runtime on both sides; an attached one is never changed.",
            ),
            Self::OtherSettingsChanged(names) => write!(
                f,
                "More than the declared change differs between the runs: {}.",
                names.join(", ")
            ),
            Self::ChangeNotObserved(what) => write!(
                f,
                "The running process does not show the declared change ({what}), so the comparison is not valid."
            ),
        }
    }
}

impl std::error::Error for Refusal {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
    /// Informational only, such as utilization.
    Neither,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricDelta {
    pub name: &'static str,
    pub unit: &'static str,
    pub better: Better,
    pub a: Option<Spread>,
    pub b: Option<Spread>,
    /// `(b - a) / a` in percent; `None` when either median is missing or `a` is zero.
    pub change_percent: Option<f64>,
    /// `Some(true)` only when both sides have enough passes and their ranges do not overlap;
    /// `None` when there are too few passes to say.
    pub distinguishable: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub a: i64,
    pub b: i64,
    pub metrics: Vec<MetricDelta>,
    /// Settings (as requested) that differ between the sides.
    pub changed_settings: Vec<&'static str>,
    /// The declared single change, for a controlled comparison.
    pub controlled: Option<Variable>,
    pub notes: Vec<String>,
}

fn summary_of(e: &Experiment) -> Summary {
    // The passes are the evidence; a stored summary is only a cache of them.
    summarize(&e.passes)
}

fn ready(e: &Experiment) -> Result<Summary, Refusal> {
    if e.state == State::Running {
        return Err(Refusal::NotFinished(e.id));
    }
    let summary = summary_of(e);
    if summary.measured_ok == 0 {
        return Err(Refusal::NoMeasuredPass(e.id));
    }
    Ok(summary)
}

fn same<T: PartialEq>(a: &T, b: &T, what: &'static str) -> Result<(), Refusal> {
    if a == b {
        Ok(())
    } else {
        Err(Refusal::Mismatch(what))
    }
}

fn same_known(a: &Option<String>, b: &Option<String>, what: &'static str) -> Result<(), Refusal> {
    match (a, b) {
        (Some(x), Some(y)) if !x.is_empty() && x == y => Ok(()),
        _ => Err(Refusal::Mismatch(what)),
    }
}

fn workload_matches(a: &Experiment, b: &Experiment) -> Result<(), Refusal> {
    if a.spec.engine != b.spec.engine {
        return Err(Refusal::DifferentEngine);
    }
    same(&a.spec.workload_id, &b.spec.workload_id, "the workload")?;
    same(
        &a.spec.prompt_sha256.to_ascii_lowercase(),
        &b.spec.prompt_sha256.to_ascii_lowercase(),
        "the prompt",
    )?;
    same(&a.spec.seed, &b.spec.seed, "the seed")?;
    same(
        &a.spec.max_tokens,
        &b.spec.max_tokens,
        "the maximum output length",
    )?;
    same_known(
        &a.spec.hardware_hash,
        &b.spec.hardware_hash,
        "the hardware profile",
    )
}

/// Compare any two finished experiments of the same workload on the same hardware.
/// Differences in model, runtime build or cache state are allowed and listed as notes.
pub fn compare(a: &Experiment, b: &Experiment) -> Result<Comparison, Refusal> {
    let (sa, sb) = (ready(a)?, ready(b)?);
    workload_matches(a, b)?;
    let mut notes = Vec::new();
    if a.spec.model_sha256 != b.spec.model_sha256 || a.spec.model_name != b.spec.model_name {
        notes.push("The runs used different models.".to_string());
    }
    if a.spec.runtime_version != b.spec.runtime_version || a.spec.backend != b.spec.backend {
        notes.push("The runs used different runtime builds or backends.".to_string());
    }
    if a.spec.cold != b.spec.cold {
        notes.push("One run measured a cold load and the other did not.".to_string());
    }
    if a.spec.ownership != b.spec.ownership {
        notes.push("One runtime was managed and the other attached.".to_string());
    }
    Ok(build(a, b, &sa, &sb, None, notes))
}

/// Compare a baseline with the controlled follow-up made from it. Everything but the one
/// declared variable must match, and the running process must show that it took effect.
pub fn compare_follow_up(
    baseline: &Experiment,
    variant: &Experiment,
) -> Result<Comparison, Refusal> {
    let variable = variant.spec.variable.ok_or(Refusal::NotAFollowUp)?;
    if variant.spec.baseline_id != Some(baseline.id) {
        return Err(Refusal::NotTheBaseline);
    }
    if baseline.spec.ownership != Ownership::Managed || variant.spec.ownership != Ownership::Managed
    {
        return Err(Refusal::NotManaged);
    }
    let (sa, sb) = (ready(baseline)?, ready(variant)?);
    workload_matches(baseline, variant)?;
    same_known(
        &baseline.spec.model_sha256,
        &variant.spec.model_sha256,
        "the model file",
    )?;
    same_known(
        &baseline.spec.runtime_version,
        &variant.spec.runtime_version,
        "the runtime version",
    )?;
    same_known(&baseline.spec.backend, &variant.spec.backend, "the backend")?;
    same(
        &baseline.spec.cold,
        &variant.spec.cold,
        "the cache state (cold or warm)",
    )?;
    same(
        &baseline.spec.engine,
        &Engine::Llamacpp,
        "the runtime (only llama.cpp can be changed)",
    )?;

    let changed = baseline
        .spec
        .requested
        .all_differences(&variant.spec.requested);
    let allowed = declared_fields(variable);
    let other: Vec<&'static str> = changed
        .iter()
        .copied()
        .filter(|name| !allowed.contains(name))
        .collect();
    if !other.is_empty() {
        return Err(Refusal::OtherSettingsChanged(other));
    }
    if !allowed.iter().any(|name| changed.contains(name)) {
        return Err(Refusal::ChangeNotObserved("the profiles do not differ"));
    }
    took_effect(variable, &baseline.observed, &variant.observed)?;
    Ok(build(
        baseline,
        variant,
        &sa,
        &sb,
        Some(variable),
        Vec::new(),
    ))
}

fn declared_fields(variable: Variable) -> &'static [&'static str] {
    match variable {
        Variable::CacheTypeQ8 => &["cache_type_k"],
        Variable::HalfContext => &["context_length"],
        Variable::FlashAttentionOn => &["flash_attention"],
    }
}

/// The change must be visible in what the process was actually started with.
fn took_effect(
    variable: Variable,
    baseline: &Option<Settings>,
    variant: &Option<Settings>,
) -> Result<(), Refusal> {
    let (Some(base), Some(var)) = (baseline, variant) else {
        return Err(Refusal::ChangeNotObserved(
            "no observed settings were recorded",
        ));
    };
    match variable {
        Variable::CacheTypeQ8 => {
            // The baseline's value must be known too: an unknown baseline may already have
            // had the new value, in which case nothing changed.
            let was = base.cache_type_k.as_deref();
            if var.cache_type_k.as_deref() != Some("q8_0") || was.is_none() || was == Some("q8_0") {
                return Err(Refusal::ChangeNotObserved("K cache type"));
            }
        }
        Variable::FlashAttentionOn => {
            if var.flash_attention != Some(true) || base.flash_attention != Some(false) {
                return Err(Refusal::ChangeNotObserved("flash attention"));
            }
        }
        Variable::HalfContext => match (base.context_length, var.context_length) {
            (Some(b), Some(v)) if v == b / 2 => {}
            _ => return Err(Refusal::ChangeNotObserved("context length")),
        },
    }
    // Nothing else the process reports may differ either.
    let other: Vec<&'static str> = base
        .all_differences(var)
        .into_iter()
        .filter(|name| !declared_fields(variable).contains(name))
        .collect();
    if other.is_empty() {
        Ok(())
    } else {
        Err(Refusal::OtherSettingsChanged(other))
    }
}

fn delta(
    name: &'static str,
    unit: &'static str,
    better: Better,
    a: Option<Spread>,
    b: Option<Spread>,
) -> MetricDelta {
    let change_percent = match (a, b) {
        (Some(a), Some(b)) if a.median != 0.0 => {
            Some((b.median - a.median) / a.median.abs() * 100.0).filter(|v| v.is_finite())
        }
        _ => None,
    };
    let distinguishable = match (a, b) {
        (Some(a), Some(b))
            if a.count >= MIN_PASSES_FOR_SPREAD && b.count >= MIN_PASSES_FOR_SPREAD =>
        {
            Some(a.distinguishable_from(&b))
        }
        _ => None,
    };
    MetricDelta {
        name,
        unit,
        better,
        a,
        b,
        change_percent,
        distinguishable,
    }
}

fn build(
    a: &Experiment,
    b: &Experiment,
    sa: &Summary,
    sb: &Summary,
    controlled: Option<Variable>,
    mut notes: Vec<String>,
) -> Comparison {
    use Better::{Higher, Lower, Neither};
    let metrics = vec![
        delta(
            "Decode speed",
            "tokens/s",
            Higher,
            sa.decode_tokens_per_s,
            sb.decode_tokens_per_s,
        ),
        delta(
            "Prompt speed",
            "tokens/s",
            Higher,
            sa.prompt_tokens_per_s,
            sb.prompt_tokens_per_s,
        ),
        delta("Time to first token", "ms", Lower, sa.ttft_ms, sb.ttft_ms),
        delta("Pass time", "ms", Lower, sa.wall_ms, sb.wall_ms),
        delta(
            "Inter-token latency, median",
            "ms",
            Lower,
            sa.itl_median_ms,
            sb.itl_median_ms,
        ),
        delta(
            "Inter-token latency, 95th percentile",
            "ms",
            Lower,
            sa.itl_p95_ms,
            sb.itl_p95_ms,
        ),
        delta("Load time (cold)", "ms", Lower, sa.load_ms, sb.load_ms),
        delta(
            "Process memory, peak",
            "bytes",
            Lower,
            sa.process_rss_peak_bytes,
            sb.process_rss_peak_bytes,
        ),
        delta(
            "GPU memory, peak",
            "bytes",
            Lower,
            sa.gpu_memory_peak_bytes,
            sb.gpu_memory_peak_bytes,
        ),
        delta(
            "GPU utilization, mean",
            "%",
            Neither,
            sa.gpu_utilization_mean_percent,
            sb.gpu_utilization_mean_percent,
        ),
        delta(
            "CPU utilization, mean",
            "%",
            Neither,
            sa.cpu_utilization_mean_percent,
            sb.cpu_utilization_mean_percent,
        ),
    ];
    if sa.excluded > 0 || sb.excluded > 0 {
        notes.push(format!(
            "Left out of the medians: {} failed or cancelled pass(es) on side A, {} on side B.",
            sa.excluded, sb.excluded
        ));
    }
    Comparison {
        a: a.id,
        b: b.id,
        metrics,
        changed_settings: a.spec.requested.differences(&b.spec.requested),
        controlled,
        notes,
    }
}

#[cfg(test)]
mod tests;
