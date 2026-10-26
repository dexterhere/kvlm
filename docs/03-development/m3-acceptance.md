# M3 acceptance evidence

Date: 2026-10-27. Status: **M3 is implemented on `develop` for managed llama.cpp; evidence below is from one reference
machine and does not claim hardware or desktop coverage beyond it.**

M3's definition of done ([plan](../04-roadmap/m3/Planned.md)) covers reproducible experiments, a cold/warm protocol with
measured passes, sampled resource metrics, A/B comparisons in the local database and diagnostic findings with a
controlled follow-up.

## Reference machine

Linux, Hyprland (Wayland) session, Intel Core i5-13420H, NVIDIA GeForce RTX 4050 Laptop GPU, 16 GB RAM. **Only the CPU
build of llama.cpp b11910 was served in this run.** A GNOME Shell session is not installed.

## Live run through the production lab path

An opt-in test (`lab::acceptance` in `workstation-app`, run with `--ignored`) drives the same code the Lab page uses: the
real managed supervisor, the installed `llama-server`, the real experiment store and the real model, without GTK. It
refuses to run unless an isolated, empty data directory is given; the user's data and keyring were not touched. The model
was the 1 MB `stories260K.gguf` toy model, so no speed figure means anything beyond showing the pipeline works.

| Scenario | Result |
| --- | --- |
| Run bound to a different server | A Run carrying another server's identity was refused: "The managed server changed since Check. Press Check again." |
| Cold baseline | Check described the restart; the server was restarted and the load timed (about 0.5 s); one warm-up and three measured passes completed; experiment stored as completed |
| Settings evidence | The requested settings and the settings read from the running process's command line agreed for every field |
| Diagnostics | Four findings stored: DIAG-01 and DIAG-02 not applicable (CPU profile), DIAG-03 not applicable (97-token prompt), DIAG-04 clear |
| Controlled follow-up | Flash attention switched on for a second run of the same model, build and cache state (cold, inherited from the baseline); the command line of the restarted process showed flash attention on |
| Restore | After the follow-up the running profile equalled the user's original profile |
| Comparison | The controlled comparison was accepted; prompt speed and first-token time were reported as clearly different, decode speed as within run-to-run spread |
| Cleanup | No `llama-server` was left running |

A first attempt compared a cold baseline with a warm follow-up and was correctly refused; the lab now keeps the baseline's
cache state for a follow-up.

Resident-memory, CPU and GPU readings were **not** produced in this run: each pass lasted about 25 ms, shorter than one
100 ms sample, and the lab reports such readings as not measured rather than zero. They are covered by unit tests with
scripted readings, not by this live run.

## Automated coverage (no GPU, model, process or network needed)

- Store: lifecycle, one-running rule, recovery, pruning with a named baseline, number round-trips, unreadable rows.
- Metrics: valid-only inter-token latency, nearest-rank percentiles, counter-based speeds, sampling scopes and coverage.
- Protocol: cold and warm passes, failure and cancellation rules, cancel-versus-result races, storage failures.
- Diagnostics: per-pass evaluation, not-evaluated reporting, thresholds.
- Comparison: refusals, controlled-change rules, distinguishable-from-noise checks.
- Page and backend: command/event round trips with a fake backend, one run at a time, panic containment, and an opt-in
  GTK smoke test of Check, Run, Cancel, follow-up and comparison.

## Not claimed

- GPU builds (Vulkan, CUDA) were not run through the lab; layers offloaded to the GPU are not read from the server, so
  DIAG-01 cannot yet say a model spilled.
- An attached Ollama is not a lab target yet.
- A real chat model: no quality or speed figure from the toy model is meaningful.
- AMD APU, CPU-only AVX2 and other machines, and GNOME Shell behavior (M4).
- The page was exercised by a smoke test, not by a person clicking through it.
