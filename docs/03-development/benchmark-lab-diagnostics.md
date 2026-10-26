# Benchmark Lab diagnostics

Milestone 3 phase 3d. `workstation-core::diagnostics::evaluate` is a pure function over a finished experiment. It returns exactly four findings, DIAG-01 to DIAG-04, in that order. Each finding is one of:

- **Triggered**: the measurements are *consistent with* a condition. A finding never states a certain cause, and it names the controlled change that would test it where one exists.
- **Clear**: the inputs were present and did not meet the condition.
- **Not evaluated**: an input was missing; the finding lists it ("Not evaluated: missing GPU utilization reading").
- **Not applicable**: the rule does not fit this setup (for example a CPU-only profile).

Every finding carries the measured inputs it used as `name: value` lines. Only completed measured passes feed a rule, and each input is the median across those passes. Thresholds are strict: a value equal to the threshold does not trigger.

## Rules

| Rule | Triggers when | Test it with |
| --- | --- | --- |
| DIAG-01 spill or offload pressure | GPU memory is in use and the owned process holds more than 256 MiB in system RAM, or the runtime reports fewer offloaded layers than the model can offload (block count plus the output layer, not the requested 99) | smaller K/V cache (`q8_0`) |
| DIAG-02 suspected CPU fallback | a GPU backend was requested, mean GPU utilization is below 15% **and** the owned process used more than 70% of the pass's wall time in CPU | none; check the build and offload |
| DIAG-03 slow prompt processing | median warm time to first token above 3000 ms with more than 2048 prompt tokens | flash attention on |
| DIAG-04 generation jitter | the 95th-percentile inter-token latency is more than 2.5 times the median, from valid inter-token latency only | none |

DIAG-01 is not applicable to a profile that requests no GPU (backend CPU or zero GPU layers) or to a machine with one shared memory pool. DIAG-02 is not evaluated for an attached runtime: system-wide CPU load alone is never treated as evidence, so without the owned process's CPU time the rule says what is missing. DIAG-03 is not applicable to a prompt of 2048 tokens or fewer, which includes the standard lab workload; it needs a long-prompt workload to run.

## Limits

DIAG-01 reads the process's resident memory, which can include pages of the memory-mapped model file even when every layer is on the GPU, so the memory signal is a hint, not a measurement of where the weights live. The layer count signal is stronger but needs the runtime to report offloaded layers and the model's layer count; llama.cpp's properties endpoint does not report offloaded layers, so for that engine the finding often rests on the memory signal alone and says so in its evidence.

The model layer count and the unified-memory flag come from the caller (`Context`), because the experiment record does not hold them.
