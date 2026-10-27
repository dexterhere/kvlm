# What has been tested, and what has not

Kvlm is validated on one real machine so far, plus automated tests that need no GPU, model or network.
This page keeps the two apart. "Tested here" means a person or script ran it on the reference machine below and
the result is recorded; "not tested" means exactly that.

## Reference machine

| Part | Value |
| --- | --- |
| CPU | 13th Gen Intel Core i5-13420H, 12 threads |
| RAM | 15.3 GiB |
| GPU | NVIDIA GeForce RTX 4050 Laptop, 6 GiB VRAM (plus an Intel integrated GPU) |
| OS | Arch-based Linux with a Hyprland session (not a full GNOME session) |

## Tested on the reference machine

| Area | Result |
| --- | --- |
| Hardware probe (CPU features, NVIDIA via NVML, Intel iGPU listed) | Works; `--report-hardware` output reviewed for redaction |
| Meson build, user-prefix install, uninstall (`scripts/install.sh`) | Works with a temporary prefix |
| First-run setup guide | Appears on first launch |
| vLLM (attached, Qwen2.5-1.5B-Instruct) | Reachable on loopback; timed by `scripts/compare_runtimes.py` |
| Ollama (attached, Qwen2.5-Coder 7B Q4_K_M) | Reachable on loopback; timed |
| llama.cpp CUDA server (unmanaged binary, same 1.5B file as the others) | Served on GPU and timed; Kvlm's managed GPU install and supervisor not yet run on GPU |
| LM Studio (attached, Qwen2.5-Coder 7B Q4_K_M) | Server reachable on loopback; timed. Not exercised through the app UI |

Benchmark numbers live in [runtime benchmarks](runtime-benchmarks.md).

## Not tested yet

| Area | Why | Planned |
| --- | --- | --- |
| Full GNOME session (Wayland and X11), Orca, large text, reduced motion | Reference machine runs a tiling compositor | A GNOME distribution install, to be set up by the owner |
| CPU-only machine (older Intel i3, no discrete GPU) | Second machine not yet used | Run `--report-hardware` and the CPU fit scenario there |
| AMD GPU or APU, a second NVIDIA generation | No such hardware | Community reports, see [hardware validation procedures](hardware-validation-procedures.md) |
| GPU builds of llama.cpp serving a model, editor imports, `systemd --user` unit | Not yet exercised | Reference machine, next |
| Gated Hugging Face model with a token | Needs the owner's token and a keyring prompt | Owner |
| Installing on a clean machine of another distribution | Only one distribution available | Community reports |

Anyone can help: run `local-ai-workstation --report-hardware`, read the output, and open an issue with the parts you
are happy to share.
