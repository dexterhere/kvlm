# 0011: Observed benchmark memory peaks

Date: 2026-10-26. Status: implemented on a dependent feature branch; integration pending.

## Decision

Collect system/device-wide memory observations around Quick Benchmark's runtime
checks and generation. The window starts after hardware discovery and the required
initial history write; it ends when the runner returns, before the final history
write. No process discovery, process signals, runtime changes, extra HTTP requests,
model downloads, or root access are introduced.

Core's `hardware::resources::MemoryProbe` fixes the device inventory to normalized
PCI identities in the run snapshot, at most 16 devices. AMD reads the existing
VRAM/GTT sysfs counters using the shared checked pool reader. NVIDIA initializes
NVML lazily once per run and resolves devices by PCI identity, rather than by a
changing device index. It reads the driver's reported `used` bytes, which may
include driver/reserved allocations, and validates counter consistency. Other
vendors and missing/invalid counters remain Unknown. Never substitute zero for an
unavailable reading. System RAM is `MemTotal - MemAvailable`: an estimate of
nonavailable system memory, not RSS or an inference process's allocation.

`workstation-bench::sampler` targets 100ms ticks with missed ticks skipped. Exactly
one blocking probe per run may be in flight; no async sampler task outlives its
owning benchmark future. Probe work and native-handle cleanup run in blocking
workers. A runner completion, cancellation, timeout, or failure never waits for a
slow in-flight probe; that reading is discarded and coverage is marked. An already
running blocking driver call cannot be forcibly cancelled. Application shutdown
retains its existing bounded wait. No driver timeout or true hard resource limit
is claimed.

Only maxima and successful-reading counts are retained, separately for system RAM
and each device's VRAM and GTT. Never add GTT to system RAM, combine device peaks,
or call these observations a model footprint. Sampling ends after at most 2,000
completed probes; the inference job continues with a limit notice. Missing
channels, a failed worker, an in-flight end probe, and the sample limit are visible
coverage limitations. Short runs can finish before any sample completes.

The terminal `RunCompletion` JSON gains an optional `resources` field. Missing
fields deserialize as None and None is omitted when writing. The existing schema
1 columns, application ID, WAL behavior, and payload bounds remain unchanged.
No migration or rewrite of existing rows is required. Old readers ignore the
additional field; immutable finalization prevents either version from replacing
an existing outcome. Tests preserve legacy completion JSON byte-for-byte while
reopening the database, and round-trip new summaries including missing/zero values
and unsigned counters. This follows additive migration guidance without a
structural schema change or a destructive rollback step.

Current results and saved history share a bounded plain-text formatter and display
sample counts and unknowns. Stale result IDs remain ignored. A final storage error
preserves the in-memory outcome and memory summary. No Benchmark Verified badge
is granted.

## Deferred

Actual process RSS, GPU utilization/temperature, CPU load, true allocation peaks,
time-series graphs, repeated cold/warm protocols, spill diagnosis, and artifact /
hardware verification remain separate work. Real hardware and native GNOME
acceptance are required before making calibrated fit or milestone-completion claims.

## Sources

- [Linux MemAvailable semantics](https://docs.kernel.org/filesystems/proc.html)
- [AMDGPU VRAM/GTT byte counters](https://docs.kernel.org/gpu/amdgpu/driver-misc.html)
- [NVML memory counters](https://docs.nvidia.com/deploy/nvml-api/latest/api/structnvmlMemory__t.html)
