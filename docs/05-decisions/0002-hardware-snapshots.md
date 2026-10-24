# 0002: Hardware snapshots

Status: accepted for phase one. Date: 2026-10-24.

## Context

The specification's hardware structs are illustrative. Unknown topology, missing
drivers, unsupported GPU vendors, and unreadable memory counters must not become
zero-valued measurements, false CPU-only claims, or fabricated acceleration support.

## Decision

- `HardwareProfile` holds optional CPU/RAM readings, a GPU list, probe issues, a
  capture timestamp, and an optional stable profile hash. No hostname is collected
  in this slice. There is no assumed primary runtime/backend.
- `MemoryPool` requires a known total and permits unknown availability. Unknown
  pools are absent; measured zero free bytes remains a valid reading. CPU physical
  topology/instruction flags and GPU unified-memory classification can be unknown.
- CPU flags are intersected across logical processors. Physical cores count
  `(socket, core)` pairs; missing topology does not silently become a guessed count.
- System RAM uses checked `kB × 1024` parsing and `MemAvailable`, not `MemFree`.
- AMD VRAM and GTT are separate pools. Their existence does not classify an APU
  or create extra host capacity. Invalid usage leaves availability unknown and
  records a notice. Intel/other GPUs remain visible even though their memory probe
  is unimplemented.
- NVIDIA loads `libnvidia-ml.so.1` dynamically per explicit snapshot. Its reported
  free value is authoritative because driver reservations can affect accounting.
  PCI identifiers normalize to the same form as DRM, avoiding duplicate devices.
  CUDA capability is hardware/driver evidence, not proof that an inference runtime
  is installed or working. No ROCm/Vulkan/SYCL capability is claimed without a probe.
- SHA256 identity covers architecture, CPU model, sorted detected GPU IDs, and
  total RAM with unambiguous field lengths. Free memory, timestamps, hostnames,
  and enumeration order do not affect it. Missing identity inputs or an incomplete
  DRM inventory suppress the hash. It identifies the detected profile, not an
  assertion that every possible headless device was discoverable.
- Only implemented backend variants are exposed; string-bearing enums derive
  `Clone`, not `Copy`. Future runtime contracts must handle these optional readings
  rather than relying on the specification's illustrative flattened fields.

## Threading and recovery

The existing bridge uses bounded command/event channels and a single serialized
probe. Blocking OS/NVML work runs in `spawn_blocking`; GTK receives typed results
on GLib. Refresh requests coalesce, and closing the UI discards late results.
Blocking OS/driver calls cannot be forcibly stopped; the application's existing
two-second runtime shutdown wait remains the bound before process exit.

Usable partial results render with notices. A complete refresh failure keeps the
previous cards marked stale. There is no polling loop, storage, privileged helper,
runtime lifecycle operation, or network request in this slice.

## Verification and sources

See the [hardware validation record](../03-development/hardware-validation.md).
API decisions were checked against the primary
[NVML wrapper documentation](https://docs.rs/nvml-wrapper/0.13.0/nvml_wrapper/struct.NvmlBuilder.html),
[Linux proc documentation](https://www.kernel.org/doc/html/latest/filesystems/proc.html),
and [AMDGPU sysfs documentation](https://kernel.org/doc/html/latest/gpu/amdgpu/driver-misc.html).

Memory-fit classifications and benchmark verification still require separate
decisions before their implementation. This phase does not close those questions.
