// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

fn pass(kind: PassKind, state: State, metrics: Option<PassMetrics>) -> Pass {
    Pass {
        id: 0,
        kind,
        ordinal: 0,
        state,
        started_at_unix_ms: 0,
        finished_at_unix_ms: None,
        metrics,
        failure: None,
    }
}

fn metrics(decode: f64, ttft: f64) -> PassMetrics {
    PassMetrics {
        wall_ms: 1000,
        ttft_ms: Some(ttft),
        decode_tokens_per_s: Some(decode),
        ..PassMetrics::default()
    }
}

#[test]
fn spread_uses_the_middle_value_and_never_hides_the_range() {
    let s = Spread::of(&[30.0, 10.0, 20.0]).unwrap();
    assert_eq!((s.median, s.min, s.max, s.count), (20.0, 10.0, 30.0, 3));
    assert_eq!(Spread::of(&[1.0, 3.0]).unwrap().median, 2.0);
    assert!(Spread::of(&[]).is_none());
    assert!(
        Spread::of(&[f64::NAN]).is_none(),
        "non-finite values are not data"
    );
}

#[test]
fn overlapping_ranges_are_not_distinguishable() {
    let a = Spread::of(&[10.0, 12.0]).unwrap();
    let b = Spread::of(&[11.0, 15.0]).unwrap();
    let c = Spread::of(&[13.0, 15.0]).unwrap();
    assert!(!a.distinguishable_from(&b));
    assert!(a.distinguishable_from(&c) && c.distinguishable_from(&a));
}

#[test]
fn a_failed_pass_is_counted_and_left_out_of_the_summary() {
    let passes = vec![
        pass(PassKind::Warmup, State::Completed, Some(metrics(1.0, 1.0))),
        pass(
            PassKind::Measured,
            State::Completed,
            Some(metrics(40.0, 100.0)),
        ),
        pass(PassKind::Measured, State::Failed, None),
        pass(
            PassKind::Measured,
            State::Completed,
            Some(metrics(44.0, 120.0)),
        ),
        pass(
            PassKind::Measured,
            State::Cancelled,
            Some(metrics(5.0, 900.0)),
        ),
    ];
    let summary = summarize(&passes);
    assert_eq!((summary.measured_ok, summary.excluded), (2, 2));
    let decode = summary.decode_tokens_per_s.unwrap();
    assert_eq!((decode.min, decode.max, decode.median), (40.0, 44.0, 42.0));
    assert!(summary.itl_median_ms.is_none(), "unmeasured stays unknown");
    assert!(summary.load_ms.is_none());
}

#[test]
fn cold_load_time_is_summarized_apart_from_measured_passes() {
    let mut load = metrics(0.0, 0.0);
    load.load_ms = Some(2500.0);
    let passes = vec![pass(PassKind::ColdLoad, State::Completed, Some(load))];
    let summary = summarize(&passes);
    assert_eq!(summary.load_ms.unwrap().median, 2500.0);
    assert_eq!(summary.measured_ok, 0);
}

#[test]
fn settings_differ_only_where_both_sides_report_a_value() {
    let asked = Settings {
        context_length: Some(8192),
        gpu_layers: Some(99),
        flash_attention: Some(true),
        ..Settings::default()
    };
    let seen = Settings {
        context_length: Some(4096),
        gpu_layers: None,
        flash_attention: Some(true),
        ..Settings::default()
    };
    assert_eq!(asked.differences(&seen), vec!["context_length"]);
}

fn spec() -> NewExperiment {
    NewExperiment {
        engine: Engine::Llamacpp,
        ownership: Ownership::Managed,
        cold: false,
        passes_requested: 3,
        workload_id: "fixed-32".into(),
        prompt_sha256: "a".repeat(64),
        seed: 42,
        max_tokens: 64,
        model_name: "tiny".into(),
        model_sha256: None,
        runtime_version: None,
        backend: None,
        hardware_hash: None,
        requested: Settings::default(),
        variable: None,
        baseline_id: None,
    }
}

#[test]
fn a_new_experiment_is_validated_before_it_is_stored() {
    assert!(spec().validate().is_ok());
    for bad in [
        NewExperiment {
            passes_requested: 0,
            ..spec()
        },
        NewExperiment {
            passes_requested: 21,
            ..spec()
        },
        NewExperiment {
            prompt_sha256: "zz".into(),
            ..spec()
        },
        NewExperiment {
            model_name: String::new(),
            ..spec()
        },
        NewExperiment {
            workload_id: "a\nb".into(),
            ..spec()
        },
        NewExperiment {
            variable: Some(Variable::HalfContext),
            ..spec()
        },
        NewExperiment {
            baseline_id: Some(1),
            ..spec()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}
