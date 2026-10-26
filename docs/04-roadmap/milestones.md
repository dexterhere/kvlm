# Milestones

The [master specification](../01-product/gnome_local_ai_workstation_master_specification.md#10-versioned-delivery-roadmap--milestone-acceptance-criteria)
owns the detailed acceptance criteria. This page tracks actual status.

## Current status: M0 implementation present, acceptance open

- [x] Four-crate workspace, native application shell, and Tokio/GLib bridge.
- [x] CPU/RAM, NVIDIA NVML, and AMD sysfs hardware discovery.
- [x] Attached Ollama discovery, installed-model inspection, and loaded allocations.
- [x] Four-part memory calculator, scoped LLaMA/Qwen2 GGUF metadata, and conditional
  CPU/CUDA scenario classes with fresh memory budgets and breakdown tooltips.
- [x] One explicit local-only streaming Quick Benchmark with cancellation and memory observations.
- [x] Immutable SQLite history and exact manifest/hardware verification after restart.
- [x] Normal unit/mock HTTP checks and real NVIDIA Qwen GTK workflow evidence.
- [ ] Native GNOME-session acceptance and review of the reference fit criterion.

The [acceptance record](../03-development/m0-acceptance.md) owns measured results.
The reference Qwen model produced Constrained at 8K and Not recommended at 32K for
additional full CUDA residence, rather than the original illustrative classifications.
The implementation must preserve those measured inputs and conservative policy;
it must not hardcode the specification's example labels. The actual desktop session
was Hyprland, with no GNOME session available. Strict M0 sign-off remains open.

## Integration and next gate

`feat/m0-fit-verification` starts from `develop` and carries the still-open GGUF fix
from [PR #14](https://github.com/dexterhere/kvlm/pull/14). No prerequisite or new PR is
merged automatically. Continue feature PR review through `develop → staging → main`.

Complete GNOME-session acceptance and resolve the reference criterion before declaring
M0 done. Process attribution, GPU utilization, controlled cold/warm comparisons, and
additional hardware matrices remain later milestone work. Do not equate static memory
policy, a saved successful request, and verified runtime settings.

## Delivery order

| Milestone | Outcome | Status |
| --- | --- | --- |
| M0 | Hardware → attached Ollama → static fit → quick benchmark → persisted verification | Feature implementation and reference CUDA checks present; final acceptance open |
| M1 | Live model catalog, normalized artifacts, resumable downloads, deduplication, Secret Service | Planned |
| M2 | Managed portable llama.cpp, lifecycle/logs, ownership guards, developer exports | Planned |
| M3 | Controlled A/B benchmarks, cold/warm separation, sampling, diagnostics | Planned |
| M4 | LM Studio/vLLM, NVIDIA/AMD APU/CPU validation, GNOME polish, public beta packaging | Planned |

Before implementing affected features, resolve the
[open design questions](../05-decisions/open-questions.md). Public release also needs
verified project hosting, reporting contacts, and a confirmed application namespace.
