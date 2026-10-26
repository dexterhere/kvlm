# Benchmark Lab protocol

Milestone 3 phase 3c. `workstation-bench::lab` runs one experiment against a target and stores every pass as it finishes. It has no interface yet.

## Protocol

1. The experiment record is saved first. If it cannot be saved, or another experiment is still running, nothing runs.
2. An optional **cold load**: the target unloads and reloads the model, and the time until the runtime reports ready is stored as `load_ms`. For a Managed runtime this is a stop and start of the owned process. "Cold" means model-cold; the operating-system file cache is not dropped, and results are labelled that way.
3. One **warm-up** pass of the fixed workload, stored but left out of the summary.
4. **N measured passes** (1 to 20, default 3) of the same prompt, seed and token limit, with the prompt re-processed each time (`cache_prompt` off) so passes are comparable.
5. The runtime is asked what it actually loaded (`observed`), the summary is computed and the experiment is finished.

The standard workload is `lab-fixed-v1`: a short fixed prompt, seed 42, temperature 0, at most 96 generated tokens. The experiment record must describe exactly that workload (id, prompt hash, seed, token limit) or the run is refused.

## Failure and cancellation

- A failed cold load or warm-up ends the experiment as failed before any measured pass.
- A failed measured pass is stored as failed and left out of the medians, with a count. Two failed measured passes in a row end the experiment as failed.
- A pass that runs longer than 180 seconds is stopped and recorded as failed.
- Cancellation is checked before every pass and while one runs. The running pass is recorded as cancelled, the experiment as cancelled, and passes already finished keep their results.
- If the database fails during a run the measurements are kept in the returned outcome and the first storage error is reported; the result is shown rather than lost. An experiment left unfinished is marked interrupted at the next start.

## What each pass records

Wall time, time to first token, prompt and generated token counts, prompt and decode speed from the server's own counters, inter-token latency when valid (or the reason it is unknown), and, sampled at a 100 ms target while the pass runs, system CPU, owned-process memory and CPU time (Managed only), GPU busy percentage and dedicated GPU memory. See [metrics](benchmark-lab-metrics.md). Every sampled figure keeps its scope, sample count, covered time, longest gap and whether the sample cap was reached.

## Targets

`LabTarget` is the seam. `LlamaTarget` drives a managed llama.cpp server through the existing loopback client; the cold load is a callback supplied by the application, so this crate never reaches into process control. An Attached runtime is never started, stopped or modified by the protocol. Ollama (attached) joins in a later phase with its own cold-start consent.

## Not in this phase

The pre-flight reading (free memory, temperature) is not stored yet; peak memory during passes is. Findings, comparisons and the interface follow in the next phases.
