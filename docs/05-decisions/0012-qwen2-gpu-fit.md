# 0012: Qwen2 and conditional GPU memory estimates

Date: 2026-10-26. Status: implemented on a dependent feature branch; integration pending.

## Decision

The bounded GGUF reader stores architecture-scoped attention declarations for exact
`llama` and `qwen2` architecture identifiers. It does not mix `llama.*` and
`qwen2.*` keys. Other architecture identifiers remain unsupported by this fit path.
Declared context is informational; it never supplies requested runtime context.

For Qwen2 only, the fit adapter uses an explicitly declared key or value head length
when present. For either missing length, it applies the GGUF default
`embedding_length / attention_heads` only when both declarations exist, are positive,
and divide exactly. LLaMA still requires explicit key and value lengths. The reader
preserves absent dimensions as absent; derivation occurs only in the fit adapter.

An estimate requires a complete shape, positive inputs, one sequence, no recognized
layout warnings, and an explicit `UniformFullAttention` assumption. A breakdown is
conditional arithmetic, not evidence that the GGUF has uniform attention or that a
runtime supports its layout. Runtime compatibility remains Unknown. The conditional
`FitAssessment::memory_class()` reports memory-policy class independently; the main
assessment remains Unknown unless the supplied budget is exceeded. No estimate
grants runtime verification or predicts throughput.

Overview's blocking fit worker inspects the selected local GGUF, then reads fresh
host and device memory. It reports the CPU scenario and each identified GPU with a
confirmed CUDA backend. Each placement accounts for four components: GGUF file-size
weight estimate, separate K/V cache, compute buffers, and runtime allowance. GPU
scenarios compare estimated additional resident load against currently free VRAM.
Current free VRAM already reflects existing allocations; the estimate adds no credit
for them and attributes none to this model. No offload plan is inferred. Devices
without a confirmed CUDA backend receive an explicit unavailable notice; AMD is not
reported as CUDA-compatible.

The UI presents memory classes as conditional estimates and labels runtime
compatibility unverified. Missing readings, unsupported metadata, or layout warnings
remain visible as unknowns or notices. Request generations discard stale results
after file, scenario, or hardware changes.

## Evidence and limits

Upstream was checked on 2026-10-26. GGUF defines optional key and value head lengths
with defaults of embedding length divided by attention head count. The official
Transformers Qwen2 implementation uses a configured `head_dim` when supplied, and
otherwise derives it from hidden size and query-head count. Qwen2's Q/K/V projections
share that head dimension. The llama.cpp Qwen2 graph expects key and value head
dimensions to match. The estimator's arithmetic does not prove runtime support or
identify a local GGUF's source checkpoint or tensor semantics.

The pinned official Qwen2.5-Coder-7B-Instruct config declares 32,768 training context,
while the inspected local GGUF declares 131,072. Neither declaration becomes a
requested context. The config is reference evidence only, not content identity.

- [GGUF specification](https://github.com/ggml-org/ggml/blob/master/docs/gguf.md)
- [Pinned Qwen2.5-Coder-7B-Instruct config](https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct/blob/686220118f733b5fe32992d02604a41f3a45b585/config.json)
- [Transformers Qwen2 attention](https://github.com/huggingface/transformers/blob/main/src/transformers/models/qwen2/modeling_qwen2.py)
- [Transformers Qwen2 configuration](https://github.com/huggingface/transformers/blob/main/src/transformers/models/qwen2/configuration_qwen2.py)
- [llama.cpp Qwen2 conversion](https://github.com/ggml-org/llama.cpp/blob/master/conversion/qwen.py)
- [llama.cpp Qwen2 graph](https://github.com/ggml-org/llama.cpp/blob/master/src/models/qwen2.cpp)
- [GGUF declarations](0005-gguf-metadata.md) and [fit adapter](0006-gguf-fit-integration.md)

CPU and CUDA scenarios remain static budget estimates. Backend allocation, actual
resident weights, runtime compatibility, shared-memory behavior, non-CUDA GPU paths,
offload, and real-hardware calibration remain separate evidence. This decision does
not complete M0 acceptance.
