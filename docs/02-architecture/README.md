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

## Execution model

The application creates one multi-thread Tokio runtime. GTK widgets remain on the
GLib main thread. UI commands use Tokio mpsc, and worker events return through
`async_channel` and `glib::MainContext::spawn_local`. No GTK objects cross into workers.
Blocking SQLite, NVML, and sysfs calls must use `spawn_blocking` when implemented.
Closing the application must cancel worker work and release owned resources.

The bridge accepts hardware-detection commands and uses `spawn_blocking` for
read-only CPU/RAM, DRM, and NVML snapshots. Bounded channels and a disabled Refresh
control prevent request buildup; queued duplicates are coalesced. Closing the
window closes its event channel, discards late results, and releases the worker.
Already-running blocking OS/library calls cannot be forcibly cancelled; runtime
shutdown has a two-second wait bound. The app does not touch models, connect to
inference runtimes, or create the database yet.

See [0002: Hardware snapshots](../05-decisions/0002-hardware-snapshots.md) for
the implemented partial-reading contract and known probe limits.

## Intended data flow

Hardware snapshot + artifact metadata + runtime profile → static memory estimate.
An independent successful run for the artifact/hardware pair → empirical verification.
Runtime adapter → stream timings + telemetry → stored benchmark → diagnostic advice.
Catalog metadata → digest match against existing blobs → user-requested download if needed.

Shared GPU memory may overlap physical system RAM. Never count it as extra physical
capacity; the detailed accounting and CPU-only classification remain open decisions.

## Ownership and storage

Attached processes permit only supported, authorized API operations. Stop/restart
and process signals are reserved for managed processes whose ownership is known.
All operations are rootless and managed endpoints default to `127.0.0.1`.

Future application data belongs under `$XDG_DATA_HOME/local-ai-workstation/`
(default `~/.local/share/local-ai-workstation/`), not in the checkout. SQLite uses
WAL and foreign keys; schema changes need versioned migrations. Credentials belong
in Secret Service. Native distribution is the first target. A Flatpak host bridge
is later work, not a requirement for running this foundation.
