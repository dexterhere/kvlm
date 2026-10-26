# Next feature: hardware discovery in Overview

Planned and implemented on 2026-10-24. Status: implementation and NVIDIA reference
validation complete; live AMD and separate CPU-only validation remain outstanding.
The code is submitted against `develop`; this does not complete the broader M0 milestone.
See the [validation record](../03-development/hardware-validation.md).
Scope: M0 Step 2 of the
[master specification](../01-product/gnome_local_ai_workstation_master_specification.md#10-versioned-delivery-roadmap--milestone-acceptance-criteria).

## User outcome

Launching the app shows the machine's CPU, physical/logical core counts, total and
available system RAM, and detected GPU memory. A Refresh action captures a new
snapshot without freezing navigation. Missing GPU libraries, unsupported devices,
or permission failures leave usable CPU/RAM information visible with an explanation.

This gives the later model-fit and benchmark features a real hardware foundation.
It does not produce model recommendations or certify that a runtime can accelerate inference.

## Scope and decisions before coding

- Keep detection in `workstation-core::hardware` and widgets in `workstation-app`.
- Define the specification's hardware types with `Clone`, not `Copy` for enums
  containing strings. Resolve how unknown/unavailable readings are represented;
  never silently convert probe failure into zero available memory or a CPU-only claim.
- Keep dedicated VRAM, shared GPU memory, and host RAM separate in the display.
  Shared GPU memory overlaps host RAM; do not sum them into an inflated total.
- Use deterministic profile identity based on stable CPU/GPU identity and total
  RAM, independent of free memory, capture time, hostname, and enumeration order.
- Report supported/available backends only when the probe supplies evidence.
  Device presence alone is insufficient to claim a working inference backend.
- Verify current upstream APIs before selecting hardware dependencies. Add only
  the dependencies used by this slice, with versions centralized in Cargo.toml.

## Small implementation checkpoints

| Order | Change | Main paths | Proof |
| --- | --- | --- | --- |
| 1 | Hardware types, partial-failure representation, CPU topology/flags, RAM parsing, stable identity | `crates/workstation-core/src/hardware/mod.rs`, `cpu.rs`, core/workspace manifests | Fixture tests for topology, units, missing/malformed fields, and stable identity |
| 2 | Dynamically loaded NVIDIA NVML and read-only AMD DRM sysfs probes | `crates/workstation-core/src/hardware/nvml.rs`, `amd_sysfs.rs` | Tests for missing libraries, inaccessible nodes, multiple devices, device deduplication, and memory bounds |
| 3 | Detection command and snapshot/error events, using `spawn_blocking` | `crates/workstation-app/src/bridge.rs` | Tests for results, partial failures, repeated requests, and receiver closure |
| 4 | Overview cards, startup detection, manual Refresh, loading/error/unsupported states | `crates/workstation-app/src/views/overview.rs`, `window.rs` | GUI checks for navigation responsiveness, narrow layout, keyboard access, and recovery after refresh |
| 5 | Feature status, contributor instructions, and validation evidence | `README.md`, `CHANGELOG.md`, roadmap and development docs | Required project checks plus explicitly recorded live-hardware results |

Make a small, coherent commit at each working checkpoint. Keep all four crates;
no new hardware service, crate, or privileged helper is needed.

## Probe and UI behavior

- CPU/RAM probing uses Linux `/proc` and topology information as needed. Parse
  values defensively, check units/overflow, and distinguish unavailable topology
  from measured core counts.
- NVIDIA uses runtime-loaded NVML, with no hard CUDA link. An unavailable library
  is a recoverable probe result, not an application startup failure.
- AMD uses vendor-checked DRM sysfs entries for VRAM/GTT totals and usage. Handle
  disappearing devices and inconsistent snapshots without underflow. Do not infer
  an APU/unified memory classification solely from the existence of GTT.
- Unsupported Intel/Vulkan discovery is explicit in this first slice; broader
  backend detection remains later work. Do not classify an unknown GPU as absent.
- Reuse the existing typed Tokio/GLib bridge. GTK updates stay on the main thread;
  filesystem and NVML work runs in `spawn_blocking`.
- Allow one in-flight detection per window. Disable or coalesce Refresh while it
  runs, discard results after the window closes, and avoid repeated task buildup.
- Retain the last successful snapshot when refresh fails and mark it stale.
  No periodic polling, database writes, or network traffic is needed yet.

## Test fixtures and acceptance

Place core-specific sanitized fixtures under
`crates/workstation-core/tests/fixtures/`; executable integration tests belong in
that crate's `tests/` directory. Normal tests must work without GPU hardware, NVML,
running inference servers, model downloads, or internet access after dependencies are cached.

Acceptance requires:

1. Correct CPU/RAM information on a CPU-only Linux machine, with GPU absence or
   unsupported detection represented honestly.
2. Correct NVIDIA total/free VRAM and AMD VRAM/GTT reporting on the applicable
   real machines; record unavailable hardware checks as outstanding.
3. Missing optional probes do not hide successful readings or crash the app.
4. Hardware identity stays stable when availability changes or devices are enumerated differently.
5. Overview remains usable during detection, refresh, navigation, and window close.
6. Formatting, Clippy, workspace tests, and repository validation pass. Record the
   environment and any missing real-hardware coverage before marking this feature done.

## What follows

After this slice, implement attached Ollama discovery (`/api/version`, `/api/tags`,
`/api/ps`) in M0 Step 3. Then resolve the remaining memory-fit design questions
before Step 4, followed by the streaming benchmark and SQLite persistence in Step 5.

Model downloads/catalog search, managed-process control, fit badges, continuous
telemetry, exports, and the full Benchmark Lab remain outside this feature.
Completing hardware discovery alone does not complete M0.
