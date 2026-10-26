# 0006: Conservative GGUF-to-fit integration

Status: implemented as a core adapter and offline CPU example on this feature branch.
Date: 2026-10-24.

## Context

The GGUF inspector reports file layout and typed declarations. The static calculator
accepts artifact bytes, a complete uniform full-attention shape, explicit runtime
settings, a placement, and available-memory readings. GGUF declarations and absence
of warning markers cannot prove a model has that supported layout or that a runtime
supports it.

## Decision

Keep inspection separate from fit arithmetic. The adapter can use the inspected
complete-file byte count as a size estimate and exact typed LLaMA declarations as
inputs, but it must preserve unknown fields. It does not derive key/value head
lengths, apply training-context defaults, whitelist architectures as proof, or infer
layout from tensor names, shapes, or missing warning markers.

`LayoutAssumption::Unconfirmed` is the default. An explicit
`UniformFullAttention` assumption may permit a breakdown only when the architecture
is exactly LLaMA, all six required shape fields are present and positive, no relevant
layout warning is present, and the scenario describes exactly one sequence. The
assumption remains an assumption. Runtime compatibility is always `Unknown` in this
adapter; a memory assessment cannot establish runtime support, actual allocation,
or benchmark verification. The fit class remains `Unknown`, except that `NotRecommended`
may report an exceeded supplied budget; neither case claims compatibility. The adapter
never returns a positive fit class.

The example requires user-supplied context, prompt microbatch, separate K/V cache
types, sequence count, and available CPU RAM. These are scenario inputs, not
observations. Unknown sequence or memory readings remain unknown. The current example
is limited to CPU and does not probe hardware or contact inference runtimes.

## Consequences

The CLI can demonstrate local parsing, explicit scenario handling, layout gating, and
static arithmetic without implying model compatibility. A missing or unsupported
shape remains unknown. A complete explicit assumption can expose a breakdown while
runtime compatibility remains unknown. No default values or fit badges are added.

The adapter is a core component and remains independent of UI and runtime crates.
Future callers must provide evidence for any stronger claim and run blocking file
inspection off the GTK main thread. This integration does not complete M0 hardware,
runtime, UI, benchmark, or persistence acceptance.

## Validation

Workspace formatting, Clippy with warnings denied, all 37 Rust tests, repository
validation, 4 Python tests, 5 Meson checks, staged installation, and 20 synthetic
CLI checks passed. See the [validation record](../03-development/gguf-fit.md#validation).
No real-model, runtime, hardware, or GUI acceptance is claimed. The dependent
feature commits remain on the feature branch; no pull request merge is claimed.
## Follow-up: Qwen2 and conditional GPU classes

[Decision 0012](0012-qwen2-gpu-fit.md) extends the original adapter to exact Qwen2
declarations and the narrow GGUF head-length default, while preserving the original
runtime-compatibility gate. The desktop view now presents independent conditional
CPU/CUDA memory-policy classes; those are not positive runtime compatibility claims.
See the [reference acceptance record](../03-development/m0-acceptance.md).
