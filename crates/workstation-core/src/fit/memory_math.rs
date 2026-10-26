// SPDX-License-Identifier: GPL-3.0-or-later
//! Static memory estimates for one uniform, full-attention decoder sequence.
//!
//! Callers must supply complete single-GGUF artifact bytes and verified architecture
//! dimensions. Leave metadata absent for unsupported layouts, shards, MLA, hybrids,
//! sliding-window attention, vision/projectors, or incomplete metadata. This module
//! does not infer dimensions, runtime support, offload plans, or benchmark evidence.

use std::fmt;

const MIB: u64 = 1024 * 1024;
const CONTEXT_ALIGNMENT: u64 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransformerShape {
    pub layers: u64,
    pub embedding_length: u64,
    pub attention_heads: u64,
    pub kv_heads: u64,
    pub key_head_dim: u64,
    pub value_head_dim: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInput {
    /// Complete single-GGUF file size, used as an estimate of resident weights.
    pub artifact_bytes: Option<u64>,
    /// Only uniform full attention with contiguous K/V rows is supported.
    pub transformer: Option<TransformerShape>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvCacheType {
    F16,
    Q8_0,
    Q4_0,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Requested tokens for exactly one sequence; allocation rounds up to 256.
    pub context_tokens: u64,
    /// Prompt evaluation microbatch, not sequence concurrency.
    pub micro_batch_tokens: u64,
    pub key_cache: KvCacheType,
    pub value_cache: KvCacheType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuBackend {
    Cuda,
    Rocm,
    Vulkan,
}

/// Available bytes must come from a current reading, not total physical capacity.
/// The whole estimated footprint resides in this placement; no offload is inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionTarget {
    Cpu {
        available_ram_bytes: Option<u64>,
    },
    DedicatedGpu {
        backend: GpuBackend,
        available_vram_bytes: Option<u64>,
    },
    SharedGpu {
        backend: GpuBackend,
        available_gpu_mapping_bytes: Option<u64>,
        available_ram_bytes: Option<u64>,
    },
}

/// Supported means the selected artifact, backend, cache types and layout were
/// verified compatible externally; memory fit alone cannot establish that support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCompatibility {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBreakdown {
    pub weights_bytes: u64,
    pub kv_cache_bytes: u64,
    pub buffer_bytes: u64,
    pub overhead_bytes: u64,
    pub total_bytes: u64,
    pub allocated_context_tokens: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryLimits {
    pub comfortable_bytes: u64,
    pub maximum_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitClass {
    Comfortable,
    Constrained,
    NotRecommended,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitReason {
    WithinComfortableBudget,
    WithinMaximumBudget,
    ExceedsMaximumBudget,
    UnsupportedRuntime,
    UnverifiedRuntime,
    MissingArtifactMetadata,
    MissingArchitectureMetadata,
    MissingMemoryReading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FitAssessment {
    pub class: FitClass,
    pub reason: FitReason,
    pub breakdown: Option<MemoryBreakdown>,
    pub limits: Option<MemoryLimits>,
}

impl FitAssessment {
    /// Conditional memory policy only; never establishes runtime compatibility.
    pub fn memory_class(&self) -> Option<FitClass> {
        let memory = self.breakdown?;
        let limits = self.limits?;
        Some(classify_memory(memory.total_bytes, limits))
    }
}

fn classify_memory(bytes: u64, limits: MemoryLimits) -> FitClass {
    if bytes <= limits.comfortable_bytes {
        FitClass::Comfortable
    } else if bytes <= limits.maximum_bytes {
        FitClass::Constrained
    } else {
        FitClass::NotRecommended
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitError {
    ZeroValue(&'static str),
    InvalidHeadGrouping,
    UnalignedCacheRow { cache: &'static str, elements: u64 },
    Overflow(&'static str),
}

impl fmt::Display for FitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroValue(field) => write!(formatter, "{field} must be greater than zero"),
            Self::InvalidHeadGrouping => formatter.write_str(
                "attention heads must be divisible by KV heads, with at least one group",
            ),
            Self::UnalignedCacheRow { cache, elements } => write!(
                formatter,
                "{cache} cache row has {elements} elements; quantized rows need multiples of 32",
            ),
            Self::Overflow(component) => {
                write!(formatter, "{component} exceeds the byte/token range")
            }
        }
    }
}

impl std::error::Error for FitError {}

/// Estimate the four components and classify only the selected resident placement.
/// Allowances and headroom are project policy, not a measured runtime allocation.
/// No result provides benchmark verification or a throughput prediction.
pub fn assess_fit(
    model: &ModelInput,
    config: &RuntimeConfig,
    target: &ExecutionTarget,
    compatibility: RuntimeCompatibility,
) -> Result<FitAssessment, FitError> {
    for (field, value) in [
        ("context tokens", config.context_tokens),
        ("microbatch tokens", config.micro_batch_tokens),
    ] {
        nonzero(field, value)?;
    }
    if let Some(bytes) = model.artifact_bytes {
        nonzero("artifact bytes", bytes)?;
    }
    let allocated_context_tokens = config
        .context_tokens
        .checked_add(CONTEXT_ALIGNMENT - 1)
        .ok_or(FitError::Overflow("context padding"))?
        / CONTEXT_ALIGNMENT
        * CONTEXT_ALIGNMENT;

    let components = model
        .transformer
        .as_ref()
        .map(|shape| {
            for (field, value) in [
                ("layers", shape.layers),
                ("embedding length", shape.embedding_length),
                ("attention heads", shape.attention_heads),
                ("KV heads", shape.kv_heads),
                ("key head dimension", shape.key_head_dim),
                ("value head dimension", shape.value_head_dim),
            ] {
                nonzero(field, value)?;
            }
            if !shape.attention_heads.is_multiple_of(shape.kv_heads) {
                return Err(FitError::InvalidHeadGrouping);
            }
            let key_row = cache_row(config.key_cache, shape.kv_heads, shape.key_head_dim, "key")?;
            let value_row = cache_row(
                config.value_cache,
                shape.kv_heads,
                shape.value_head_dim,
                "value",
            )?;
            let row_bytes = key_row
                .checked_add(value_row)
                .ok_or(FitError::Overflow("KV row"))?;
            let kv = multiply(
                &[row_bytes, allocated_context_tokens, shape.layers],
                "KV cache",
            )?;
            let buffers = multiply(
                &[8, config.micro_batch_tokens, shape.embedding_length],
                "compute buffers",
            )?
            .max(256 * MIB);
            Ok((kv, buffers))
        })
        .transpose()?;

    let breakdown = match (model.artifact_bytes, components) {
        (Some(weights), Some((kv, buffers))) => {
            let overhead = target.overhead_bytes();
            let total = [weights, kv, buffers, overhead]
                .into_iter()
                .try_fold(0_u64, |sum, bytes| sum.checked_add(bytes))
                .ok_or(FitError::Overflow("total memory"))?;
            Some(MemoryBreakdown {
                weights_bytes: weights,
                kv_cache_bytes: kv,
                buffer_bytes: buffers,
                overhead_bytes: overhead,
                total_bytes: total,
                allocated_context_tokens,
            })
        }
        _ => None,
    };
    let limits = target.limits();
    let (class, reason) = if compatibility == RuntimeCompatibility::Unsupported {
        (FitClass::NotRecommended, FitReason::UnsupportedRuntime)
    } else if model.artifact_bytes.is_none() {
        (FitClass::Unknown, FitReason::MissingArtifactMetadata)
    } else if let Some(memory) = breakdown {
        match limits {
            None => (FitClass::Unknown, FitReason::MissingMemoryReading),
            Some(limits)
                if classify_memory(memory.total_bytes, limits) == FitClass::NotRecommended =>
            {
                (FitClass::NotRecommended, FitReason::ExceedsMaximumBudget)
            }
            Some(_) if compatibility == RuntimeCompatibility::Unknown => {
                (FitClass::Unknown, FitReason::UnverifiedRuntime)
            }
            Some(limits)
                if classify_memory(memory.total_bytes, limits) == FitClass::Comfortable =>
            {
                (FitClass::Comfortable, FitReason::WithinComfortableBudget)
            }
            Some(_) => (FitClass::Constrained, FitReason::WithinMaximumBudget),
        }
    } else {
        (FitClass::Unknown, FitReason::MissingArchitectureMetadata)
    };
    Ok(FitAssessment {
        class,
        reason,
        breakdown,
        limits,
    })
}

impl ExecutionTarget {
    fn overhead_bytes(&self) -> u64 {
        match self {
            Self::Cpu { .. } => 64 * MIB,
            Self::DedicatedGpu { backend, .. } | Self::SharedGpu { backend, .. } => match backend {
                GpuBackend::Cuda | GpuBackend::Rocm => 350 * MIB,
                GpuBackend::Vulkan => 150 * MIB,
            },
        }
    }

    fn limits(&self) -> Option<MemoryLimits> {
        let (comfortable_bytes, maximum_bytes) = match *self {
            Self::Cpu {
                available_ram_bytes,
            } => {
                let ram = available_ram_bytes?;
                (percentage(ram, 70), percentage(ram, 75))
            }
            Self::DedicatedGpu {
                available_vram_bytes,
                ..
            } => {
                let vram = available_vram_bytes?;
                (percentage(vram, 85), percentage(vram, 96))
            }
            Self::SharedGpu {
                available_gpu_mapping_bytes,
                available_ram_bytes,
                ..
            } => {
                let mapping = available_gpu_mapping_bytes?;
                let ram = available_ram_bytes?;
                (
                    percentage(mapping, 85).min(percentage(ram, 70)),
                    percentage(mapping, 96).min(percentage(ram, 75)),
                )
            }
        };
        Some(MemoryLimits {
            comfortable_bytes,
            maximum_bytes,
        })
    }
}

fn percentage(bytes: u64, percent: u8) -> u64 {
    (u128::from(bytes) * u128::from(percent) / 100) as u64
}

fn nonzero(field: &'static str, value: u64) -> Result<(), FitError> {
    if value == 0 {
        Err(FitError::ZeroValue(field))
    } else {
        Ok(())
    }
}

fn multiply(values: &[u64], component: &'static str) -> Result<u64, FitError> {
    values.iter().try_fold(1_u64, |product, value| {
        product
            .checked_mul(*value)
            .ok_or(FitError::Overflow(component))
    })
}

fn cache_row(
    cache_type: KvCacheType,
    heads: u64,
    head_dim: u64,
    cache: &'static str,
) -> Result<u64, FitError> {
    let elements = multiply(&[heads, head_dim], "KV row elements")?;
    match cache_type {
        KvCacheType::F16 => multiply(&[elements, 2], "KV row"),
        KvCacheType::Q8_0 | KvCacheType::Q4_0 => {
            if !elements.is_multiple_of(32) {
                return Err(FitError::UnalignedCacheRow { cache, elements });
            }
            let block_bytes = if cache_type == KvCacheType::Q8_0 {
                34
            } else {
                18
            };
            multiply(&[elements / 32, block_bytes], "quantized KV row")
        }
    }
}
