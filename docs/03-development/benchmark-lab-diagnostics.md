# Benchmark Lab diagnostics

Milestone 3 phase 3d. `workstation-core::diagnostics::evaluate` is a pure function over a finished experiment. It returns exactly four findings, DIAG-01 to DIAG-04, in that order. Each finding is one of:

- **Triggered**: the measurements are *consistent with* a condition. A finding never states a certain cause, and it names the controlled change that would test it where one exists.
- **Clear**: the inputs were present and did not meet the condition.
- **Not evaluated**: an input was missing; the finding lists it ("Not evaluated: missing GPU utilization reading").
- **Not applicable**: the rule does not fit this setup (for example a CPU-only profile).

Every finding carries the measured inputs it used as `name: value` lines. Only completed measured passes feed a rule. A rule combines inputs only within the same pass, so two passes that each hold half of the evidence never add up to a finding; the figure used is the median over the passes that hold all of a rule's inputs. Thresholds are strict: a value equal to the threshold does not trigger, and a non-finite reading is ignored.

## Rules

| Rule | Triggers when | Test it with |
| --- | --- | --- |
| DIAG-01 spill or offload pressure | GPU memory is in use and the runtime reports more than 256 MiB of model memory in system RAM, or it reports fewer offloaded layers than the model can offload (block count plus the output layer, not the requested 99) | smaller K cache (`q8_0`) |
| DIAG-02 suspected CPU fallback | a GPU backend was requested, mean GPU utilization is below 15% **and** the owned process used more than 70% of the pass's wall time in CPU | none; check the build and offload |
| DIAG-03 slow prompt processing | median warm time to first token above 3000 ms with more than 2048 prompt tokens | flash attention on |
| DIAG-04 generation jitter | the 95th-percentile inter-token latency is more than 2.5 times the median, from valid inter-token latency only | none |

DIAG-01 is not applicable to a profile that requests no GPU (backend CPU or zero GPU layers) or to a machine with one shared memory pool. DIAG-02 is not evaluated for an attached runtime: system-wide CPU load alone is never treated as evidence, so without the owned process's CPU time the rule says what is missing. DIAG-03 is not applicable to a prompt of 2048 tokens or fewer, which includes the standard lab workload; it needs a long-prompt workload to run.

## Limits

DIAG-01 never uses the process's resident size as model allocation: that figure also counts mapped files and the runtime itself, so a fully offloaded model can show hundreds of megabytes. The memory signal needs the runtime to report model memory held in system RAM (for example Ollama's loaded-model sizes); llama.cpp does not, so for that engine the finding rests on the layer signal. If only one of the two signals can be checked and it does not trigger, the finding is Clear but lists the unchecked signal in `missing` and in its message. A machine whose memory topology is unknown is Not evaluated rather than assumed to be dedicated.

The model layer count and the unified-memory flag come from the caller (`Context`), because the experiment record does not hold them.
