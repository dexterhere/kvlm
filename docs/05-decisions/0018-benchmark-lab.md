# 0018: Benchmark Lab

Date: 2026-10-27. Status: M3 implemented for managed llama.cpp.

## Decision

Reproducible experiments are a separate record from the quick benchmark history. An experiment fixes the workload (a
versioned prompt, seed and token limit), the settings the profile asked for, the settings the running process shows, the
model and runtime identities and the hardware hash, then stores every pass with its metrics and coverage.

- **Protocol.** An optional cold load (restart of the owned server, timed until it is ready), one warm-up pass that is
  discarded, then the requested number of measured passes. Two measured failures in a row end the run. A cancelled run is
  stored as cancelled with the passes it finished.
- **Metrics.** Speeds come from the server's own counters. Inter-token latency exists only when the stream proved one
  token per event; otherwise it is unknown and says why. Resident memory, CPU and GPU readings carry their scope
  (owned process group, system, device) and coverage; a pass too short to be sampled reports no reading, never zero.
- **Diagnostics.** Four rules (DIAG-01 to DIAG-04) evaluated from the stored passes. A rule whose inputs are missing says
  which, instead of passing.
- **Comparison.** Any two finished experiments of the same workload on the same hardware can be compared, with
  differences listed as notes. A *controlled* comparison changes exactly one declared profile setting and refuses unless
  everything else matches and the process's own command line shows the change took effect.
- **Ownership.** Only a server this application started is restarted. A cold load or a follow-up restarts it under a guard
  (expected generation, cancellation) and puts the user's profile back unless they started something else meanwhile.
  Nothing starts at launch, and a run needs an explicit Check and Run of the same request.
- **Not yet.** An attached Ollama is not a lab target. Offloaded layers are not read from the server log, so DIAG-01
  reports what it could not check.

## Consequences

The store is schema version 4 with a one-running-experiment rule and pruning that keeps a named baseline. Details:
[store](../03-development/benchmark-lab-store.md), [metrics](../03-development/benchmark-lab-metrics.md),
[protocol](../03-development/benchmark-lab-protocol.md), [diagnostics](../03-development/benchmark-lab-diagnostics.md),
[comparisons](../03-development/benchmark-lab-compare.md), [page](../03-development/benchmark-lab-ui.md).
