# Milestones

The [master specification](../01-product/gnome_local_ai_workstation_master_specification.md#10-versioned-delivery-roadmap--milestone-acceptance-criteria)
owns the detailed acceptance criteria. This page tracks actual status.

## Current status: foundation, M0 incomplete

- [x] Bounded local GGUF v3 metadata and tensor-directory inspection in core.
- [x] GGUF-to-calculator scenario adapter with explicit settings and unverified compatibility.
- [x] Overview CPU scenario form with local-file inspection, fresh RAM, and stale-reply protection.
- [x] Four-crate Cargo workspace and documented module ownership.
- [x] Minimal GNOME shell, four navigation views, Tokio/GLib bridge.
- [x] Native build foundation, desktop metadata, community docs, and validation checks.
- [x] Hardware detection for CPU/RAM, NVIDIA NVML, AMD VRAM/GTT (carried from PR #1).
- [x] Attached Ollama discovery, model listing, and loaded-memory inspection (carried from PR #2).
- [x] Installed-model selection with read-only declarations and fresh loaded readings.
- [ ] Validated memory-fit calculations and architecture metadata integration.
- [x] Pure supplied-metadata calculator, placement policy, offline example, and tests.
- [ ] Quick streaming benchmark with resource sampling and SQLite persistence.
- [ ] Persisted empirical badge after restart.
- [ ] Full M0 unit, mocked HTTP, and real-hardware acceptance evidence.

The shell does not fulfill the M0 vertical slice. Do not mark the specification's
M0 checklist complete until the behavior is implemented and verified.

## Next feature

This dependent branch carries [PR #2](https://github.com/dexterhere/kvlm/pull/2)
and [PR #6](https://github.com/dexterhere/kvlm/pull/6), including the hardware, static
calculator, GGUF, and scenario prerequisites in #1, #3, #4, and #5. All remain unmerged.
The [desktop CPU workflow](../03-development/desktop-cpu-fit.md) and
[Ollama model inspection](../03-development/ollama-inspection.md) now coexist.
Inspection keeps declared defaults separate from fresh loaded context and allocation;
it does not prove local execution, artifact identity, or runtime compatibility.
Next: define a fail-closed local-execution guard and reproducible quick streaming
benchmark, then add SQLite persistence and restart verification. Actual K/V types,
concurrency, micro-batch settings, layout evidence, and allocation calibration remain
open. M0 is incomplete.

## Delivery order

| Milestone | Outcome | Status |
| --- | --- | --- |
| M0 | Hardware → attached Ollama → static fit → quick benchmark → persisted verification | Core estimate and open feature PRs; integration pending |
| M1 | Live model catalog, normalized artifacts, resumable downloads, deduplication, Secret Service | Planned |
| M2 | Managed portable llama.cpp, lifecycle/logs, ownership guards, developer exports | Planned |
| M3 | Controlled A/B benchmarks, cold/warm separation, sampling, diagnostics | Planned |
| M4 | LM Studio/vLLM, NVIDIA/AMD APU/CPU validation, GNOME polish, public beta packaging | Planned |

Before implementing affected features, resolve the
[open design questions](../05-decisions/open-questions.md). Public release also needs
verified project hosting, reporting contacts, and a confirmed application namespace.
