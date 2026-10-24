# 0004: Supplied-metadata static memory fit

Status: implemented for the initial core calculator. Date: 2026-10-24.

## Context

M0 Step 4 needs a useful memory estimate before real inference is available. The
specification leaves CPU-only thresholds, shared-memory overlap, quantized cache
storage, unsupported architectures, and metadata provenance unresolved. Hardware
and attached-Ollama PRs remain separate and unmerged. This slice must work from
`develop` without copying those implementations or introducing speculative adapters.

## Decision

Implement `workstation_core::fit::memory_math::assess_fit` without new dependencies.
It accepts supplied complete single-GGUF file bytes, explicit transformer dimensions,
context/microbatch, separate K/V cache types, one resident execution placement, current
available-memory readings, and externally established runtime compatibility.

The supported model has uniform full-attention layers and contiguous K/V cache rows
for exactly one decoder sequence. The file size estimates resident weights; it is not
a measurement of the runtime's allocation. Missing or unsupported metadata stays
absent. No dimensions, compatibility, or routing are inferred from names or sizes.

### Four components

1. Weights: supplied complete GGUF file bytes.
2. KV: calculate key and value rows separately using `KV heads × explicit head dimension`.
   F16 uses two bytes per element. Q8_0 uses 34 bytes per block of 32 elements, and
   Q4_0 uses 18 bytes per block of 32. Quantized rows must be block aligned; reject
   unsupported dimensions rather than rounding the row. Multiply each stored row by
   context rounded up to 256 tokens and by the layer count, then add K and V.
3. Compute buffers: `max(256 MiB, 8 × microbatch tokens × embedding length)`.
4. Runtime allowance: CPU 64 MiB, CUDA/ROCm 350 MiB, Vulkan 150 MiB.

All component and total arithmetic checks `u64` overflow. Percentage calculations
use `u128` intermediates and floor to whole bytes. Zero requested context, microbatch,
artifact size, or dimensions is invalid input; zero available memory is a real reading.
Attention-head count must be divisible by KV-head count. Explicit K/V head dimensions
may differ and need not be derived by dividing the embedding length.

Padding and block counts describe the supported reference cache layout. Backend
alignment, repacked weights, runtime metadata, and actual buffers can differ. The
four-component result remains a static estimate even when its integer arithmetic is exact.

### Complete placement classification policy

The following headroom limits are project policy, not upstream performance claims.
Use available bytes, never total device capacity. The whole footprint is charged to
the selected resident placement; an oversized GPU plan does not establish that CPU
inference or an explicit offload plan is impossible.

| Placement | Comfortable limit | Maximum constrained limit |
| --- | --- | --- |
| CPU | 70% of available host RAM | 75% of available host RAM |
| Dedicated GPU | 85% of available VRAM | 96% of available VRAM |
| Shared GPU | Minimum of 85% GPU-mappable memory and 70% host RAM | Minimum of 96% GPU-mappable memory and 75% host RAM |

Limits are inclusive. A requirement above the maximum is NotRecommended for that
placement. This closes the 96–100% gap conservatively by preserving the final 4%
GPU headroom. Shared memory is constrained by both pools; it is never added to host
RAM. CPU limits reuse the specification's conservative host-memory allowances. These
policies still need hardware/runtime calibration before general recommendation claims.

Runtime incompatibility produces NotRecommended. A known budget excess also produces
NotRecommended even if compatibility is unknown. Otherwise missing metadata/readings
or unverified compatibility produces Unknown, with a typed reason. Missing metadata
has no invented breakdown. Invalid present inputs return a typed error before any
classification. A single primary reason is returned when several inputs are missing.

### Metadata integration

`llmfit-core` 1.1.17 was evaluated, not added. Its MIT-licensed API provides useful
heuristic recommendations, but its inspected cache estimates use coarse quantization
coefficients and fallbacks. It does not supply this contract's separate K/V block
accounting or exact supplied artifact bytes. No tables or source were copied, and no
substitute name-based architecture database was invented.

## Consequences and non-goals

This is a callable core component with an offline example and tests. Desktop badges,
metadata extraction, live hardware/runtime wiring, and the full M0 Step 4 remain pending.
MLA, hybrid/recurrent/sliding-window layouts, vision/projectors, speculative decoding,
multiple sequences, multi-GPU, partial offload, and sharded safetensors are unsupported
in this slice. Callers leave the applicable metadata absent for those cases. No automatic
context/quantization fallback is attempted without verifying that plan's compatibility.

No result contains BenchmarkVerified, creates database records, starts inference,
reads model files, or predicts throughput. Empirical verification remains a separate
persistence concern. The original specification's broader target remains incomplete.

## Evidence and validation

Upstream was checked on 2026-10-24. Reproducible llama.cpp reference:
`6b9ba4f2e8c1436511956f0b3c309e2c9e642071`.

- [GGML block definitions](https://github.com/ggml-org/llama.cpp/blob/6b9ba4f2e8c1436511956f0b3c309e2c9e642071/ggml/src/ggml-common.h)
  establish the Q8_0 and Q4_0 stored block sizes.
- [Context allocation](https://github.com/ggml-org/llama.cpp/blob/6b9ba4f2e8c1436511956f0b3c309e2c9e642071/src/llama-context.cpp)
  pads context to 256 tokens.
- [Cache tensors](https://github.com/ggml-org/llama.cpp/blob/6b9ba4f2e8c1436511956f0b3c309e2c9e642071/src/llama-kv-cache.cpp)
  use distinct key/value widths and types. Other layouts require their own validation.
- [llmfit 1.1.17 API](https://github.com/AlexsJones/llmfit/blob/v1.1.17/llmfit-core/src/models.rs)
  and [license](https://github.com/AlexsJones/llmfit/blob/v1.1.17/LICENSE).

Independent tests cover hand-calculated components, thresholds one byte either side,
all supported backends/cache formats, shared-memory constraints, absent/zero readings,
invalid geometry, and overflow. See the [development checks and example](../03-development/static-memory-fit.md).
