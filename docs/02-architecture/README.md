# Architecture

The [master specification](../01-product/gnome_local_ai_workstation_master_specification.md)
defines the intended product. This document describes its implementation boundaries.

## Four crates

```text
workstation-app ──> workstation-bench ──> workstation-runtimes ──> workstation-core
       │                    │                    │
       └────────────────────┴────────────────────┘
                              core domain
```

- `workstation-core`: GTK-independent domain, hardware, memory fit, model metadata,
  deduplication/catalog, SQLite, profile export. No additional catalog or database crate in M0–M4.
- `workstation-runtimes`: runtime contracts, ownership, HTTP adapters, owned-process supervisor.
- `workstation-bench`: deterministic workloads, request timing, sampling, comparisons.
- `workstation-app`: application lifecycle, navigation, UI state, typed worker bridge.

Dependencies flow toward core; core must not import GTK or runtime/UI crates.
Workspace dependency versions are centralized in the root `Cargo.toml`.

Core exposes synchronous, bounded local GGUF inspection through `models::gguf`.
It validates a supported header/directory subset and computes tensor storage bytes
without reading payloads. Architecture values remain declarations, not runtime
configuration or fit evidence. See [the inspection guide](../03-development/gguf-metadata.md).
CLI examples call it synchronously. Overview sends typed fit requests to a blocking
worker; no file inspection or hardware probing occurs on the GTK thread.

`fit::gguf_fit` connects inspected declarations to the static calculator with an
explicit runtime scenario and available-memory budget. Only a labeled uniform
full-attention assumption, complete dimensions, and one sequence allow a breakdown.
Warnings block that path; absence of warnings does not establish support. Runtime
compatibility stays Unknown. See [the scenario guide](../03-development/gguf-fit.md).
The [desktop CPU workflow](../03-development/desktop-cpu-fit.md) reuses that adapter
and reads Linux `MemAvailable` afresh for each estimate without probing GPUs again.
Request generations discard replies after file, scenario, or hardware changes.

## Execution model

The application creates one multi-thread Tokio runtime. GTK widgets remain on the
GLib main thread. UI commands use Tokio mpsc, and worker events return through
`async_channel` and `glib::MainContext::spawn_local`. No GTK objects cross into workers.
Blocking SQLite, NVML, and sysfs calls must use `spawn_blocking` when implemented.
Closing the application must cancel worker work and release owned resources.

On this dependent feature branch, hardware discovery runs in `spawn_blocking` and
returns typed snapshots to Overview. Attached-Ollama discovery is carried from its unmerged PR and runs asynchronously
on Tokio; it never changes runtime state. Hardware, fit, and GGUF prerequisite PRs have not been merged either.
The core calculator is pure: callers supply supported model/runtime metadata and
available-memory readings; it performs no probing, network, database, or GTK work.

## Intended data flow

Hardware snapshot + artifact metadata + runtime profile → static memory estimate.
An independent successful run for the artifact/hardware pair → empirical verification.
Runtime adapter → stream timings + telemetry → stored benchmark → diagnostic advice.
Catalog metadata → digest match against existing blobs → user-requested download if needed.

Shared GPU memory overlaps physical system RAM. The initial calculator caps its
resident footprint by both GPU mapping and host-RAM budgets, without adding them.
CPU-only and selected GPU placement policies are defined in
[decision 0004](../05-decisions/0004-static-memory-fit.md). Offload, verified layout
and compatibility evidence, actual runtime settings, and GPU fit wiring remain
separate work. CPU estimates in Overview are explicitly labeled scenarios.

## Ownership and storage

Attached processes permit only supported, authorized API operations. Stop/restart
and process signals are reserved for managed processes whose ownership is known.
All operations are rootless and managed endpoints default to `127.0.0.1`.

Future application data belongs under `$XDG_DATA_HOME/local-ai-workstation/`
(default `~/.local/share/local-ai-workstation/`), not in the checkout. SQLite uses
WAL and foreign keys; schema changes need versioned migrations. Credentials belong
in Secret Service. Native distribution is the first target. A Flatpak host bridge
is later work, not a requirement for running this foundation.
