# Hardware validation procedures

These are the checks the owner or a tester runs on real machines. None has been run by the automated
build, and this page records no results. Each run produces one evidence record in the format at the end.

## Machines still needed

| Machine | What it proves |
| --- | --- |
| AMD discrete GPU, or AMD APU | Dedicated VRAM and shared GTT are read as two separate pools; shared memory is never added to host RAM |
| CPU-only x86-64 with AVX2 and no GPU | Overview, fit and a managed llama.cpp CPU run work with no GPU and no NVML library |
| A second NVIDIA GPU generation | Compute capability, driver and VRAM readings match `nvidia-smi` |
| A GNOME session (Wayland and X11) | Window sizing, shortcuts, theme and text scaling in the real shell |

The reference machine (Intel i5-13420H, RTX 4050 Laptop) is covered in
[Hardware validation](hardware-validation.md).

## Procedure on each machine

1. Build and test from a clean checkout: `cargo test --workspace --locked`.
2. Print the redacted report and keep it: `cargo run --locked -p workstation-app -- --report-hardware`.
   It omits device identifiers, the identity hash, hostnames, paths and the capture time. Read it before sharing.
3. Compare each reading with the operating system's own tool and note any difference:
   - CPU cores and flags: `lscpu`.
   - RAM: `free -b` (total) .
   - NVIDIA: `nvidia-smi --query-gpu=name,memory.total,compute_cap,driver_version --format=csv`.
   - AMD: `cat /sys/class/drm/card*/device/mem_info_vram_total /sys/class/drm/card*/device/mem_info_gtt_total`.
4. Start the application (`cargo run --locked -p workstation-app`). Check Overview shows the same numbers, that
   missing readings say "not measured", and that a GPU without a supported probe shows a notice rather than zero.
5. Optional, with a GGUF model you already have: run the memory-fit scenario in Overview and compare its
   weights and KV-cache numbers with what `llama-server` prints when it loads that model with the same settings.
6. Optional, with a managed llama.cpp install: Runtimes → install, start a profile, run one Lab experiment,
   and check that Cold and Warm passes complete and the resource summary says which channels were measured.

## Evidence record

Copy this into the pull request or issue that adds the result.

```text
Machine: <short description, no serial numbers or hostnames>
Date: <yyyy-mm-dd>
Application commit: <git rev-parse --short HEAD>
Report: <paste the --report-hardware output>
Differences from OS tools: <none | list>
Steps run: <numbers from the procedure>
Result: <pass | fail | partly> and what was seen
```

A result that was not observed is written as "not run", never as a pass.
