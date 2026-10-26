// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::Cursor;

use workstation_core::{
    fit::{
        gguf_fit::{GgufFitStatus, LayoutAssumption, RuntimeScenario, assess_gguf_fit},
        memory_math::{ExecutionTarget, FitClass, FitError, FitReason, KvCacheType, RuntimeConfig},
    },
    models::gguf::{GgufError, LayoutWarning, inspect_reader},
};

const TYPE_U32: u32 = 4;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_U64: u32 = 10;
const MIB: u64 = 1_048_576;

fn u32(value: u32, output: &mut Vec<u8>) {
    output.extend(value.to_le_bytes());
}

fn u64(value: u64, output: &mut Vec<u8>) {
    output.extend(value.to_le_bytes());
}

fn string(value: &[u8], output: &mut Vec<u8>) {
    u64(value.len() as u64, output);
    output.extend(value);
}

fn text(value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    string(value, &mut output);
    output
}

fn entry(key: &[u8], kind: u32, value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    string(key, &mut output);
    u32(kind, &mut output);
    output.extend(value);
    output
}

fn scalar(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn tensor() -> Vec<u8> {
    let mut output = Vec::new();
    string(b"weight", &mut output);
    u32(1, &mut output);
    u64(1, &mut output);
    u32(0, &mut output);
    u64(0, &mut output);
    output
}

fn gguf(entries: &[Vec<u8>], with_tensor: bool) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend(b"GGUF");
    u32(3, &mut output);
    u64(if with_tensor { 1 } else { 0 }, &mut output);
    u64(entries.len() as u64, &mut output);
    for entry in entries {
        output.extend(entry);
    }
    if with_tensor {
        output.extend(tensor());
        output.resize(output.len().div_ceil(32) * 32, 0);
        output.extend(0f32.to_le_bytes());
    }
    output
}

fn complete_entries() -> Vec<Vec<u8>> {
    vec![
        entry(b"general.architecture", TYPE_STRING, &text(b"llama")),
        entry(b"llama.block_count", TYPE_U32, &scalar(2)),
        entry(b"llama.embedding_length", TYPE_U32, &scalar(128)),
        entry(b"llama.attention.head_count", TYPE_U32, &scalar(4)),
        entry(b"llama.attention.head_count_kv", TYPE_U32, &scalar(2)),
        entry(b"llama.attention.key_length", TYPE_U32, &scalar(32)),
        entry(b"llama.attention.value_length", TYPE_U32, &scalar(64)),
    ]
}

fn qwen2_entries(key_length: Option<u32>, value_length: Option<u32>) -> Vec<Vec<u8>> {
    let mut entries = vec![
        entry(b"general.architecture", TYPE_STRING, &text(b"qwen2")),
        entry(b"qwen2.block_count", TYPE_U32, &scalar(2)),
        entry(b"qwen2.embedding_length", TYPE_U32, &scalar(128)),
        entry(b"qwen2.attention.head_count", TYPE_U32, &scalar(4)),
        entry(b"qwen2.attention.head_count_kv", TYPE_U32, &scalar(2)),
    ];
    if let Some(length) = key_length {
        entries.push(entry(
            b"qwen2.attention.key_length",
            TYPE_U32,
            &scalar(length),
        ));
    }
    if let Some(length) = value_length {
        entries.push(entry(
            b"qwen2.attention.value_length",
            TYPE_U32,
            &scalar(length),
        ));
    }
    entries
}

fn metadata(
    entries: &[Vec<u8>],
    with_tensor: bool,
) -> workstation_core::models::gguf::GgufMetadata {
    inspect_reader(&mut Cursor::new(gguf(entries, with_tensor))).unwrap()
}

fn scenario() -> RuntimeScenario {
    RuntimeScenario {
        config: RuntimeConfig {
            context_tokens: 257,
            micro_batch_tokens: 1,
            key_cache: KvCacheType::F16,
            value_cache: KvCacheType::F16,
        },
        sequence_count: Some(1),
    }
}

fn cpu(capacity: Option<u64>) -> ExecutionTarget {
    ExecutionTarget::Cpu {
        available_ram_bytes: capacity,
    }
}

#[test]
fn complete_geometry_uses_parser_file_length_and_supplied_scenario() {
    let entries = complete_entries();
    let bytes = gguf(&entries, true);
    let parsed = inspect_reader(&mut Cursor::new(bytes.clone())).unwrap();
    let result = assess_gguf_fit(
        &parsed,
        &scenario(),
        &cpu(Some(16 * 1024 * MIB)),
        LayoutAssumption::UniformFullAttention,
    )
    .unwrap();
    let breakdown = result.assessment.breakdown.unwrap();
    assert_eq!(result.status, GgufFitStatus::AssumedGeometry);
    assert_eq!(result.assessment.class, FitClass::Unknown);
    assert_eq!(result.assessment.reason, FitReason::UnverifiedRuntime);
    assert_eq!(breakdown.weights_bytes, bytes.len() as u64);
    assert_eq!(breakdown.allocated_context_tokens, 512);
    assert_eq!(breakdown.kv_cache_bytes, 393_216);
    assert_eq!(breakdown.buffer_bytes, 256 * MIB);
    assert_eq!(breakdown.overhead_bytes, 64 * MIB);

    let mut mixed_cache = scenario();
    mixed_cache.config.key_cache = KvCacheType::Q8_0;
    mixed_cache.config.value_cache = KvCacheType::Q4_0;
    let mixed = assess_gguf_fit(
        &parsed,
        &mixed_cache,
        &cpu(Some(16 * 1024 * MIB)),
        LayoutAssumption::UniformFullAttention,
    )
    .unwrap();
    assert_eq!(mixed.assessment.breakdown.unwrap().kv_cache_bytes, 143_360);
    assert_eq!(mixed.assessment.class, FitClass::Unknown);
}

#[test]
fn qwen2_uses_declared_lengths_or_independent_integral_gguf_defaults() {
    for (key_length, value_length, expected_kv) in [
        (Some(32), Some(64), 393_216),
        (None, None, 262_144),
        (Some(24), None, 229_376),
    ] {
        let parsed = metadata(&qwen2_entries(key_length, value_length), true);
        let attention = parsed.attention.as_ref().unwrap();
        assert_eq!(attention.key_length, key_length);
        assert_eq!(attention.value_length, value_length);

        let result = assess_gguf_fit(
            &parsed,
            &scenario(),
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        )
        .unwrap();
        assert_eq!(result.status, GgufFitStatus::AssumedGeometry);
        assert_eq!(result.assessment.class, FitClass::Unknown);
        assert_eq!(result.assessment.reason, FitReason::UnverifiedRuntime);
        assert_eq!(
            result.assessment.breakdown.unwrap().kv_cache_bytes,
            expected_kv
        );
    }
}

#[test]
fn qwen2_nonintegral_default_head_length_does_not_promote_geometry() {
    let mut entries = qwen2_entries(None, None);
    entries[2] = entry(b"qwen2.embedding_length", TYPE_U32, &scalar(129));
    let parsed = metadata(&entries, true);
    assert_eq!(parsed.attention.as_ref().unwrap().key_length, None);
    assert_eq!(parsed.attention.as_ref().unwrap().value_length, None);

    let result = assess_gguf_fit(
        &parsed,
        &scenario(),
        &cpu(Some(16 * 1024 * MIB)),
        LayoutAssumption::UniformFullAttention,
    )
    .unwrap();
    assert_eq!(result.status, GgufFitStatus::MissingDimensions);
    assert_eq!(result.assessment.class, FitClass::Unknown);
    assert!(result.assessment.breakdown.is_none());
}

#[test]
fn qwen2_still_requires_one_sequence_explicit_assumption_and_no_layout_warnings() {
    let complete = qwen2_entries(None, None);
    for (scenario, layout, expected) in [
        (
            RuntimeScenario {
                sequence_count: None,
                ..scenario()
            },
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::UnknownSequenceCount,
        ),
        (
            RuntimeScenario {
                sequence_count: Some(2),
                ..scenario()
            },
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::MultipleSequences,
        ),
        (
            scenario(),
            LayoutAssumption::Unconfirmed,
            GgufFitStatus::UnconfirmedLayout,
        ),
    ] {
        let result = assess_gguf_fit(
            &metadata(&complete, true),
            &scenario,
            &cpu(Some(16 * 1024 * MIB)),
            layout,
        )
        .unwrap();
        assert_eq!(result.status, expected);
        assert_eq!(result.assessment.class, FitClass::Unknown);
        assert!(result.assessment.breakdown.is_none());
    }

    let markers: [(&[u8], u32, Vec<u8>); 8] = [
        (b"qwen2.attention.sliding_window", TYPE_U32, scalar(4096)),
        (
            b"qwen2.attention.sliding_window_pattern",
            TYPE_U32,
            scalar(2),
        ),
        (b"qwen2.attention.causal", TYPE_BOOL, vec![0]),
        (b"qwen2.full_attention_interval", TYPE_U32, scalar(2)),
        (b"qwen2.expert_count", TYPE_U32, scalar(2)),
        (b"qwen2.recurrent.state_size", TYPE_U32, scalar(2)),
        (b"qwen2.adapters.lora_rank", TYPE_U32, scalar(2)),
        (b"qwen2.tensor_data_layout", TYPE_STRING, text(b"repacked")),
    ];
    for (key, kind, value) in markers {
        let mut entries = complete.clone();
        entries.push(entry(key, kind, &value));
        let result = assess_gguf_fit(
            &metadata(&entries, true),
            &scenario(),
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        )
        .unwrap();
        assert_eq!(result.status, GgufFitStatus::LayoutWarnings, "{key:?}");
        assert_eq!(result.assessment.class, FitClass::Unknown);
        assert!(result.assessment.breakdown.is_none());
    }
}

#[test]
fn declared_training_context_does_not_replace_supplied_scenario_context() {
    let mut entries = complete_entries();
    entries.push(entry(b"llama.context_length", TYPE_U32, &scalar(131_072)));
    let result = assess_gguf_fit(
        &metadata(&entries, true),
        &scenario(),
        &cpu(Some(16 * 1024 * MIB)),
        LayoutAssumption::UniformFullAttention,
    )
    .unwrap();
    let memory = result.assessment.breakdown.unwrap();
    assert_eq!(memory.allocated_context_tokens, 512);
    assert_eq!(memory.kv_cache_bytes, 393_216);
}

#[test]
fn gate_order_preserves_missing_and_unsupported_states() {
    let complete = complete_entries();
    let mut unknown_architecture = complete.clone();
    unknown_architecture[0] = entry(b"general.architecture", TYPE_STRING, &text(b"other"));
    let mut warned = complete.clone();
    warned.push(entry(b"llama.expert_count", TYPE_U32, &scalar(2)));
    let mut missing_dimensions = complete.clone();
    missing_dimensions.pop();

    for (metadata, scenario, layout, status, reason) in [
        (
            metadata(&complete, false),
            scenario(),
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::NoTensorData,
            FitReason::MissingArtifactMetadata,
        ),
        (
            metadata(&unknown_architecture, true),
            scenario(),
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::UnsupportedArchitecture,
            FitReason::MissingArchitectureMetadata,
        ),
        (
            metadata(&warned, true),
            scenario(),
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::LayoutWarnings,
            FitReason::MissingArchitectureMetadata,
        ),
        (
            metadata(&complete, true),
            RuntimeScenario {
                sequence_count: None,
                ..scenario()
            },
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::UnknownSequenceCount,
            FitReason::MissingArchitectureMetadata,
        ),
        (
            metadata(&complete, true),
            RuntimeScenario {
                sequence_count: Some(2),
                ..scenario()
            },
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::MultipleSequences,
            FitReason::MissingArchitectureMetadata,
        ),
        (
            metadata(&complete, true),
            scenario(),
            LayoutAssumption::Unconfirmed,
            GgufFitStatus::UnconfirmedLayout,
            FitReason::MissingArchitectureMetadata,
        ),
        (
            metadata(&missing_dimensions, true),
            scenario(),
            LayoutAssumption::UniformFullAttention,
            GgufFitStatus::MissingDimensions,
            FitReason::MissingArchitectureMetadata,
        ),
    ] {
        let result =
            assess_gguf_fit(&metadata, &scenario, &cpu(Some(16 * 1024 * MIB)), layout).unwrap();
        assert_eq!(result.status, status);
        assert_eq!(result.assessment.class, FitClass::Unknown);
        assert_eq!(result.assessment.reason, reason);
        assert!(result.assessment.breakdown.is_none());
    }
}

#[test]
fn zero_sequence_and_invalid_geometry_return_typed_errors() {
    let complete = complete_entries();
    assert!(matches!(
        assess_gguf_fit(
            &metadata(&complete, true),
            &RuntimeScenario {
                sequence_count: Some(0),
                ..scenario()
            },
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        ),
        Err(FitError::ZeroValue("sequence count"))
    ));

    let mut invalid_grouping = complete.clone();
    invalid_grouping[3] = entry(b"llama.attention.head_count", TYPE_U32, &scalar(3));
    assert!(matches!(
        assess_gguf_fit(
            &metadata(&invalid_grouping, true),
            &scenario(),
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        ),
        Err(FitError::InvalidHeadGrouping)
    ));

    let mut unaligned = complete.clone();
    unaligned[5] = entry(b"llama.attention.key_length", TYPE_U32, &scalar(31));
    let mut quantized = scenario();
    quantized.config.key_cache = KvCacheType::Q8_0;
    assert!(matches!(
        assess_gguf_fit(
            &metadata(&unaligned, true),
            &quantized,
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        ),
        Err(FitError::UnalignedCacheRow {
            cache: "key",
            elements: 62
        })
    ));

    let mut overflow = scenario();
    overflow.config.context_tokens = u64::MAX;
    assert!(matches!(
        assess_gguf_fit(
            &metadata(&complete, true),
            &overflow,
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        ),
        Err(FitError::Overflow(_))
    ));
}

#[test]
fn parser_rejects_wrong_scalar_types_before_fit_adapter() {
    let mut entries = complete_entries();
    entries[1] = entry(
        b"llama.block_count",
        TYPE_U64,
        &u64::from(2_u32).to_le_bytes(),
    );
    assert!(matches!(
        inspect_reader(&mut Cursor::new(gguf(&entries, true))),
        Err(GgufError::Invalid(_))
    ));
}

#[test]
fn all_layout_warnings_block_geometry_individually() {
    let markers: [(&[u8], u32, Vec<u8>, LayoutWarning); 9] = [
        (
            b"llama.expert_count",
            TYPE_U32,
            scalar(2),
            LayoutWarning::ExpertMetadata,
        ),
        (
            b"llama.attention.sliding_window",
            TYPE_U32,
            scalar(2),
            LayoutWarning::SlidingWindowMetadata,
        ),
        (
            b"llama.ssm.state",
            TYPE_U32,
            scalar(2),
            LayoutWarning::RecurrentMetadata,
        ),
        (
            b"llama.full_attention_interval",
            TYPE_U32,
            scalar(2),
            LayoutWarning::HybridMetadata,
        ),
        (
            b"adapter.type",
            TYPE_U32,
            scalar(2),
            LayoutWarning::AdapterMetadata,
        ),
        (
            b"llama.attention.causal",
            TYPE_BOOL,
            vec![0],
            LayoutWarning::NonCausalAttention,
        ),
        (
            b"general.type",
            TYPE_STRING,
            text(b"adapter"),
            LayoutWarning::NonModelType,
        ),
        (
            b"llama.tensor_data_layout",
            TYPE_STRING,
            text(b"separate"),
            LayoutWarning::TensorLayoutMetadata,
        ),
        (
            b"clip.projector",
            TYPE_U32,
            scalar(2),
            LayoutWarning::AuxiliaryMetadata,
        ),
    ];
    for (key, kind, value, warning) in markers {
        let mut entries = complete_entries();
        entries.push(entry(key, kind, &value));
        let parsed = metadata(&entries, true);
        assert_eq!(parsed.warnings, vec![warning]);
        let result = assess_gguf_fit(
            &parsed,
            &scenario(),
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        )
        .unwrap();
        assert_eq!(result.status, GgufFitStatus::LayoutWarnings);
        assert!(result.assessment.breakdown.is_none());
    }
}

#[test]
fn every_required_dimension_remains_missing_without_defaults() {
    for index in 1..7 {
        let mut entries = complete_entries();
        entries.remove(index);
        let result = assess_gguf_fit(
            &metadata(&entries, true),
            &scenario(),
            &cpu(Some(16 * 1024 * MIB)),
            LayoutAssumption::UniformFullAttention,
        )
        .unwrap();
        assert_eq!(
            result.status,
            GgufFitStatus::MissingDimensions,
            "field {index}"
        );
        assert!(result.assessment.breakdown.is_none());
    }
}

#[test]
fn overbudget_stays_not_recommended_but_absent_capacity_stays_unknown() {
    let parsed = metadata(&complete_entries(), true);
    let overbudget = assess_gguf_fit(
        &parsed,
        &scenario(),
        &cpu(Some(1)),
        LayoutAssumption::UniformFullAttention,
    )
    .unwrap();
    assert_eq!(overbudget.status, GgufFitStatus::AssumedGeometry);
    assert_eq!(overbudget.assessment.class, FitClass::NotRecommended);
    assert_eq!(
        overbudget.assessment.reason,
        FitReason::ExceedsMaximumBudget
    );

    let missing_capacity = assess_gguf_fit(
        &parsed,
        &scenario(),
        &cpu(None),
        LayoutAssumption::UniformFullAttention,
    )
    .unwrap();
    assert_eq!(missing_capacity.assessment.class, FitClass::Unknown);
    assert_eq!(
        missing_capacity.assessment.reason,
        FitReason::MissingMemoryReading
    );
}
