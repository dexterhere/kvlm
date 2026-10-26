# M0 reference acceptance

Date: 2026-10-26. Feature implementation and the reference GTK/CUDA workflows pass.
Strict milestone sign-off remains open pending the GNOME-session/reference-criterion
decision described below. Integration and promotion are separate from this evidence.

## Reference machine and model

- Linux Wayland session: Hyprland. GNOME Shell/session is not installed.
- Intel Core i5-13420H, 8 physical / 12 logical cores; reported RAM 16,444,858,368 bytes.
- NVIDIA GeForce RTX 4050 Laptop GPU, with an additional Intel integrated GPU.
- Attached Ollama 0.40.6, cloud features disabled, local `qwen2.5-coder:7b`.
- Single merged Qwen2.5-Coder-7B-Instruct Q4_K_M GGUF: 4,683,073,536 bytes,
  339 tensors, architecture `qwen2`, 28 layers, 3,584 embedding dimensions,
  28 attention heads and 4 KV heads. Missing K/V head lengths default to 128 in
  the fit adapter under the supported GGUF rule.

The local file and Ollama manifest have different identities. The inspector does
not authenticate weights, and the benchmark badge is scoped to the Ollama manifest.

## Real fit workflow

The native GTK test selected the actual file and used the normal bridge/blocking
worker at both contexts. Inputs were one sequence, F16 keys and values, microbatch
512, and an explicit uniform-full-attention assumption. The worker read fresh CPU
and CUDA memory. Free GPU memory was 6,084,296,704 bytes at both readings.

| Component | 8,192 context | 32,768 context |
| --- | ---: | ---: |
| Weight estimate | 4,683,073,536 | 4,683,073,536 |
| KV cache | 469,762,048 | 1,879,048,192 |
| Compute buffers | 268,435,456 | 268,435,456 |
| CUDA runtime allowance | 367,001,600 | 367,001,600 |
| CUDA total | 5,788,272,640 | 7,197,558,784 |
| CUDA memory policy | Constrained | Not recommended |

CPU totals were 5,488,379,904 and 6,897,666,048 bytes; both exceeded the current
available-RAM policy budget while other applications occupied memory. These are
additional full-residence estimates against current free memory. They do not
reclaim an already loaded model, infer partial offload, or verify runtime settings.
The UI rendered the declarations, conditional classes, breakdowns and limits, and
cleared stale results when the scenario changed.

The blueprint's illustrative Comfortable-at-8K / Constrained-at-32K example is not
the result for this artifact and these budgets. Preserving the actual calculation
is required; changing labels to satisfy the example would hide the memory constraint.

## Real benchmark and restart

The native GTK benchmark test used actual discovery, model selection, local-only
guards, digest-pinned generation, memory sampling, SQLite persistence and verification.
It used a fresh isolated XDG data directory; no user history was overwritten.

| Observation | Result |
| --- | ---: |
| Output tokens | 128 |
| Server evaluation duration | 3,385,619,000 ns |
| Server decode rate | 37.81 tokens/s |
| Client first response text, including load | 9,004,983,322 ns |
| Server model load duration | 8,885,397,800 ns |
| Client elapsed | 12,391,728,274 ns |
| Completed samples | 124 |
| Observed driver VRAM peak | 4,805,033,984 bytes |
| Observed system RAM nonavailable peak | 12,153,520,128 bytes |
| Saved row | 1, Completed, matching snapshot/completion digests |

The visible `Benchmark Verified` indicator matched that saved row. A separate GTK
test process refreshed inventory and hardware, reopened the same SQLite file and
restored the badge without another inference request. The hardware identity used
`hardware-v2`. Peaks are sampled system/device observations, not process attribution
or certified true peaks. First response text is not token-level TTFT or ITL.

## Repeat the opt-in checks

Normal tests do not require a GPU, model, display, runtime or network access beyond
cached dependencies. These ignored tests require explicit environment values and a
working native GTK display. Run each test in a separate process: Rust's test runner
can use different OS threads even with `--test-threads=1`, which GTK does not allow.

Read-only real-file fit:

```sh
KVLM_ACCEPTANCE_GGUF=/absolute/path/to/model.gguf \
  cargo test --locked -p workstation-app desktop_fit_real_qwen_acceptance -- \
  --ignored --exact views::model_fit::smoke::desktop_fit_real_qwen_acceptance --nocapture
```

One actual inference request and then a separate-process restart check:

```sh
m0_data_dir=$(mktemp -d /tmp/kvlm-m0-acceptance-XXXXXXXX)
XDG_DATA_HOME="$m0_data_dir" KVLM_ACCEPTANCE_MODEL=qwen2.5-coder:7b \
  cargo test --locked -p workstation-app real_local_benchmark_and_saved_badge -- \
  --ignored --exact views::bench_lab::smoke::real_local_benchmark_and_saved_badge --nocapture
XDG_DATA_HOME="$m0_data_dir" KVLM_ACCEPTANCE_MODEL=qwen2.5-coder:7b \
  cargo test --locked -p workstation-app saved_badge_survives_process_restart -- \
  --ignored --exact views::bench_lab::smoke::saved_badge_survives_process_restart --nocapture
```

The first benchmark check requires a fresh database. The restart check requires that
saved run and must use the same XDG directory. The acceptance file is under
`$m0_data_dir/local-ai-workstation/workstation.db`; retain it only as long as needed.

## Validation and remaining sign-off

Formatting, workspace Clippy with warnings denied, all 160 normal Rust tests, the
repository validator and 9 Python tests passed. All 9 opt-in GTK tests also passed:
6 synthetic flows on an isolated loopback Broadway display and 3 real-model checks
on the native Wayland display. Native fit-rendering and stale-verification checks
also passed independently. Independent identity/query review found no concrete issues.

Local Meson is unavailable; packaging files did not change. Hosted CI validates the
native build separately. Native file-chooser interaction, manual accessibility,
GNOME-session behavior and the original illustrative fit classifications have not
been accepted by these checks. AMD/CPU physical matrices, process attribution and
controlled cold/warm comparisons remain later milestone work.

The implemented features may be reviewed and integrated now. Keep strict M0 sign-off
open unless the reference-machine evidence and documented limits are accepted.
