# Milestones

The [master specification](../01-product/gnome_local_ai_workstation_master_specification.md#10-versioned-delivery-roadmap--milestone-acceptance-criteria)
owns the detailed acceptance criteria. This page tracks actual status.

## Current status: hardware discovery implemented, M0 incomplete

- [x] Four-crate Cargo workspace and documented module ownership.
- [x] Minimal GNOME shell, four navigation views, Tokio/GLib bridge.
- [x] Native build foundation, desktop metadata, community docs, and validation checks.
- [x] Hardware detection implementation for CPU/RAM, NVIDIA NVML, AMD VRAM/GTT, and Overview Refresh.
- [ ] Complete live AMD and separate CPU-only machine validation; NVIDIA reference checks and fixtures pass.
- [ ] Attached Ollama discovery, model listing, and loaded-memory inspection.
- [ ] Validated memory-fit calculations and architecture metadata integration.
- [ ] Quick streaming benchmark with resource sampling and SQLite persistence.
- [ ] Persisted empirical badge after restart.
- [ ] Full M0 unit, mocked HTTP, and real-hardware acceptance evidence.

The shell does not fulfill the M0 vertical slice. Do not mark the specification's
M0 checklist complete until the behavior is implemented and verified.

## Current feature and next step

[Hardware discovery in Overview](next-feature-hardware-discovery.md) is implemented
on `feature/hardware-discovery` for review into `develop`. See the
[validation record](../03-development/hardware-validation.md) for coverage and limits.

After reviewing this slice, the next implementation is attached Ollama discovery:
health/version, installed models, and running-model memory through its official
API. It must preserve external-process ownership and handle an absent service.

## Delivery order

| Milestone | Outcome | Status |
| --- | --- | --- |
| M0 | Hardware → attached Ollama → static fit → quick benchmark → persisted verification | Hardware discovery implemented; later steps pending |
| M1 | Live model catalog, normalized artifacts, resumable downloads, deduplication, Secret Service | Planned |
| M2 | Managed portable llama.cpp, lifecycle/logs, ownership guards, developer exports | Planned |
| M3 | Controlled A/B benchmarks, cold/warm separation, sampling, diagnostics | Planned |
| M4 | LM Studio/vLLM, NVIDIA/AMD APU/CPU validation, GNOME polish, public beta packaging | Planned |

Before implementing affected features, resolve the
[open design questions](../05-decisions/open-questions.md). Public release also needs
verified project hosting, reporting contacts, and a confirmed application namespace.
