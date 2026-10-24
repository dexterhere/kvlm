# Milestones

The [master specification](../01-product/gnome_local_ai_workstation_master_specification.md#10-versioned-delivery-roadmap--milestone-acceptance-criteria)
owns the detailed acceptance criteria. This page tracks actual status.

## Current status: foundation, M0 incomplete

- [x] Four-crate Cargo workspace and documented module ownership.
- [x] Minimal GNOME shell, four navigation views, Tokio/GLib bridge.
- [x] Native build foundation, desktop metadata, community docs, and validation checks.
- [x] Bounded local GGUF v3 metadata and tensor-directory inspection in core (this feature branch).
- [ ] Hardware detection for CPU/RAM, NVIDIA NVML, AMD VRAM/GTT.
- [ ] Attached Ollama discovery, model listing, and loaded-memory inspection.
- [ ] Validated memory-fit calculations and architecture metadata integration.
- [ ] Quick streaming benchmark with resource sampling and SQLite persistence.
- [ ] Persisted empirical badge after restart.
- [ ] Full M0 unit, mocked HTTP, and real-hardware acceptance evidence.

The shell does not fulfill the M0 vertical slice. Do not mark the specification's
M0 checklist complete until the behavior is implemented and verified.

## Next feature

[Hardware discovery in Overview](next-feature-hardware-discovery.md) is the next
implementation slice: CPU/RAM detection, NVIDIA and AMD memory probing, and an
asynchronous Overview display with explicit partial-failure states. This is a
plan. Hardware, Ollama discovery, and static-fit implementation PRs are pending
review; they remain separate from this branch and have not been merged.

This branch adds the [GGUF inspection slice](../03-development/gguf-metadata.md).
After review and integration through `develop`, the next step is to combine
declared artifact metadata with explicit runtime settings and the static-fit
calculator, preserving unknown/unsupported states. Overview fit badges follow
that integration; live benchmark and persistence acceptance remain later M0 work.

## Delivery order

| Milestone | Outcome | Status |
| --- | --- | --- |
| M0 | Hardware → attached Ollama → static fit → quick benchmark → persisted verification | Foundation only |
| M1 | Live model catalog, normalized artifacts, resumable downloads, deduplication, Secret Service | Planned |
| M2 | Managed portable llama.cpp, lifecycle/logs, ownership guards, developer exports | Planned |
| M3 | Controlled A/B benchmarks, cold/warm separation, sampling, diagnostics | Planned |
| M4 | LM Studio/vLLM, NVIDIA/AMD APU/CPU validation, GNOME polish, public beta packaging | Planned |

Before implementing affected features, resolve the
[open design questions](../05-decisions/open-questions.md). Public release also needs
verified project hosting, reporting contacts, and a confirmed application namespace.
