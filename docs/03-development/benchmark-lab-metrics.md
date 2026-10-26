# Benchmark Lab metrics

Milestone 3 phase 3b. Pure building blocks for the per-pass figures; the protocol runner (3c) wires them to a live runtime.

## Inter-token latency

`workstation-bench::metrics::StreamRecord` keeps the arrival time of each non-empty content event. Inter-token latency (ITL) is reported only when all of these hold:

- the server reported how many tokens it generated;
- the number of content events equals that count;
- no reasoning events were streamed and the client combined no events;
- the stream did not exceed 8,192 events and the times are in order;
- there are at least 8 intervals.

Otherwise ITL is unknown and the record carries the reason in plain words, which the interface shows. Percentiles are nearest-rank over the sorted intervals (median is the 50th, the tail figure the 95th). Network chunks are never counted as tokens.

Time to first token is the arrival of the first content event. Decode and prompt speed come from the server's own token count and time (`tokens_per_second`); a zero count, a zero or negative time or a non-finite value gives unknown, never a speed.

## Resource load

`workstation-core::hardware::load` reads, from procfs, sysfs and NVML:

- system CPU busy percentage from `/proc/stat` differences, excluding idle and iowait;
- for a Managed runtime only, resident memory and CPU time summed over every live process in the owned process group (a scan of `/proc/<pid>/stat`, capped at 16,384 entries);
- GPU busy percentage from `gpu_busy_percent` on AMD and NVML utilization on NVIDIA.

An Attached runtime gets no process measurements: its process is not ours to inspect. A reading that cannot be taken is `None`. A group with no live member reads as unknown rather than idle.

`LoadAggregate` folds the samples of one pass into the `Sampled` figures stored with the pass: peak, mean, scope (device, system or owned process group), sample count, covered time, longest gap and whether the 2,000-sample cap was reached. With several GPUs the busiest device is reported. Process CPU time is the difference between the first and last group reading, so it undercounts if a process exits mid-pass.

The load probe runs inside the existing blocking probe of the memory sampler, so it inherits that sampler's 100 ms target, single in-flight probe, deadline and cancellation behavior.

## Limits

CPU ticks assume the Linux USER_HZ of 100. Utilization is sampled, not integrated, so short bursts between samples can be missed; the longest gap is recorded so that is visible.
