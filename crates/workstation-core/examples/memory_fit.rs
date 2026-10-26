// SPDX-License-Identifier: GPL-3.0-or-later

use workstation_core::fit::memory_math::{
    ExecutionTarget, GpuBackend, KvCacheType, ModelInput, RuntimeCompatibility, RuntimeConfig,
    TransformerShape, assess_fit,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const GIB: u64 = 1024 * 1024 * 1024;
    println!("Synthetic reference inputs only: no model, hardware probe, or benchmark.");
    let model = ModelInput {
        artifact_bytes: Some(4 * GIB),
        transformer: Some(TransformerShape {
            layers: 32,
            embedding_length: 4096,
            attention_heads: 32,
            kv_heads: 2,
            key_head_dim: 128,
            value_head_dim: 128,
        }),
    };
    let target = ExecutionTarget::DedicatedGpu {
        backend: GpuBackend::Cuda,
        available_vram_bytes: Some(6 * GIB),
    };
    for context_tokens in [8192, 32768] {
        let config = RuntimeConfig {
            context_tokens,
            micro_batch_tokens: 512,
            key_cache: KvCacheType::F16,
            value_cache: KvCacheType::F16,
        };
        let result = assess_fit(&model, &config, &target, RuntimeCompatibility::Supported)?;
        println!(
            "Context {context_tokens}: {:?} ({:?})",
            result.class, result.reason
        );
        if let Some(memory) = result.breakdown {
            println!(
                "  Estimated bytes: weights {}, KV {}, buffers {}, overhead {}, total {}",
                memory.weights_bytes,
                memory.kv_cache_bytes,
                memory.buffer_bytes,
                memory.overhead_bytes,
                memory.total_bytes,
            );
        }
    }
    println!(
        "Supported runtime compatibility is an assumption of this fixture, not a probe result."
    );
    Ok(())
}
