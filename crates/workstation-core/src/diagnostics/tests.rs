use super::*;
use crate::experiments::{Coverage, Engine, Itl, NewExperiment, Pass, Sampled, Scope, Settings};

fn sampled(peak: f64, mean: f64) -> Sampled {
    Sampled {
        peak,
        mean,
        coverage: Coverage {
            scope: Scope::Device,
            samples: 10,
            covered_ms: 1000,
            longest_gap_ms: 100,
            capped: false,
        },
    }
}

fn pass(ordinal: u32, metrics: PassMetrics) -> Pass {
    Pass {
        id: i64::from(ordinal),
        kind: PassKind::Measured,
        ordinal,
        state: State::Completed,
        started_at_unix_ms: 1,
        finished_at_unix_ms: Some(2),
        metrics: Some(metrics),
        failure: None,
    }
}

fn experiment(backend: Option<&str>, ownership: Ownership, passes: Vec<PassMetrics>) -> Experiment {
    Experiment {
        id: 1,
        state: State::Completed,
        started_at_unix_ms: 1,
        finished_at_unix_ms: Some(2),
        spec: NewExperiment {
            engine: Engine::Llamacpp,
            ownership,
            cold: false,
            passes_requested: 3,
            workload_id: "w".into(),
            prompt_sha256: "a".repeat(64),
            seed: 42,
            max_tokens: 96,
            model_name: "m".into(),
            model_sha256: None,
            runtime_version: None,
            backend: backend.map(str::to_string),
            hardware_hash: None,
            requested: Settings::default(),
            variable: None,
            baseline_id: None,
        },
        observed: None,
        summary: None,
        failure: None,
        passes: passes
            .into_iter()
            .enumerate()
            .map(|(i, m)| pass(i as u32, m))
            .collect(),
        findings: vec![],
    }
}

fn by(findings: &[Finding], rule: &str) -> Finding {
    findings.iter().find(|f| f.rule_id == rule).unwrap().clone()
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

#[test]
fn no_completed_measured_pass_means_nothing_is_evaluated() {
    let findings = evaluate(
        &experiment(Some("cuda"), Ownership::Managed, vec![]),
        &Context::default(),
    );
    assert_eq!(findings.len(), 4);
    assert!(
        findings
            .iter()
            .all(|f| f.status == FindingStatus::NotEvaluated && !f.warning)
    );
}

const DEDICATED: Context = Context {
    offloadable_layers: Some(33),
    unified_memory: Some(false),
};

#[test]
fn spill_triggers_on_vram_with_model_memory_in_ram_or_on_fewer_layers_than_the_model_has() {
    let metrics = |vram: f64, ram: u64| PassMetrics {
        wall_ms: 1000,
        gpu_memory_bytes: Some(sampled(vram, vram)),
        model_ram_bytes: Some(ram),
        ..PassMetrics::default()
    };
    let hit = evaluate(
        &experiment(
            Some("cuda"),
            Ownership::Managed,
            vec![metrics(4.0 * GIB, 300 * 1024 * 1024); 3],
        ),
        &DEDICATED,
    );
    let f = by(&hit, "DIAG-01");
    assert_eq!(f.status, FindingStatus::Triggered);
    assert!(f.warning && f.message.contains("Consistent with"));
    assert_eq!(f.follow_up, Some(Variable::CacheTypeQ8));

    // Exactly the threshold is not above it; the layer signal is then the one left unchecked.
    let edge = evaluate(
        &experiment(
            Some("cuda"),
            Ownership::Managed,
            vec![metrics(4.0 * GIB, 256 * 1024 * 1024); 3],
        ),
        &DEDICATED,
    );
    let f = by(&edge, "DIAG-01");
    assert_eq!(f.status, FindingStatus::Clear);
    assert!(
        f.missing.iter().any(|m| m.contains("offloaded layers")),
        "unchecked signal is named"
    );
    assert!(f.message.contains("Not checked"));

    // Layers: the model's real count is the measure, per pass or experiment-wide.
    let mut e = experiment(
        Some("cuda"),
        Ownership::Attached,
        vec![PassMetrics::default(); 3],
    );
    e.observed = Some(Settings {
        gpu_layers: Some(20),
        ..Settings::default()
    });
    assert_eq!(
        by(&evaluate(&e, &DEDICATED), "DIAG-01").status,
        FindingStatus::Triggered
    );
    e.observed = Some(Settings {
        gpu_layers: Some(33),
        ..Settings::default()
    });
    let full = by(&evaluate(&e, &DEDICATED), "DIAG-01");
    assert_eq!(full.status, FindingStatus::Clear);
    assert!(
        full.missing.iter().any(|m| m.contains("system RAM")),
        "memory signal reported as unchecked"
    );
    e.observed = None;
    e.passes
        .iter_mut()
        .for_each(|p| p.metrics.as_mut().unwrap().offloaded_layers = Some(20));
    assert_eq!(
        by(&evaluate(&e, &DEDICATED), "DIAG-01").status,
        FindingStatus::Triggered
    );
}

#[test]
fn process_resident_size_alone_is_not_model_allocation() {
    let metrics = PassMetrics {
        gpu_memory_bytes: Some(sampled(4.0 * GIB, 4.0 * GIB)),
        process_rss_bytes: Some(sampled(400.0 * 1024.0 * 1024.0, 400.0 * 1024.0 * 1024.0)),
        ..PassMetrics::default()
    };
    let f = by(
        &evaluate(
            &experiment(Some("cuda"), Ownership::Managed, vec![metrics; 3]),
            &DEDICATED,
        ),
        "DIAG-01",
    );
    assert_eq!(f.status, FindingStatus::NotEvaluated);
}

#[test]
fn spill_signals_from_different_passes_are_not_combined() {
    let vram_only = PassMetrics {
        gpu_memory_bytes: Some(sampled(4.0 * GIB, 4.0 * GIB)),
        ..PassMetrics::default()
    };
    let ram_only = PassMetrics {
        model_ram_bytes: Some(1 << 30),
        ..PassMetrics::default()
    };
    let e = experiment(
        Some("cuda"),
        Ownership::Managed,
        vec![vram_only, ram_only.clone(), ram_only],
    );
    assert_eq!(
        by(&evaluate(&e, &DEDICATED), "DIAG-01").status,
        FindingStatus::NotEvaluated
    );
}

#[test]
fn spill_is_not_applicable_for_cpu_only_zero_layers_and_unified_memory_and_unknown_topology_waits()
{
    let mut e = experiment(
        Some("cpu"),
        Ownership::Managed,
        vec![PassMetrics::default(); 3],
    );
    assert_eq!(
        by(&evaluate(&e, &DEDICATED), "DIAG-01").status,
        FindingStatus::NotApplicable
    );
    e.spec.backend = Some("cuda".into());
    e.spec.requested.gpu_layers = Some(0);
    assert_eq!(
        by(&evaluate(&e, &DEDICATED), "DIAG-01").status,
        FindingStatus::NotApplicable
    );
    e.spec.requested.gpu_layers = Some(99);
    let apu = Context {
        offloadable_layers: Some(33),
        unified_memory: Some(true),
    };
    assert_eq!(
        by(&evaluate(&e, &apu), "DIAG-01").status,
        FindingStatus::NotApplicable
    );
    e.observed = Some(Settings {
        gpu_layers: Some(20),
        ..Settings::default()
    });
    let unknown = Context {
        offloadable_layers: Some(33),
        unified_memory: None,
    };
    let f = by(&evaluate(&e, &unknown), "DIAG-01");
    assert_eq!(f.status, FindingStatus::NotEvaluated);
    assert!(f.missing[0].contains("share memory"));
}

#[test]
fn spill_names_every_missing_input() {
    let e = experiment(
        Some("cuda"),
        Ownership::Attached,
        vec![PassMetrics::default(); 3],
    );
    let f = by(
        &evaluate(
            &e,
            &Context {
                offloadable_layers: None,
                unified_memory: Some(false),
            },
        ),
        "DIAG-01",
    );
    assert_eq!(f.status, FindingStatus::NotEvaluated);
    assert!(f.missing.iter().any(|m| m.contains("system RAM")));
    assert!(f.missing.iter().any(|m| m.contains("offloaded layers")));
    assert!(f.missing.iter().any(|m| m.contains("layer count")));
}

#[test]
fn cpu_fallback_needs_idle_gpu_and_a_busy_owned_process() {
    let metrics = |gpu: f64, cpu_ms: u64| PassMetrics {
        wall_ms: 1000,
        gpu_utilization_percent: Some(sampled(gpu, gpu)),
        process_cpu_ms: Some(cpu_ms),
        ..PassMetrics::default()
    };
    let ctx = Context::default();
    let run = |gpu, cpu| {
        by(
            &evaluate(
                &experiment(Some("cuda"), Ownership::Managed, vec![metrics(gpu, cpu); 3]),
                &ctx,
            ),
            "DIAG-02",
        )
    };
    assert_eq!(run(5.0, 900).status, FindingStatus::Triggered);
    assert!(run(5.0, 900).follow_up.is_none());
    assert_eq!(
        run(15.0, 900).status,
        FindingStatus::Clear,
        "15% is not below 15%"
    );
    assert_eq!(
        run(5.0, 700).status,
        FindingStatus::Clear,
        "70% is not above 70%"
    );
    assert_eq!(run(60.0, 900).status, FindingStatus::Clear);

    let attached = evaluate(
        &experiment(
            Some("cuda"),
            Ownership::Attached,
            vec![metrics(5.0, 900); 3],
        ),
        &ctx,
    );
    let f = by(&attached, "DIAG-02");
    assert_eq!(
        f.status,
        FindingStatus::NotEvaluated,
        "system load alone is never enough"
    );
    assert!(f.missing[0].contains("managed"));

    let no_gpu_reading = experiment(
        Some("cuda"),
        Ownership::Managed,
        vec![
            PassMetrics {
                wall_ms: 1000,
                process_cpu_ms: Some(900),
                ..PassMetrics::default()
            };
            3
        ],
    );
    assert!(by(&evaluate(&no_gpu_reading, &ctx), "DIAG-02").missing[0].contains("GPU utilization"));
    let cpu_only = experiment(Some("cpu"), Ownership::Managed, vec![metrics(1.0, 900); 3]);
    assert_eq!(
        by(&evaluate(&cpu_only, &ctx), "DIAG-02").status,
        FindingStatus::NotApplicable
    );
    let unknown = experiment(None, Ownership::Managed, vec![metrics(1.0, 900); 3]);
    assert_eq!(
        by(&evaluate(&unknown, &ctx), "DIAG-02").status,
        FindingStatus::NotEvaluated
    );
}

#[test]
fn slow_prompt_needs_both_a_long_prompt_and_a_slow_first_token() {
    let metrics = |ttft: f64, tokens: u64| PassMetrics {
        wall_ms: 5000,
        ttft_ms: Some(ttft),
        prompt_tokens: Some(tokens),
        ..PassMetrics::default()
    };
    let run = |ttft, tokens| {
        by(
            &evaluate(
                &experiment(
                    Some("cpu"),
                    Ownership::Managed,
                    vec![metrics(ttft, tokens); 3],
                ),
                &Context::default(),
            ),
            "DIAG-03",
        )
    };
    let hit = run(3500.0, 4000);
    assert_eq!(hit.status, FindingStatus::Triggered);
    assert_eq!(hit.follow_up, Some(Variable::FlashAttentionOn));
    assert_eq!(run(3000.0, 4000).status, FindingStatus::Clear);
    assert_eq!(
        run(9000.0, 2048).status,
        FindingStatus::NotApplicable,
        "needs more than 2048 tokens"
    );
    let missing = by(
        &evaluate(
            &experiment(
                Some("cpu"),
                Ownership::Managed,
                vec![
                    PassMetrics {
                        ttft_ms: Some(9000.0),
                        ..PassMetrics::default()
                    };
                    3
                ],
            ),
            &Context::default(),
        ),
        "DIAG-03",
    );
    assert_eq!(missing.status, FindingStatus::NotEvaluated);
    assert_eq!(missing.missing, vec!["prompt token count"]);
}

#[test]
fn jitter_uses_only_valid_itl_and_reports_why_it_cannot_run() {
    let metrics = |median: f64, p95: f64| PassMetrics {
        itl: Some(Itl {
            intervals: 50,
            median_ms: median,
            p95_ms: p95,
        }),
        ..PassMetrics::default()
    };
    let run = |median, p95| {
        by(
            &evaluate(
                &experiment(
                    Some("cpu"),
                    Ownership::Managed,
                    vec![metrics(median, p95); 3],
                ),
                &Context::default(),
            ),
            "DIAG-04",
        )
    };
    assert_eq!(run(10.0, 26.0).status, FindingStatus::Triggered);
    assert_eq!(
        run(10.0, 25.0).status,
        FindingStatus::Clear,
        "2.5x exactly is not above"
    );
    let unknown = by(
        &evaluate(
            &experiment(
                Some("cpu"),
                Ownership::Managed,
                vec![
                    PassMetrics {
                        itl_unknown_reason: Some("3 stream events for 9 generated tokens".into()),
                        ..PassMetrics::default()
                    };
                    3
                ],
            ),
            &Context::default(),
        ),
        "DIAG-04",
    );
    assert_eq!(unknown.status, FindingStatus::NotEvaluated);
    assert!(unknown.missing[0].contains("3 stream events for 9"));
}

#[test]
fn failed_passes_do_not_feed_any_rule() {
    let mut e = experiment(
        Some("cpu"),
        Ownership::Managed,
        vec![PassMetrics {
            itl: Some(Itl {
                intervals: 50,
                median_ms: 10.0,
                p95_ms: 100.0,
            }),
            ..PassMetrics::default()
        }],
    );
    e.passes[0].state = State::Failed;
    assert_eq!(
        by(&evaluate(&e, &Context::default()), "DIAG-04").status,
        FindingStatus::NotEvaluated
    );
}

#[test]
fn cpu_share_uses_each_passs_own_wall_time() {
    let pass = |wall: u64, cpu: Option<u64>| PassMetrics {
        wall_ms: wall,
        gpu_utilization_percent: Some(sampled(5.0, 5.0)),
        process_cpu_ms: cpu,
        ..PassMetrics::default()
    };
    let e = experiment(
        Some("cuda"),
        Ownership::Managed,
        vec![pass(10_000, Some(900)), pass(100, None), pass(100, None)],
    );
    let f = by(&evaluate(&e, &Context::default()), "DIAG-02");
    assert_eq!(
        f.status,
        FindingStatus::Clear,
        "the one usable pass is 9% busy"
    );
}

#[test]
fn slow_prompt_does_not_combine_unrelated_passes() {
    let a = PassMetrics {
        ttft_ms: Some(9000.0),
        ..PassMetrics::default()
    };
    let b = PassMetrics {
        prompt_tokens: Some(4096),
        ..PassMetrics::default()
    };
    let e = experiment(Some("cpu"), Ownership::Managed, vec![a.clone(), a, b]);
    assert_eq!(
        by(&evaluate(&e, &Context::default()), "DIAG-03").status,
        FindingStatus::NotEvaluated
    );
}

#[test]
fn jitter_handles_a_zero_median_and_ignores_non_finite_latency() {
    let itl = |median_ms: f64, p95_ms: f64| PassMetrics {
        itl: Some(Itl {
            intervals: 8,
            median_ms,
            p95_ms,
        }),
        ..PassMetrics::default()
    };
    let e = experiment(Some("cpu"), Ownership::Managed, vec![itl(0.0, 100.0); 3]);
    assert_eq!(
        by(&evaluate(&e, &Context::default()), "DIAG-04").status,
        FindingStatus::Triggered
    );
    let bad = experiment(
        Some("cpu"),
        Ownership::Managed,
        vec![itl(f64::INFINITY, 100.0); 3],
    );
    assert_eq!(
        by(&evaluate(&bad, &Context::default()), "DIAG-04").status,
        FindingStatus::NotEvaluated
    );
}
