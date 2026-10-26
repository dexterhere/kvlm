# M3 plan: benchmark lab and diagnostics

Date: 2026-10-27. Status: planned. Extends the existing quick benchmark and history
([0009](../../05-decisions/0009-quick-benchmark.md), [0010](../../05-decisions/0010-benchmark-history.md),
[0011](../../05-decisions/0011-benchmark-memory-sampling.md)). It does not replace them.

## Outcome

A Benchmark Lab that runs a reproducible experiment (workload, runtime configuration, requested versus
observed settings), with an optional cold-load measurement, a deterministic warm-up and N measured passes
(default 3), samples resources at a 100 ms target, stores every result in SQLite, compares two
configurations side by side, runs four evidence-based diagnostic rules, and lets the user test one
recommended change as a controlled follow-up.

## Honesty rules

- **No invented tokens.** Inter-token latency (ITL) is computed only from arrival times of stream events that
  are proven to carry exactly one token each. A pass qualifies only when **all** hold: the number of non-empty
  content events equals the server-reported generated-token count (`timings.predicted_n` / `eval_count`); role,
  usage, keep-alive and empty events were excluded from that count; no event was merged by the client; and there
  are at least 8 intervals. Percentiles use nearest-rank on the sorted intervals. Otherwise ITL is **Unknown** for
  that pass and the interface says why. Decode speed uses the server's own counters, never `chunks / seconds`.
- **Diagnostics need evidence.** Each rule states the measured inputs it used and the thresholds; if an
  input is missing (no GPU utilization reading, no per-model allocation, no prompt-token count) the rule
  reports "not evaluated: missing X", never a guess. A triggered rule never states a certain cause: it
  says "consistent with" and names the follow-up that would test it.
- **Requested versus observed.** Settings are stored twice: what the profile asked for and what the
  runtime reported (loaded context, offloaded layers, KV type from logs/props where available). Differences
  are shown. Unobserved values are Unknown.
- **Sampling is system/device wide** unless a process id is known (managed runtimes only); the record
  says which. Peaks are sample maxima at a ~100 ms target, with the sample count and the longest gap.

## Design

### Cold-start semantics and consent

"Cold" means **model-cold**: the model is unloaded and loaded again, timed until the runtime reports Ready. It
does not drop the operating-system file cache, and results are labelled that way. For a Managed runtime this is a
stop/start of the owned process. For an Attached runtime it is allowed only through the runtime's own documented API
(Ollama `keep_alive: 0`), only after an explicit confirmation naming the model and warning that other clients may
be using it, and only if the unload is confirmed by the runtime afterwards; the application never signals the
process. If an unload cannot be confirmed, or another client loaded the model again, the pass is marked invalid.
After the run the previous load state is reported, not silently restored.

### Protocol (`workstation-bench`)

`Experiment { workload, profile snapshot, passes, cold: bool }` runs: pre-flight reading (free RAM/VRAM,
temperature when available, CPU load) -> optional cold load (unload then load, time until Ready) -> warm-up
pass (fixed 32-token prompt, seed 42, temperature 0, discarded) -> N measured passes with the same prompt
and seed -> summary (median and min/max across passes, never an average hiding a failed pass). A failed or
cancelled pass is stored as such and excluded from the medians with a visible count. Explicit user Check +
Run, local-only preflight at execution time, cancellation at any point, and one experiment at a time stay
as in M0/M1.

### Metrics

Per pass: TTFT, prompt tokens and prompt speed, generated tokens and decode speed (server counters), ITL
median and p95 when valid, peak process RAM (managed: VmRSS summed over the owned process group from `/proc`;
attached: unknown), peak and mean GPU memory, GPU utilization (NVML on NVIDIA, `gpu_busy_percent` sysfs on AMD where
readable, otherwise Unknown), CPU utilization (system `/proc/stat` deltas and owned-process CPU time), offloaded layers
when the runtime reports them. Every metric records its **scope** (device, system, owned process group, unknown), the
covered duration, the sample count, the longest gap, and whether the sample cap was reached. Probes stay single-flight
and cancellable; a stalled driver call cannot hold up a pass beyond its deadline. Sampler target 100 ms.

### Storage

Schema v4 adds `experiments`, `experiment_passes` and `diagnostic_findings` (ids, foreign keys, indexes,
enumerations by CHECK, bounded text). Experiments and passes are written when they start (`running`) and finished
with a terminal state; at startup any `running` record is marked `interrupted` and kept visible. Existing benchmark
history tables are untouched; the lab writes its own tables and keeps `PRAGMA foreign_keys = ON`. Retention prunes
completed and interrupted experiments together beyond a cap, in one transaction with their passes and findings.
Tests upgrade a v2 database through v3 and v4 and prove existing history rows are unchanged.

### A/B comparison

Two stored experiments of the same workload and hardware profile are compared per metric with the
difference and the pass spread; if spreads overlap the interface says the difference is not distinguishable.
Different hardware profile or workload makes the comparison refuse with the reason.

### Diagnostic Advisor (`workstation-core::diagnostics`)

Pure functions over a finished experiment: DIAG-01 spill/offload pressure (VRAM in use and RAM-resident model
allocation above 256 MiB, or offloaded layers below the layers the model can actually offload (block count + 1,
not the requested 99); a profile that intentionally requests zero GPU layers, and unified-memory APUs, are excluded
and reported as "not applicable"), DIAG-02 suspected CPU fallback (device GPU
utilization below 15% **and** the owned process's CPU time above 70% of one pass's wall time while a GPU backend was
requested; system-wide CPU load alone, or an Attached runtime without process evidence, leaves it "not evaluated"), DIAG-03 slow prompt processing
(warm TTFT above 3000 ms with more than 2048 prompt tokens), DIAG-04 generation jitter (ITL p95 above 2.5x the
median, only when ITL is valid). Output: rule id, severity, evidence list, what was not evaluated, suggested
follow-up.

### Controlled follow-up and comparison controls

From a finding the user can start a comparison that clones a **Managed** profile with exactly one declared change
(K/V cache `q8_0`, half the context, or flash attention on), runs the same workload, and shows the A/B result.
A comparison is valid only if everything except the declared variable matches: artifact sha256, runtime version
and backend, hardware profile, workload text, seed, maximum output tokens, and (stated) cache state. The run is
refused if the change is unsupported by the runtime, would truncate the prompt, or the observed settings do not
show that the change took effect. Attached runtimes are never modified, so follow-ups require a Managed profile
(M2). This is why the profile switching of M2 comes before any lab UI.

## Phases

| Phase | Branch | Contents |
| --- | --- | --- |
| 3a | `feat/m3-experiment-store` | schema v4, experiment/pass/finding records, requested vs observed, store tests |
| 3b | `feat/m3-metrics` | valid-ITL logic, CPU/process samplers, NVML utilization, token-event bookkeeping, tests |
| 3c | `feat/m3-protocol` | cold/warm protocol runner over the adapters, cancellation, failure handling, tests with fake adapters |
| 3d | `feat/m3-diagnostics` | DIAG-01..04, evidence model, boundary tests |
| 3e | `feat/m3-ab-compare` | comparison logic and refusals |
| 3f | `feat/m3-lab-ui` | Lab view, experiment list, A/B view, diagnostics panel, controlled follow-up, smoke tests |
| 3g | `chore/m3-acceptance` | live run on the reference machine, evidence document |

## Edge cases

Pass fails midway; runtime unloads the model during warm-up; zero generated tokens; one-token response; ITL with
batched chunks; GPU absent or unreadable; sampler gaps; clock steps (monotonic clocks only); cancellation during
cold load; duplicate experiments; comparing runs with different seeds; huge prompts; database write failure after
a long run (the result is shown and the failure reported).
