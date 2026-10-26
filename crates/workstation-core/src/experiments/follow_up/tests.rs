use super::*;

fn profile() -> LaunchProfile {
    LaunchProfile {
        name: "p".into(),
        model_path: "/models/tiny.gguf".into(),
        port: 18080,
        context_length: 4096,
        gpu_layers: 99,
        cache_type_k: CacheType::F16,
        cache_type_v: CacheType::F16,
        flash_attention: false,
        ubatch_size: 512,
        extra: vec![
            ExtraFlag::Threads(8),
            ExtraFlag::Parallel(2),
            ExtraFlag::Mlock,
        ],
    }
}

#[test]
fn requested_settings_mirror_the_profile() {
    let s = requested_settings(&profile());
    assert_eq!(s.context_length, Some(4096));
    assert_eq!(s.gpu_layers, Some(99));
    assert_eq!(s.cache_type_k.as_deref(), Some("f16"));
    assert_eq!(s.flash_attention, Some(false));
    assert_eq!(
        (s.threads, s.parallel, s.ubatch_size),
        (Some(8), Some(2), Some(512))
    );
}

#[test]
fn observed_settings_round_trip_what_the_launcher_emits() {
    let p = profile();
    let args = p.arguments("abcdef0123456789").unwrap();
    assert_eq!(observed_from_arguments(&args), requested_settings(&p));
    assert!(
        requested_settings(&p)
            .differences(&observed_from_arguments(&args))
            .is_empty()
    );
}

#[test]
fn unusable_or_missing_flags_stay_unknown() {
    let args: Vec<String> = [
        "--ctx-size",
        "lots",
        "--cache-type-k",
        "rm -rf",
        "--flash-attn",
        "auto",
        "--threads",
    ]
    .map(String::from)
    .into();
    let s = observed_from_arguments(&args);
    assert_eq!(s, Settings::default());
    assert_eq!(observed_from_arguments(&[]), Settings::default());
}

#[test]
fn each_variable_changes_exactly_one_thing() {
    let base = profile();
    let mut with_fa = base.clone();
    with_fa.flash_attention = true;

    let q8 = with_variable(&base, Variable::CacheTypeQ8, None, 96).unwrap();
    assert_eq!(
        requested_settings(&base).differences(&requested_settings(&q8)),
        vec!["cache_type_k"]
    );
    assert_eq!(
        q8.cache_type_v,
        CacheType::F16,
        "V cache stays unquantized without flash attention"
    );

    let fa = with_variable(&base, Variable::FlashAttentionOn, None, 96).unwrap();
    assert_eq!(
        requested_settings(&base).differences(&requested_settings(&fa)),
        vec!["flash_attention"]
    );

    let half = with_variable(&base, Variable::HalfContext, Some(30), 96).unwrap();
    assert_eq!(half.context_length, 2048);
    assert_eq!(
        requested_settings(&base).differences(&requested_settings(&half)),
        vec!["context_length"]
    );

    assert!(with_variable(&with_fa, Variable::FlashAttentionOn, None, 96).is_err());
    let mut already = base.clone();
    already.cache_type_k = CacheType::Q8_0;
    assert!(with_variable(&already, Variable::CacheTypeQ8, None, 96).is_err());
}

#[test]
fn half_context_is_refused_when_it_cannot_be_shown_to_fit() {
    let base = profile();
    assert!(
        with_variable(&base, Variable::HalfContext, None, 96)
            .unwrap_err()
            .contains("prompt length")
    );
    // 2 slots over 2048 leaves 1024 per slot.
    assert!(with_variable(&base, Variable::HalfContext, Some(1000), 96).is_err());
    assert!(with_variable(&base, Variable::HalfContext, Some(900), 96).is_ok());
    let mut small = base.clone();
    small.context_length = 500;
    assert!(
        with_variable(&small, Variable::HalfContext, Some(1), 1)
            .unwrap_err()
            .contains("smallest")
    );
    let mut default_slots = base;
    default_slots
        .extra
        .retain(|f| !matches!(f, ExtraFlag::Parallel(_)));
    assert!(
        with_variable(&default_slots, Variable::HalfContext, Some(500), 96).is_err(),
        "4 slots: 512 each"
    );
}

#[test]
fn memory_switches_are_part_of_the_snapshots() {
    let mut p = profile();
    let plain = requested_settings(&p);
    p.extra.push(ExtraFlag::NoMmap);
    let with = requested_settings(&p);
    assert_eq!(plain.differences(&with), vec!["no_mmap"]);
    let args = p.arguments("abcdef0123456789").unwrap();
    assert_eq!(observed_from_arguments(&args).no_mmap, Some(true));
    assert_eq!(observed_from_arguments(&args).mlock, Some(true));
    assert_eq!(observed_from_arguments(&[]).mlock, None);
}

#[test]
fn an_enormous_prompt_length_is_refused_without_overflow() {
    assert!(with_variable(&profile(), Variable::HalfContext, Some(u64::MAX), 96).is_err());
}
