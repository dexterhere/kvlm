# Benchmark Lab comparisons and controlled follow-ups

Milestone 3 phase 3e. Pure logic in `workstation-core::compare` and `workstation-core::experiments::follow_up`.

## A/B comparison

`compare(a, b)` compares two finished experiments, metric by metric: decode and prompt speed, time to first token, pass time, inter-token latency (median and 95th percentile), cold load time, process memory, GPU memory and the two mean utilizations. It works from the stored passes, not from a cached summary. Each metric shows both sides' median, minimum, maximum and pass count, the change in percent, and whether the difference is **distinguishable**: only when each side has at least two passes and their ranges do not overlap. With fewer passes it says it cannot tell. A metric one side did not measure stays unknown.

It **refuses**, with the reason in plain words, when an experiment is still running or has no completed measured pass, when the runtimes differ, or when the workload, prompt, seed, maximum output length or hardware profile differ or are not recorded on either side. Differences in model, runtime build, backend, cold versus warm, or managed versus attached do not refuse; they are listed as notes so the reader knows what else changed. Failed or cancelled passes left out of the medians are counted in a note.

## Controlled follow-up

`compare_follow_up(baseline, variant)` is stricter. The variant must be marked as a follow-up of exactly this baseline, both must be Managed (an attached runtime is never changed), and everything except the declared variable must match: the model file hash, runtime version and backend (all recorded on both sides), hardware profile, workload text, seed, maximum output, and cache state. The requested settings may differ only in the declared field, and the running process must show the change: the observed settings of the variant must carry the new value and the baseline must not have had it, with no other observed setting differing.

Observed settings come from the process's own command line (`observed_from_arguments`), because llama-server's API does not report cache types or flash attention. That is evidence the change reached the process, not the runtime's own report of what it allocated.

The three declared variables each change one profile field:

- **K cache type q8_0**: the K cache only. The V cache stays as it was, because a quantized V cache also requires flash attention and would be a second change.
- **Flash attention on.**
- **Half the context.**

`with_variable` builds the changed profile and refuses when the change is a no-op (already q8_0, already on), when half the context is below the launcher's minimum, when the baseline's prompt length is unknown, or when the prompt plus the output limit would not fit in the half context divided by the server's slots (4 unless the profile sets `--parallel`).
