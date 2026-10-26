// SPDX-License-Identifier: GPL-3.0-or-later
//! Plain-language text for Benchmark Lab results. Nothing here invents a number: a value the
//! run did not measure reads "not measured".

use workstation_core::{
    compare::{Better, Comparison},
    experiments::{Experiment, Finding, FindingStatus, PassKind, Spread, State, Variable},
};

pub(super) fn variable_label(variable: Variable) -> &'static str {
    match variable {
        Variable::CacheTypeQ8 => "Quantize the K cache to q8_0",
        Variable::HalfContext => "Halve the context length",
        Variable::FlashAttentionOn => "Turn flash attention on",
    }
}

/// A deleted baseline leaves no id behind; say so instead of showing a made-up one.
pub(super) fn baseline_label(id: Option<i64>) -> String {
    id.map_or_else(|| "a removed baseline".into(), |id| format!("#{id}"))
}

pub(super) fn state_label(state: State) -> &'static str {
    match state {
        State::Running => "Running",
        State::Completed => "Completed",
        State::Failed => "Failed",
        State::Cancelled => "Cancelled",
        State::Interrupted => "Interrupted",
    }
}

fn spread(value: &Option<Spread>, unit: &str, scale: f64, decimals: usize) -> String {
    match value {
        None => "not measured".into(),
        Some(s) if s.count > 1 => format!(
            "{:.d$} {unit} (range {:.d$}\u{2013}{:.d$}, {} passes)",
            s.median / scale,
            s.min / scale,
            s.max / scale,
            s.count,
            d = decimals
        ),
        Some(s) => format!("{:.d$} {unit} (1 pass)", s.median / scale, d = decimals),
    }
}

const MIB: f64 = 1024.0 * 1024.0;

/// The measured numbers of one experiment, one per line.
pub(super) fn summary_text(experiment: &Experiment) -> String {
    let mut lines = vec![format!(
        "Experiment #{} \u{b7} {} \u{b7} {}",
        experiment.id,
        experiment.spec.model_name,
        state_label(experiment.state)
    )];
    if let Some(failure) = &experiment.failure {
        lines.push(format!("Stopped: {failure}"));
    }
    if let Some(variable) = experiment.spec.variable {
        lines.push(format!(
            "Follow-up of {}: {}",
            baseline_label(experiment.spec.baseline_id),
            variable_label(variable)
        ));
    }
    let Some(s) = &experiment.summary else {
        lines.push("No measured passes were summarized.".into());
        return lines.join("\n");
    };
    lines.push(format!(
        "Measured passes: {} used, {} excluded",
        s.measured_ok, s.excluded
    ));
    lines.push(format!("Wall time: {}", spread(&s.wall_ms, "ms", 1.0, 0)));
    lines.push(format!(
        "Time to first token: {}",
        spread(&s.ttft_ms, "ms", 1.0, 0)
    ));
    lines.push(format!(
        "Prompt speed: {}",
        spread(&s.prompt_tokens_per_s, "tokens/s", 1.0, 1)
    ));
    lines.push(format!(
        "Generation speed: {}",
        spread(&s.decode_tokens_per_s, "tokens/s", 1.0, 1)
    ));
    if s.itl_median_ms.is_some() {
        lines.push(format!(
            "Inter-token latency, median: {}",
            spread(&s.itl_median_ms, "ms", 1.0, 1)
        ));
        lines.push(format!(
            "Inter-token latency, p95: {}",
            spread(&s.itl_p95_ms, "ms", 1.0, 1)
        ));
    } else {
        let reason = experiment
            .passes
            .iter()
            .filter(|p| p.kind == PassKind::Measured)
            .find_map(|p| p.metrics.as_ref()?.itl_unknown_reason.clone());
        lines.push(format!(
            "Inter-token latency: not measured{}",
            reason.map(|r| format!(" ({r})")).unwrap_or_default()
        ));
    }
    lines.push(format!(
        "Server memory (resident, owned process group): {}",
        spread(&s.process_rss_peak_bytes, "MiB", MIB, 0)
    ));
    lines.push(format!(
        "GPU memory (peak): {}",
        spread(&s.gpu_memory_peak_bytes, "MiB", MIB, 0)
    ));
    lines.push(format!(
        "GPU utilization (mean): {}",
        spread(&s.gpu_utilization_mean_percent, "%", 1.0, 0)
    ));
    lines.push(format!(
        "CPU utilization (mean, whole system): {}",
        spread(&s.cpu_utilization_mean_percent, "%", 1.0, 0)
    ));
    if s.load_ms.is_some() {
        lines.push(format!("Cold load: {}", spread(&s.load_ms, "ms", 1.0, 0)));
    }
    if let Some(observed) = &experiment.observed {
        let drift = experiment.spec.requested.differences(observed);
        if !drift.is_empty() {
            lines.push(format!(
                "The server did not apply the requested value for: {}",
                drift.join(", ")
            ));
        }
    }
    lines.join("\n")
}

pub(super) fn finding_title(finding: &Finding) -> String {
    let status = match finding.status {
        FindingStatus::Triggered => "Needs attention",
        FindingStatus::Clear => "Not seen",
        FindingStatus::NotEvaluated => "Could not be checked",
        FindingStatus::NotApplicable => "Not applicable",
    };
    format!("{} \u{b7} {status}", finding.rule_id)
}

pub(super) fn finding_detail(finding: &Finding) -> String {
    let mut lines = vec![finding.message.clone()];
    lines.extend(finding.evidence.iter().map(|e| format!("\u{2022} {e}")));
    if !finding.missing.is_empty() {
        lines.push(format!("Missing: {}", finding.missing.join(", ")));
    }
    lines.join("\n")
}

/// How the second run differs from the first, metric by metric.
pub(super) fn comparison_text(comparison: &Comparison) -> String {
    let mut lines = vec![format!("#{} compared with #{}", comparison.a, comparison.b)];
    if let Some(variable) = comparison.controlled {
        lines.push(format!("Controlled change: {}", variable_label(variable)));
    } else if !comparison.changed_settings.is_empty() {
        lines.push(format!(
            "Settings that differ: {}",
            comparison.changed_settings.join(", ")
        ));
    }
    lines.extend(comparison.notes.iter().cloned());
    for metric in &comparison.metrics {
        let (Some(a), Some(b)) = (&metric.a, &metric.b) else {
            lines.push(format!("{}: not measured on both runs", metric.name));
            continue;
        };
        let change = metric
            .change_percent
            .map_or_else(String::new, |c| format!(" ({c:+.1}%)"));
        let verdict = match (metric.distinguishable, metric.better) {
            (Some(true), Better::Higher) if b.median > a.median => "clearly better",
            (Some(true), Better::Higher) => "clearly worse",
            (Some(true), Better::Lower) if b.median < a.median => "clearly better",
            (Some(true), Better::Lower) => "clearly worse",
            (Some(true), Better::Neither) => "clearly different",
            (Some(false), _) => "within run-to-run spread",
            (None, _) => "too few passes to tell",
        };
        lines.push(format!(
            "{}: {:.1} \u{2192} {:.1} {}{change} \u{b7} {verdict}",
            metric.name, a.median, b.median, metric.unit
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use workstation_core::{
        compare::MetricDelta,
        experiments::{Engine, NewExperiment, Ownership, Settings},
    };

    fn experiment() -> Experiment {
        Experiment {
            id: 4,
            state: State::Completed,
            started_at_unix_ms: 1,
            finished_at_unix_ms: Some(2),
            spec: NewExperiment {
                engine: Engine::Llamacpp,
                ownership: Ownership::Managed,
                cold: false,
                passes_requested: 3,
                workload_id: "lab-fixed-v1".into(),
                prompt_sha256: "0".repeat(64),
                seed: 42,
                max_tokens: 96,
                model_name: "tiny.gguf".into(),
                model_sha256: None,
                runtime_version: None,
                backend: None,
                hardware_hash: None,
                requested: Settings::default(),
                variable: None,
                baseline_id: None,
            },
            observed: None,
            summary: None,
            failure: None,
            passes: vec![],
            findings: vec![],
        }
    }

    #[test]
    fn unmeasured_values_read_not_measured_and_never_zero() {
        let mut e = experiment();
        e.summary = Some(Default::default());
        let text = summary_text(&e);
        assert!(text.contains("Generation speed: not measured"));
        assert!(text.contains("Inter-token latency: not measured"));
        assert!(!text.contains("0.0"));
    }

    #[test]
    fn a_removed_baseline_is_not_shown_as_an_id() {
        let mut e = experiment();
        e.spec.variable = Some(Variable::HalfContext);
        assert!(summary_text(&e).contains("Follow-up of a removed baseline"));
        e.spec.baseline_id = Some(2);
        assert!(summary_text(&e).contains("Follow-up of #2"));
    }

    #[test]
    fn a_missing_summary_and_a_failure_are_stated() {
        let mut e = experiment();
        e.failure = Some("The server stopped.".into());
        let text = summary_text(&e);
        assert!(text.contains("Stopped: The server stopped."));
        assert!(text.contains("No measured passes"));
    }

    #[test]
    fn comparison_words_follow_direction_and_distinguishability() {
        let s = |median| Spread {
            median,
            min: median,
            max: median,
            count: 3,
        };
        let metric = |better, a, b, change, distinguishable| MetricDelta {
            name: "Generation speed",
            unit: "tokens/s",
            better,
            a: Some(s(a)),
            b: Some(s(b)),
            change_percent: change,
            distinguishable,
        };
        let comparison = Comparison {
            a: 1,
            b: 2,
            metrics: vec![
                metric(Better::Higher, 10.0, 8.0, Some(-20.0), Some(true)),
                metric(Better::Lower, 100.0, 90.0, Some(-10.0), Some(true)),
                metric(Better::Higher, 10.0, 10.2, Some(2.0), Some(false)),
                metric(Better::Higher, 10.0, 11.0, Some(10.0), None),
                MetricDelta {
                    name: "GPU",
                    unit: "%",
                    better: Better::Neither,
                    a: None,
                    b: Some(s(1.0)),
                    change_percent: None,
                    distinguishable: None,
                },
            ],
            changed_settings: vec!["cache_type_k"],
            controlled: None,
            notes: vec!["The runs used different models.".into()],
        };
        let text = comparison_text(&comparison);
        assert!(text.contains("10.0 \u{2192} 8.0 tokens/s (-20.0%) \u{b7} clearly worse"));
        assert!(text.contains("100.0 \u{2192} 90.0 tokens/s (-10.0%) \u{b7} clearly better"));
        assert!(text.contains("within run-to-run spread"));
        assert!(text.contains("too few passes to tell"));
        assert!(text.contains("GPU: not measured on both runs"));
        assert!(text.contains("Settings that differ: cache_type_k"));
        assert!(text.contains("different models"));
    }
}
