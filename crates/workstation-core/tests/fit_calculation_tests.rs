// SPDX-License-Identifier: GPL-3.0-or-later

use workstation_core::fit::memory_math::{
    ExecutionTarget, FitAssessment, FitClass, FitError, FitReason, GpuBackend, KvCacheType,
    ModelInput, RuntimeCompatibility, RuntimeConfig, TransformerShape, assess_fit,
};

const MIB: u64 = 1_048_576;
const GIB: u64 = 1_073_741_824;

fn model() -> ModelInput {
    ModelInput {
        artifact_bytes: Some(4 * GIB),
        transformer: Some(TransformerShape {
            layers: 32,
            embedding_length: 4096,
            attention_heads: 32,
            kv_heads: 2,
            key_head_dim: 128,
            value_head_dim: 128,
        }),
    }
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        context_tokens: 8192,
        micro_batch_tokens: 512,
        key_cache: KvCacheType::F16,
        value_cache: KvCacheType::F16,
    }
}

fn gpu() -> ExecutionTarget {
    ExecutionTarget::DedicatedGpu {
        backend: GpuBackend::Cuda,
        available_vram_bytes: Some(6 * GIB),
    }
}

fn supported(
    model: &ModelInput,
    config: &RuntimeConfig,
    target: &ExecutionTarget,
) -> FitAssessment {
    assess_fit(model, config, target, RuntimeCompatibility::Supported).unwrap()
}

#[test]
fn gqa_four_part_estimate_changes_class_at_longer_context() {
    let mut config = config();
    let short = supported(&model(), &config, &gpu());
    let memory = short.breakdown.unwrap();
    assert_eq!(memory.weights_bytes, 4 * GIB);
    assert_eq!(memory.kv_cache_bytes, 256 * MIB);
    assert_eq!(memory.buffer_bytes, 256 * MIB);
    assert_eq!(memory.overhead_bytes, 350 * MIB);
    assert_eq!(memory.total_bytes, 4 * GIB + 862 * MIB);
    assert_eq!(memory.allocated_context_tokens, 8192);
    assert_eq!(short.class, FitClass::Comfortable);
    assert_eq!(short.reason, FitReason::WithinComfortableBudget);

    config.context_tokens = 32768;
    let long = supported(&model(), &config, &gpu());
    assert_eq!(long.breakdown.unwrap().kv_cache_bytes, GIB);
    assert_eq!(long.class, FitClass::Constrained);
    assert_eq!(long.reason, FitReason::WithinMaximumBudget);
}

#[test]
fn standard_multi_head_attention_uses_all_kv_heads() {
    let mut model = model();
    model.transformer.as_mut().unwrap().kv_heads = 32;
    let mut config = config();
    config.context_tokens = 4096;
    assert_eq!(
        supported(&model, &config, &gpu())
            .breakdown
            .unwrap()
            .kv_cache_bytes,
        2 * GIB
    );
}

#[test]
fn quantized_keys_and_values_use_separate_dimensions_and_block_overheads() {
    let model = ModelInput {
        artifact_bytes: Some(GIB),
        transformer: Some(TransformerShape {
            layers: 4,
            embedding_length: 1024,
            attention_heads: 8,
            kv_heads: 2,
            key_head_dim: 128,
            value_head_dim: 64,
        }),
    };
    let config = RuntimeConfig {
        context_tokens: 257,
        micro_batch_tokens: 1,
        key_cache: KvCacheType::Q8_0,
        value_cache: KvCacheType::Q4_0,
    };
    let memory = supported(&model, &config, &gpu()).breakdown.unwrap();
    // Each token stores eight 34-byte key blocks and four 18-byte value blocks.
    assert_eq!(memory.kv_cache_bytes, 704_512);
    assert_eq!(memory.allocated_context_tokens, 512);
}

#[test]
fn context_is_rounded_up_without_adding_an_extra_aligned_block() {
    for (requested, allocated) in [(1, 256), (255, 256), (256, 256), (257, 512)] {
        let mut config = config();
        config.context_tokens = requested;
        let memory = supported(&model(), &config, &gpu()).breakdown.unwrap();
        assert_eq!(memory.allocated_context_tokens, allocated);
        assert_eq!(memory.kv_cache_bytes, allocated * 32_768);
    }
}

#[test]
fn quantized_rows_require_complete_blocks_but_f16_rows_do_not() {
    for key in [true, false] {
        let mut model = model();
        let shape = model.transformer.as_mut().unwrap();
        if key {
            shape.key_head_dim = 1;
        } else {
            shape.value_head_dim = 1;
        }
        assert!(assess_fit(&model, &config(), &gpu(), RuntimeCompatibility::Supported).is_ok());
        for cache in [KvCacheType::Q8_0, KvCacheType::Q4_0] {
            let mut config = config();
            if key {
                config.key_cache = cache;
            } else {
                config.value_cache = cache;
            }
            assert!(matches!(
                assess_fit(&model, &config, &gpu(), RuntimeCompatibility::Supported),
                Err(FitError::UnalignedCacheRow { elements: 2, .. })
            ));
        }
    }
}

#[test]
fn scratch_buffer_has_a_floor_and_grows_with_micro_batch() {
    let mut config = config();
    config.micro_batch_tokens = 1;
    assert_eq!(
        supported(&model(), &config, &gpu())
            .breakdown
            .unwrap()
            .buffer_bytes,
        256 * MIB
    );
    config.micro_batch_tokens = 16384;
    assert_eq!(
        supported(&model(), &config, &gpu())
            .breakdown
            .unwrap()
            .buffer_bytes,
        512 * MIB
    );
}

#[test]
fn context_overhead_depends_on_the_selected_backend() {
    for (target, expected) in [
        (
            ExecutionTarget::Cpu {
                available_ram_bytes: Some(16 * GIB),
            },
            64 * MIB,
        ),
        (gpu(), 350 * MIB),
        (
            ExecutionTarget::DedicatedGpu {
                backend: GpuBackend::Rocm,
                available_vram_bytes: Some(8 * GIB),
            },
            350 * MIB,
        ),
        (
            ExecutionTarget::DedicatedGpu {
                backend: GpuBackend::Vulkan,
                available_vram_bytes: Some(8 * GIB),
            },
            150 * MIB,
        ),
    ] {
        assert_eq!(
            supported(&model(), &config(), &target)
                .breakdown
                .unwrap()
                .overhead_bytes,
            expected
        );
    }
}

fn model_requiring(total_bytes: u64, target: &ExecutionTarget) -> ModelInput {
    let mut model = model();
    model.artifact_bytes = Some(1);
    let other_bytes = supported(&model, &config(), target)
        .breakdown
        .unwrap()
        .total_bytes
        - 1;
    model.artifact_bytes = Some(total_bytes - other_bytes);
    model
}

#[test]
fn exact_thresholds_are_inclusive_for_cpu_gpu_and_each_shared_constraint() {
    for (target, comfortable, maximum) in [
        (
            ExecutionTarget::Cpu {
                available_ram_bytes: Some(10000 * MIB),
            },
            7000 * MIB,
            7500 * MIB,
        ),
        (
            ExecutionTarget::DedicatedGpu {
                backend: GpuBackend::Cuda,
                available_vram_bytes: Some(10000 * MIB),
            },
            8500 * MIB,
            9600 * MIB,
        ),
        (
            ExecutionTarget::SharedGpu {
                backend: GpuBackend::Vulkan,
                available_gpu_mapping_bytes: Some(10000 * MIB),
                available_ram_bytes: Some(20000 * MIB),
            },
            8500 * MIB,
            9600 * MIB,
        ),
        (
            ExecutionTarget::SharedGpu {
                backend: GpuBackend::Vulkan,
                available_gpu_mapping_bytes: Some(20000 * MIB),
                available_ram_bytes: Some(10000 * MIB),
            },
            7000 * MIB,
            7500 * MIB,
        ),
    ] {
        for (required, expected_class, expected_reason) in [
            (
                comfortable - 1,
                FitClass::Comfortable,
                FitReason::WithinComfortableBudget,
            ),
            (
                comfortable,
                FitClass::Comfortable,
                FitReason::WithinComfortableBudget,
            ),
            (
                comfortable + 1,
                FitClass::Constrained,
                FitReason::WithinMaximumBudget,
            ),
            (
                maximum - 1,
                FitClass::Constrained,
                FitReason::WithinMaximumBudget,
            ),
            (
                maximum,
                FitClass::Constrained,
                FitReason::WithinMaximumBudget,
            ),
            (
                maximum + 1,
                FitClass::NotRecommended,
                FitReason::ExceedsMaximumBudget,
            ),
        ] {
            let result = supported(&model_requiring(required, &target), &config(), &target);
            assert_eq!(result.class, expected_class);
            assert_eq!(result.reason, expected_reason);
            assert_eq!(result.breakdown.unwrap().total_bytes, required);
            let limits = result.limits.unwrap();
            assert_eq!(limits.comfortable_bytes, comfortable);
            assert_eq!(limits.maximum_bytes, maximum);
        }
    }
}

#[test]
fn maximum_capacity_percentages_are_floored_without_overflow() {
    for (target, comfortable, maximum) in [
        (
            ExecutionTarget::Cpu {
                available_ram_bytes: Some(u64::MAX),
            },
            12_912_720_851_596_686_130,
            13_835_058_055_282_163_711,
        ),
        (
            ExecutionTarget::DedicatedGpu {
                backend: GpuBackend::Cuda,
                available_vram_bytes: Some(u64::MAX),
            },
            15_679_732_462_653_118_872,
            17_708_874_310_761_169_550,
        ),
        (
            ExecutionTarget::SharedGpu {
                backend: GpuBackend::Rocm,
                available_gpu_mapping_bytes: Some(u64::MAX),
                available_ram_bytes: Some(u64::MAX),
            },
            12_912_720_851_596_686_130,
            13_835_058_055_282_163_711,
        ),
    ] {
        let limits = supported(&model(), &config(), &target).limits.unwrap();
        assert_eq!(limits.comfortable_bytes, comfortable);
        assert_eq!(limits.maximum_bytes, maximum);
    }
}

#[test]
fn absent_capacity_is_unknown_but_zero_capacity_rejects_placement() {
    for target in [
        ExecutionTarget::Cpu {
            available_ram_bytes: None,
        },
        ExecutionTarget::DedicatedGpu {
            backend: GpuBackend::Cuda,
            available_vram_bytes: None,
        },
        ExecutionTarget::SharedGpu {
            backend: GpuBackend::Vulkan,
            available_gpu_mapping_bytes: None,
            available_ram_bytes: Some(16 * GIB),
        },
        ExecutionTarget::SharedGpu {
            backend: GpuBackend::Vulkan,
            available_gpu_mapping_bytes: Some(16 * GIB),
            available_ram_bytes: None,
        },
    ] {
        let result = supported(&model(), &config(), &target);
        assert_eq!(result.class, FitClass::Unknown);
        assert_eq!(result.reason, FitReason::MissingMemoryReading);
        assert!(result.limits.is_none());
    }
    for target in [
        ExecutionTarget::Cpu {
            available_ram_bytes: Some(0),
        },
        ExecutionTarget::DedicatedGpu {
            backend: GpuBackend::Cuda,
            available_vram_bytes: Some(0),
        },
        ExecutionTarget::SharedGpu {
            backend: GpuBackend::Vulkan,
            available_gpu_mapping_bytes: Some(0),
            available_ram_bytes: Some(16 * GIB),
        },
    ] {
        let result = supported(&model(), &config(), &target);
        assert_eq!(result.class, FitClass::NotRecommended);
        assert_eq!(result.reason, FitReason::ExceedsMaximumBudget);
    }
}

#[test]
fn absent_metadata_is_unknown_and_has_no_invented_breakdown() {
    for (model, reason) in [
        (
            ModelInput {
                artifact_bytes: None,
                transformer: model().transformer,
            },
            FitReason::MissingArtifactMetadata,
        ),
        (
            ModelInput {
                artifact_bytes: Some(GIB),
                transformer: None,
            },
            FitReason::MissingArchitectureMetadata,
        ),
    ] {
        let result = supported(&model, &config(), &gpu());
        assert_eq!(result.class, FitClass::Unknown);
        assert_eq!(result.reason, reason);
        assert!(result.breakdown.is_none());
    }
}

#[test]
fn unsupported_runtime_rejects_even_without_metadata() {
    let input = ModelInput {
        artifact_bytes: None,
        transformer: None,
    };
    let result = assess_fit(&input, &config(), &gpu(), RuntimeCompatibility::Unsupported).unwrap();
    assert_eq!(result.class, FitClass::NotRecommended);
    assert_eq!(result.reason, FitReason::UnsupportedRuntime);
}

#[test]
fn unknown_runtime_never_proves_fit_but_known_excess_is_still_rejected() {
    let result = assess_fit(&model(), &config(), &gpu(), RuntimeCompatibility::Unknown).unwrap();
    assert_eq!(result.class, FitClass::Unknown);
    assert_eq!(result.reason, FitReason::UnverifiedRuntime);
    assert!(result.breakdown.is_some());
    let target = ExecutionTarget::Cpu {
        available_ram_bytes: Some(GIB),
    };
    let result = assess_fit(&model(), &config(), &target, RuntimeCompatibility::Unknown).unwrap();
    assert_eq!(result.class, FitClass::NotRecommended);
    assert_eq!(result.reason, FitReason::ExceedsMaximumBudget);
}

#[test]
fn present_zero_values_are_errors_even_when_other_metadata_is_missing() {
    let mut runtime = config();
    runtime.context_tokens = 0;
    assert!(matches!(
        assess_fit(&model(), &runtime, &gpu(), RuntimeCompatibility::Supported),
        Err(FitError::ZeroValue(_))
    ));
    runtime.context_tokens = 8192;
    runtime.micro_batch_tokens = 0;
    assert!(matches!(
        assess_fit(&model(), &runtime, &gpu(), RuntimeCompatibility::Supported),
        Err(FitError::ZeroValue(_))
    ));

    let mut input = model();
    input.artifact_bytes = Some(0);
    input.transformer = None;
    assert!(matches!(
        assess_fit(&input, &config(), &gpu(), RuntimeCompatibility::Supported),
        Err(FitError::ZeroValue(_))
    ));
    for field in 0..6 {
        let mut input = model();
        input.artifact_bytes = None;
        let shape = input.transformer.as_mut().unwrap();
        match field {
            0 => shape.layers = 0,
            1 => shape.embedding_length = 0,
            2 => shape.attention_heads = 0,
            3 => shape.kv_heads = 0,
            4 => shape.key_head_dim = 0,
            _ => shape.value_head_dim = 0,
        }
        assert!(matches!(
            assess_fit(&input, &config(), &gpu(), RuntimeCompatibility::Supported),
            Err(FitError::ZeroValue(_))
        ));
    }
}

#[test]
fn invalid_grouped_query_head_counts_are_errors() {
    for (attention_heads, kv_heads) in [(32, 3), (8, 16)] {
        let mut input = model();
        let shape = input.transformer.as_mut().unwrap();
        shape.attention_heads = attention_heads;
        shape.kv_heads = kv_heads;
        assert!(matches!(
            assess_fit(&input, &config(), &gpu(), RuntimeCompatibility::Supported),
            Err(FitError::InvalidHeadGrouping)
        ));
    }
}

#[test]
fn arithmetic_overflow_is_reported_instead_of_wrapping_or_panicking() {
    for case in 0..13 {
        let mut input = model();
        let shape = input.transformer.as_mut().unwrap();
        let mut config = config();
        match case {
            0 => config.context_tokens = u64::MAX,
            1 => shape.key_head_dim = u64::MAX,
            2 => shape.value_head_dim = u64::MAX,
            3 => {
                shape.kv_heads = 1;
                shape.key_head_dim = u64::MAX / 2;
            }
            4 => {
                shape.kv_heads = 1;
                shape.value_head_dim = u64::MAX / 2;
            }
            5 => shape.layers = u64::MAX,
            6 => config.micro_batch_tokens = u64::MAX,
            7 => shape.embedding_length = u64::MAX,
            8 => input.artifact_bytes = Some(u64::MAX),
            9 => {
                shape.kv_heads = 1;
                shape.key_head_dim = (u64::MAX / 32) * 32;
                config.key_cache = KvCacheType::Q8_0;
            }
            10 => {
                shape.kv_heads = 1;
                shape.key_head_dim = u64::MAX;
            }
            11 => {
                shape.kv_heads = 1;
                shape.value_head_dim = u64::MAX;
            }
            _ => {
                shape.kv_heads = 1;
                shape.key_head_dim = u64::MAX / 4;
            }
        }
        assert!(
            matches!(
                assess_fit(&input, &config, &gpu(), RuntimeCompatibility::Supported),
                Err(FitError::Overflow(_))
            ),
            "overflow case {case}"
        );
    }
}
