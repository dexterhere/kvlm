use super::*;
use crate::experiments::{NewExperiment, Pass, PassKind, PassMetrics};

fn spec(variable: Option<Variable>, baseline: Option<i64>) -> NewExperiment {
    NewExperiment {
        engine: Engine::Llamacpp,
        ownership: Ownership::Managed,
        cold: false,
        passes_requested: 3,
        workload_id: "lab-fixed-v1".into(),
        prompt_sha256: "a".repeat(64),
        seed: 42,
        max_tokens: 96,
        model_name: "tiny".into(),
        model_sha256: Some("m".repeat(64)),
        runtime_version: Some("b1".into()),
        backend: Some("cpu".into()),
        hardware_hash: Some("hw".into()),
        requested: Settings {
            context_length: Some(4096),
            cache_type_k: Some("f16".into()),
            flash_attention: Some(false),
            ..Settings::default()
        },
        variable,
        baseline_id: baseline,
    }
}

fn observed_like(spec: &NewExperiment) -> Option<Settings> {
    Some(spec.requested.clone())
}

fn experiment(id: i64, spec: NewExperiment, decode: &[f64]) -> Experiment {
    Experiment {
        id,
        state: State::Completed,
        started_at_unix_ms: 1,
        finished_at_unix_ms: Some(2),
        observed: observed_like(&spec),
        spec,
        summary: None,
        failure: None,
        passes: decode
            .iter()
            .enumerate()
            .map(|(i, d)| Pass {
                id: i as i64,
                kind: PassKind::Measured,
                ordinal: i as u32,
                state: State::Completed,
                started_at_unix_ms: 1,
                finished_at_unix_ms: Some(2),
                metrics: Some(PassMetrics {
                    wall_ms: 1000,
                    decode_tokens_per_s: Some(*d),
                    ..PassMetrics::default()
                }),
                failure: None,
            })
            .collect(),
        findings: vec![],
    }
}

fn metric<'a>(c: &'a Comparison, name: &str) -> &'a MetricDelta {
    c.metrics.iter().find(|m| m.name == name).unwrap()
}

#[test]
fn separated_spreads_are_distinguishable_and_overlapping_ones_are_not() {
    let a = experiment(1, spec(None, None), &[40.0, 42.0, 44.0]);
    let b = experiment(2, spec(None, None), &[50.0, 52.0, 54.0]);
    let c = compare(&a, &b).unwrap();
    let m = metric(&c, "Decode speed");
    assert_eq!(m.distinguishable, Some(true));
    assert!((m.change_percent.unwrap() - (52.0 - 42.0) / 42.0 * 100.0).abs() < 1e-9);

    let overlap = experiment(3, spec(None, None), &[43.0, 45.0, 47.0]);
    assert_eq!(
        metric(&compare(&a, &overlap).unwrap(), "Decode speed").distinguishable,
        Some(false)
    );

    let single = experiment(4, spec(None, None), &[60.0]);
    let m = compare(&a, &single).unwrap();
    assert_eq!(
        metric(&m, "Decode speed").distinguishable,
        None,
        "one pass cannot show a spread"
    );
    assert!(
        metric(&m, "Time to first token").a.is_none(),
        "unmeasured stays unknown"
    );
}

#[test]
fn comparison_refuses_runs_that_did_not_measure_the_same_thing() {
    let a = experiment(1, spec(None, None), &[40.0, 41.0]);
    let with = |f: fn(&mut NewExperiment)| {
        let mut s = spec(None, None);
        f(&mut s);
        experiment(2, s, &[40.0, 41.0])
    };
    for (other, what) in [
        (with(|s| s.workload_id = "other".into()), "the workload"),
        (with(|s| s.prompt_sha256 = "b".repeat(64)), "the prompt"),
        (with(|s| s.seed = 7), "the seed"),
        (with(|s| s.max_tokens = 10), "the maximum output length"),
        (
            with(|s| s.hardware_hash = Some("other".into())),
            "the hardware profile",
        ),
        (with(|s| s.hardware_hash = None), "the hardware profile"),
    ] {
        assert_eq!(compare(&a, &other).unwrap_err(), Refusal::Mismatch(what));
    }
    assert_eq!(
        compare(&a, &with(|s| s.engine = Engine::Ollama)).unwrap_err(),
        Refusal::DifferentEngine
    );
    let mut running = experiment(3, spec(None, None), &[1.0]);
    running.state = State::Running;
    assert_eq!(compare(&a, &running).unwrap_err(), Refusal::NotFinished(3));
    let empty = experiment(4, spec(None, None), &[]);
    assert_eq!(compare(&a, &empty).unwrap_err(), Refusal::NoMeasuredPass(4));
    assert!(
        Refusal::Mismatch("the seed")
            .to_string()
            .contains("the seed")
    );
}

#[test]
fn differences_that_are_allowed_are_named_and_failed_passes_are_counted() {
    let a = experiment(1, spec(None, None), &[40.0, 41.0, 42.0]);
    let mut other = spec(None, None);
    other.model_name = "bigger".into();
    other.model_sha256 = Some("z".repeat(64));
    other.backend = Some("cuda".into());
    other.cold = true;
    let mut b = experiment(2, other, &[40.0, 41.0, 42.0]);
    b.passes[2].state = State::Failed;
    let c = compare(&a, &b).unwrap();
    assert!(c.notes.iter().any(|n| n.contains("different models")));
    assert!(c.notes.iter().any(|n| n.contains("runtime builds")));
    assert!(c.notes.iter().any(|n| n.contains("cold load")));
    assert!(
        c.notes
            .iter()
            .any(|n| n.contains("0 failed or cancelled pass(es) on side A, 1 on side B"))
    );
}

fn follow_up(
    variable: Variable,
    f: impl FnOnce(&mut NewExperiment, &mut Experiment),
) -> (Experiment, Experiment) {
    let base = experiment(1, spec(None, None), &[40.0, 41.0, 42.0]);
    let mut s = spec(Some(variable), Some(1));
    match variable {
        Variable::CacheTypeQ8 => s.requested.cache_type_k = Some("q8_0".into()),
        Variable::FlashAttentionOn => s.requested.flash_attention = Some(true),
        Variable::HalfContext => s.requested.context_length = Some(2048),
    }
    let mut var = experiment(2, s.clone(), &[44.0, 45.0, 46.0]);
    f(&mut s, &mut var);
    (base, var)
}

#[test]
fn a_valid_controlled_follow_up_compares_and_names_its_variable() {
    for variable in [
        Variable::CacheTypeQ8,
        Variable::FlashAttentionOn,
        Variable::HalfContext,
    ] {
        let (base, var) = follow_up(variable, |_, _| {});
        let c = compare_follow_up(&base, &var).unwrap();
        assert_eq!(c.controlled, Some(variable));
        assert_eq!(c.changed_settings.len(), 1);
        assert_eq!(metric(&c, "Decode speed").distinguishable, Some(true));
    }
}

#[test]
fn a_controlled_follow_up_is_refused_unless_only_the_declared_change_differs() {
    let (base, var) = follow_up(Variable::CacheTypeQ8, |_, _| {});
    let mut not_follow_up = var.clone();
    not_follow_up.spec.variable = None;
    assert_eq!(
        compare_follow_up(&base, &not_follow_up).unwrap_err(),
        Refusal::NotAFollowUp
    );

    let mut wrong_base = var.clone();
    wrong_base.spec.baseline_id = Some(99);
    assert_eq!(
        compare_follow_up(&base, &wrong_base).unwrap_err(),
        Refusal::NotTheBaseline
    );

    let mut attached = var.clone();
    attached.spec.ownership = Ownership::Attached;
    assert_eq!(
        compare_follow_up(&base, &attached).unwrap_err(),
        Refusal::NotManaged
    );

    let mut other = var.clone();
    other.spec.requested.context_length = Some(2048);
    assert_eq!(
        compare_follow_up(&base, &other).unwrap_err(),
        Refusal::OtherSettingsChanged(vec!["context_length"])
    );

    let mut same_profile = var.clone();
    same_profile.spec.requested = base.spec.requested.clone();
    assert!(matches!(
        compare_follow_up(&base, &same_profile).unwrap_err(),
        Refusal::ChangeNotObserved(_)
    ));

    for (mutate, what) in [
        (
            (|e: &mut Experiment| e.spec.model_sha256 = Some("x".repeat(64)))
                as fn(&mut Experiment),
            "the model file",
        ),
        (|e| e.spec.model_sha256 = None, "the model file"),
        (
            |e| e.spec.runtime_version = Some("b2".into()),
            "the runtime version",
        ),
        (|e| e.spec.backend = Some("cuda".into()), "the backend"),
        (|e| e.spec.cold = true, "the cache state (cold or warm)"),
        (|e| e.spec.seed = 1, "the seed"),
        (|e| e.spec.prompt_sha256 = "c".repeat(64), "the prompt"),
        (
            |e| e.spec.hardware_hash = Some("other".into()),
            "the hardware profile",
        ),
    ] {
        let mut v = var.clone();
        mutate(&mut v);
        assert_eq!(
            compare_follow_up(&base, &v).unwrap_err(),
            Refusal::Mismatch(what)
        );
    }
}

#[test]
fn the_change_must_show_in_what_the_process_was_started_with() {
    let (base, var) = follow_up(Variable::CacheTypeQ8, |_, _| {});
    let mut unobserved = var.clone();
    unobserved.observed = None;
    assert!(matches!(
        compare_follow_up(&base, &unobserved).unwrap_err(),
        Refusal::ChangeNotObserved(_)
    ));

    let mut not_applied = var.clone();
    not_applied.observed.as_mut().unwrap().cache_type_k = Some("f16".into());
    assert_eq!(
        compare_follow_up(&base, &not_applied).unwrap_err(),
        Refusal::ChangeNotObserved("K cache type")
    );

    let mut extra = var.clone();
    extra.observed.as_mut().unwrap().threads = Some(2);
    let mut base_obs = base.clone();
    base_obs.observed.as_mut().unwrap().threads = Some(8);
    assert_eq!(
        compare_follow_up(&base_obs, &extra).unwrap_err(),
        Refusal::OtherSettingsChanged(vec!["threads"])
    );

    let (base, mut half) = follow_up(Variable::HalfContext, |_, _| {});
    half.observed.as_mut().unwrap().context_length = Some(4096);
    assert_eq!(
        compare_follow_up(&base, &half).unwrap_err(),
        Refusal::ChangeNotObserved("context length")
    );
    let (base, mut fa) = follow_up(Variable::FlashAttentionOn, |_, _| {});
    fa.observed.as_mut().unwrap().flash_attention = None;
    assert_eq!(
        compare_follow_up(&base, &fa).unwrap_err(),
        Refusal::ChangeNotObserved("flash attention")
    );
}
