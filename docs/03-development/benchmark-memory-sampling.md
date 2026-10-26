# Benchmark memory sampling

Quick Benchmark now records observed memory peaks alongside its result and saved
history. This dependent branch carries unmerged PR #9 and its prerequisites; no
integration branch is promoted or PR merged by this feature.

Start [Quick Benchmark](quick-benchmark.md) normally. No extra controls or runtime
setup are required. The application attempts a read every 100ms during runtime
checks and generation, after hardware discovery and the initial database write.
Reads run outside GTK and Tokio's asynchronous workers. A slow driver read cannot
delay the runner's completion/cancellation. Native-handle cleanup also runs on a
blocking worker. Cancellation cannot forcibly interrupt an already running driver
call, so its late reading is discarded.

The result and [history](benchmark-history.md) show:

- Observed maximum nonavailable **system RAM**, estimated as MemTotal minus
  MemAvailable. This includes other applications and is not Ollama's RSS.
- Per-device **driver-reported VRAM** usage. This can include other workloads,
  reserved allocations, and APU carveouts; it does not prove dedicated physical
  memory, model residency, execution placement, or inference compatibility.
- Per-device **GTT/shared memory** usage, separately. This overlaps system RAM;
  do not add these values or sum individual device maxima into a total footprint.
- Successful-reading counts per channel, total completed probes, target interval,
  measured window, and notices for missing readings, worker failure, an in-flight
  end probe, or reaching the 2,000-probe limit.

Peaks are maxima of collected observations and can miss activity between samples.
Short runs may finish with no completed sample. Unsupported, missing, or invalid
readings display Unknown; a measured zero remains zero. At most 16 devices with
stable PCI identity from the initial hardware snapshot are sampled. CPU-only
systems remain usable, and missing GPU observations never prove CPU execution.

Every terminal outcome can retain its partial summary. If final saving fails, the
current result stays visible with a separate storage error. Older saved records
show “Not sampled”; they are not rewritten. No credentials, prompts, responses,
raw time series, or telemetry uploads are added. See
[decision 0011](../05-decisions/0011-benchmark-memory-sampling.md) for the contract.

## Validation

Normal tests use fixtures and injected probes, without a GPU, model, or inference
service. Core fixtures cover separate AMD pools, invalid identities, missing
sources, and RAM arithmetic. Sampler tests cover unknown/zero values, stable device
identity, duplicate rejection, the sample cap, slow and panicking probes, and
cancellation/drop behavior. Persistence tests reopen new and legacy rows and prove
that existing completion JSON stays byte-for-byte unchanged. GTK flow fixtures
cover current and saved summaries and stale-result suppression.

Real NVIDIA/AMD sampling, native GNOME and screen-reader interaction, true peak
calibration, process attribution, and real-model restart acceptance remain unchecked.
This feature does not complete M0 or grant empirical verification.

At the integrated feature tip, formatting, workspace Clippy with warnings denied,
141 Rust tests, nine Python tests, repository validation, and AppStream/desktop /
GSettings metadata checks passed. All four optional GTK flow tests passed on an
isolated Broadway display with GLib criticals fatal; populated resource results fit
360px width. Independent review's coverage wording, missing-reading notices, and
native cleanup concerns were resolved before verification. Portal display warnings
in this isolated session are not native GNOME acceptance. Native Meson build and
installation checks run in the required PR foundation CI job.
