// SPDX-License-Identifier: GPL-3.0-or-later
//! Static scenarios from inspected GGUF declarations, never runtime verification.

use super::memory_math::{
    ExecutionTarget, FitAssessment, FitError, ModelInput, RuntimeCompatibility, RuntimeConfig,
    TransformerShape, assess_fit,
};
use crate::models::gguf::{AttentionMetadata, GgufMetadata};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutAssumption {
    Unconfirmed,
    /// Operator assumption of uniform full attention with contiguous K/V rows.
    /// This does not establish the model's actual layout or runtime support.
    UniformFullAttention,
}

/// Supplied scenario, not settings observed from or applied to a running process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeScenario {
    pub config: RuntimeConfig,
    pub sequence_count: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GgufFitStatus {
    NoTensorData,
    UnsupportedArchitecture,
    LayoutWarnings,
    UnknownSequenceCount,
    MultipleSequences,
    UnconfirmedLayout,
    MissingDimensions,
    AssumedGeometry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GgufFitAssessment {
    pub status: GgufFitStatus,
    pub assessment: FitAssessment,
}

/// Estimate a supplied scenario using only LLaMA or Qwen2 dimensions.
/// Expects a successfully inspected snapshot; it does not check file identity or freshness.
/// A layout assumption can enable a breakdown, never Comfortable/Constrained.
/// NotRecommended, if returned, applies only to this scenario's supplied budget.
/// This pure adapter does not read files, probe hardware or contact runtimes.
pub fn assess_gguf_fit(
    metadata: &GgufMetadata,
    scenario: &RuntimeScenario,
    target: &ExecutionTarget,
    layout: LayoutAssumption,
) -> Result<GgufFitAssessment, FitError> {
    if scenario.sequence_count == Some(0) {
        return Err(FitError::ZeroValue("sequence count"));
    }
    let has_tensors = metadata.tensor_count > 0 && metadata.tensor_bytes > 0;
    let shape = metadata.attention.as_ref().and_then(|declarations| {
        declared_shape(
            declarations,
            metadata.architecture.as_deref() == Some("qwen2"),
        )
    });
    let status = if !has_tensors {
        GgufFitStatus::NoTensorData
    } else if !matches!(metadata.architecture.as_deref(), Some("llama" | "qwen2"))
        || metadata.attention.is_none()
    {
        GgufFitStatus::UnsupportedArchitecture
    } else if !metadata.warnings.is_empty() {
        GgufFitStatus::LayoutWarnings
    } else if scenario.sequence_count.is_none() {
        GgufFitStatus::UnknownSequenceCount
    } else if scenario.sequence_count != Some(1) {
        GgufFitStatus::MultipleSequences
    } else if layout == LayoutAssumption::Unconfirmed {
        GgufFitStatus::UnconfirmedLayout
    } else if shape.is_none() {
        GgufFitStatus::MissingDimensions
    } else {
        GgufFitStatus::AssumedGeometry
    };
    let transformer = if status == GgufFitStatus::AssumedGeometry {
        shape
    } else {
        None
    };
    let assessment = assess_fit(
        &ModelInput {
            artifact_bytes: has_tensors.then_some(metadata.file_bytes),
            transformer,
        },
        &scenario.config,
        target,
        RuntimeCompatibility::Unknown,
    )?;
    Ok(GgufFitAssessment { status, assessment })
}

fn declared_shape(metadata: &AttentionMetadata, qwen2: bool) -> Option<TransformerShape> {
    let default_head = if qwen2 {
        let embedding = metadata.embedding_length?;
        let heads = metadata.attention_heads?;
        (heads > 0 && embedding.is_multiple_of(heads)).then(|| embedding / heads)
    } else {
        None
    };
    Some(TransformerShape {
        layers: u64::from(metadata.block_count?),
        embedding_length: u64::from(metadata.embedding_length?),
        attention_heads: u64::from(metadata.attention_heads?),
        kv_heads: u64::from(metadata.kv_heads?),
        key_head_dim: u64::from(metadata.key_length.or(default_head)?),
        value_head_dim: u64::from(metadata.value_length.or(default_head)?),
    })
}
