# Milestones

The [master specification](../01-product/gnome_local_ai_workstation_master_specification.md#10-versioned-delivery-roadmap--milestone-acceptance-criteria)
owns the detailed acceptance criteria. This page tracks actual status.

## Current status: M0 complete for the accepted reference baseline

- [x] Four-crate workspace, native application shell, and Tokio/GLib bridge.
- [x] CPU/RAM, NVIDIA NVML, and AMD sysfs hardware discovery.
- [x] Attached Ollama discovery, installed-model inspection, and loaded allocations.
- [x] Four-part memory calculator, scoped LLaMA/Qwen2 GGUF metadata, and conditional
  CPU/CUDA scenario classes with fresh memory budgets and breakdown tooltips.
- [x] One explicit local-only streaming Quick Benchmark with cancellation and memory observations.
- [x] Immutable SQLite history and exact manifest/hardware verification after restart.
- [x] Normal unit/mock HTTP checks and real NVIDIA Qwen GTK workflow evidence.
- [x] Explicit acceptance of the measured GTK/CUDA reference baseline and documented limits.

The [acceptance record](../03-development/m0-acceptance.md) owns measured results.
The reference Qwen model produced Constrained at 8K and Not recommended at 32K for
additional full CUDA residence, rather than the original illustrative classifications.
The implementation must preserve those measured inputs and conservative policy;
it must not hardcode the specification's example labels. The actual desktop session
was Hyprland, with no GNOME session available. Explicit project instruction accepted
these measured results and limits for M0 on 2026-10-26. GNOME-session validation
remains follow-up work and is not claimed as passed.

## Integration and next milestone

`feat/m0-fit-verification` starts from `develop` and carries the still-open GGUF fix
from [PR #14](https://github.com/dexterhere/kvlm/pull/14). No prerequisite or new PR is
merged automatically. Continue feature PR review through `develop → staging → main`.

M0 milestone acceptance does not merge these PRs or bypass required CI and promotion.
M1 (live model catalog, normalized artifacts, local deduplication, resumable downloads,
keyring token) is implemented on `develop`; see the
[M1 acceptance evidence](../03-development/m1-acceptance.md) for what was run and what is not claimed.
Managed llama.cpp (M2) is implemented on `develop`; see the
[M2 acceptance evidence](../03-development/m2-acceptance.md). Benchmark Lab (M3) is implemented for managed llama.cpp; see the [M3 acceptance evidence](../03-development/m3-acceptance.md). Next feature work belongs to M4. GNOME-session validation and accessibility remain
tracked follow-ups. Additional hardware matrices and attached-runtime lab targets remain
later milestone work. Do not
equate static memory policy, a saved successful request, and verified runtime settings.

## Delivery order

| Milestone | Outcome | Status |
| --- | --- | --- |
| M0 | Hardware → attached Ollama → static fit → quick benchmark → persisted verification | Complete for accepted GTK/CUDA reference baseline; merged to `main` 2026-10-26 |
| M1 | Live model catalog, normalized artifacts, resumable downloads, deduplication, Secret Service | Implemented on `develop`; evidence recorded, sign-off pending ([evidence](../03-development/m1-acceptance.md)) |
| M2 | Managed portable llama.cpp, lifecycle/logs, ownership guards, developer exports | Implemented on `develop`; evidence recorded, sign-off pending ([evidence](../03-development/m2-acceptance.md)) |
| M3 | Controlled A/B benchmarks, cold/warm separation, sampling, diagnostics | Implemented on `develop` for managed llama.cpp; evidence recorded, sign-off pending ([evidence](../03-development/m3-acceptance.md)) |
| M4 | LM Studio/vLLM, NVIDIA/AMD APU/CPU validation, GNOME polish, public beta packaging | Planned |

Before implementing affected features, resolve the
[open design questions](../05-decisions/open-questions.md). Public release also needs
verified project hosting, reporting contacts, and a confirmed application namespace.
