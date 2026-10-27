# M2 acceptance evidence

Date: 2026-10-27. Status: **M2 is implemented on `develop`; evidence below is from one reference machine and does not
claim hardware or desktop coverage beyond it.**

M2's definition of done ([plan](../04-roadmap/m2/Planned.md)) covers: attached and managed ownership, a portable
official llama.cpp install with checksums and no root, supervision (start, stop, restart, recovery, bounded logs), serving
an installed GGUF with readiness, loaded-model facts and streaming inference, and developer exports. Acceptance
scenarios: startup failures, port conflicts, shutdown, restart and real model serving.

## Reference machine

Linux, Hyprland (Wayland) session, Intel Core i5-13420H, NVIDIA GeForce RTX 4050 Laptop GPU (driver 615.78), 16 GB RAM.
The machine offers CPU, Vulkan and CUDA builds. **Only the CPU build was served in this run.** A GNOME Shell session
is not installed.

## Live run with the real server

Command: `cargo run --locked -p workstation-runtimes --example m2_acceptance -- <empty dir> <step>`. Everything lived in a
temporary directory; the application's own data and the desktop keyring were not touched. The model was
`ggml-org/tiny-llamas` `stories260K.gguf` (1,185,376 bytes, SHA-256 `047bf464…c04b`), a toy model chosen for size.

| Scenario | Result |
| --- | --- |
| Install | Release b11910, CPU build, one archive of 22,474,744 bytes, SHA-256 `811603cd…e0d0` checked, extracted and published without root |
| Start and readiness | `Starting` then `Ready` in 0.5 s; the identity check passed (health and launch alias) |
| Loopback only | `ss` listed `127.0.0.1:<port>` only |
| Loaded-model facts | Server reported its own context (128 tokens, below the 512 requested: the model's limit), four idle slots, `busy` = false |
| Streaming inference | 24 content events; first content 6.1 ms; finish reason `length`; usage 46 prompt and 24 completion tokens; server timings 5.08 ms prompt, 10.2 ms decode. Network chunks were not counted as tokens |
| Restart | New process and new launch alias, `Ready` again in 0.5 s |
| Crash and recovery | `SIGKILL` of the server showed `Failed` then an automatic restart (1 of at most 3) back to `Ready` with a new pid in 1.6 s |
| Shutdown | Stop took 26 ms; the port was free again |
| Port conflict | Starting on an occupied port was refused with `PortInUse`; nothing was signalled or spawned |
| Orphan after an abrupt exit | The example aborted with the server still running; the next launch's check reported `Reclaimed`, and the port was free afterwards |
| Application start and quit | The GTK application launched on the reference session with an isolated data directory, created its runtime directory, printed nothing, and exited on SIGTERM without needing SIGKILL |

## Automated coverage (no GPU, model, process of record or network needed)

- Profile validation, argument vector and supervisor-owned flags; capability table; schema upgrade from v1 and v2.
- Installer: checksum mismatch, hostile archives (links, traversal, oversized entries, entry counts, pax and sparse
  headers), foreign redirects, cancellation at every stage, disk reserve, concurrent installs.
- Supervisor (21 integration tests with a fake server binary): pre-flight refusals, startup failure, early exit, start
  timeout, wrong identity on the port, crash and bounded auto-restart, stop with a stubborn descendant, stuck child, record
  and reclaim safety, concurrent supervisors, log bounds and sanitizing.
- Client: readiness, props, slots, metrics, streaming limits, cancellation before send and between buffered events,
  incomplete streams, event cap.
- Exports: JSON parsed, YAML scalars quoted, systemd words escaped, `systemd-analyze --user verify` on a hostile-path
  unit.
- Interface: bridge round trips with an in-memory backend, the card's control rules, editor validation, and two opt-in
  GTK smoke flows (`--include-ignored`) that passed on the reference session.

## Not claimed

- GPU builds (Vulkan, CUDA) were not served; the CUDA runtime bundle path is covered by the installer's unit tests only.
- A real chat model: the model used is a 1 MB toy; no quality or speed figure from it means anything.
- AMD APU, CPU-only AVX2 and other machines, and GNOME Shell behavior (M4).
- The Continue.dev, Zed and OpenCode texts were checked as syntax, not imported into those editors. The systemd unit was
  verified with `systemd-analyze` and was never installed, enabled or started.
- The interface flows were exercised by smoke tests and one launch, not by a person clicking through them.
