# Phase-one hardware validation

Date: 2026-10-24. Scope: hardware discovery in Overview, the first feature after
the foundation. This is not completion of all M0 acceptance criteria.

## Automated checks

- `cargo fmt --all --check` and workspace Clippy with `-D warnings`.
- Workspace tests: 19 tests, comprising 13 core hardware checks and six UI-state/
  worker checks. Tests require no GPU, installed driver, inference process, network,
  or model download after dependencies are cached.
- Core coverage includes CPU topology/flags, checked memory units and bounds,
  missing sources, CPU-only inventory, normalized PCI identity, duplicate DRM
  devices, reserved NVML memory, missing NVML library, separate AMD pools, invalid
  usage, and stable identity independent of availability/order/time.
- Worker/UI coverage includes usable partial results, failed/panicking probes,
  refresh coalescing, event-channel closure during a blocked probe, retaining stale
  readings, and recovery after refresh failure.
- Python unittest discovery: nine checks for repository/build helpers and Git policy.
- Repository links/identity check, native Meson build, all five Meson checks, and
  staged installation, including license texts.
- GitHub PR checks `git-policy` and `foundation` passed on the implementation.
  Consult [PR #1](https://github.com/dexterhere/kvlm/pull/1) for final-head check status.

## Live reference evidence

Read-only local probing detected an Intel i5-13420H with eight physical and twelve
logical cores, approximately 15.32 GiB usable system RAM, Intel graphics, and an
NVIDIA RTX 4050 Laptop GPU. Core topology was independently compared with `lscpu`.
NVML reported 6,439,305,216 total VRAM bytes, consistent with `nvidia-smi`'s 6141 MiB
display. Free memory varies with other workloads and is not a fixed expected value.

Intel memory/backend probing is explicitly unsupported and displayed as a notice,
not as zero memory or a missing GPU. No inference-runtime capability was certified.
Raw snapshots remain local; no device identifiers, hostname, or private paths are
included in this report.

## GUI and native packaging

The real application ran on an isolated, loopback-only GTK Broadway display with
a separate D-Bus session. Verified startup readings, button Refresh, `Ctrl+R`,
navigation after detection, and layouts at wide, 600-pixel, and 360-pixel widths.
The Refresh action updated both capture time and available RAM. NVIDIA cards
showed model, available/total VRAM, and reported CUDA compute capability. `Ctrl+Q`
exited with status 0. The test browser tab and display server were closed.

The Broadway backend emitted frame-timing warnings during interaction, and the
isolated session emitted portal notices. No GTK critical error or application panic
occurred. This does not replace native GNOME Wayland/X11 or full accessibility testing.

Native packaging was exercised with Meson 1.3.2, matching the hosted Ubuntu runner.
The bootstrap AppStream failure was fixed by supplying the real developer namespace
and repository URL; validation now runs without a severity override. Meson still
reports its pre-existing `configure_file(copy: true)` deprecation warning.

## Outstanding evidence

- Live AMD discrete/APU and a separate CPU-only Linux machine. Sanitized fixtures
  cover their implemented data paths; fixtures are not a substitute for these checks.
- Intel/Vulkan/SYCL/ROCm backend discovery, unified-memory classification, and
  live performance metrics remain outside this phase.
- The minimum declared toolchain/library versions and optimized release build
  were not independently exercised. Hosted CI validates the Ubuntu native library set.

Keep this coverage visible when reviewing promotion through develop and staging.
M0 still needs attached Ollama discovery, memory fit, and a persisted benchmark.
