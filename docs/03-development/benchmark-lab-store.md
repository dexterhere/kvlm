# Benchmark Lab store

Milestone 3 phase 3a. This is the persistence layer for reproducible experiments. It has no UI and no runner yet; later phases fill it.

## What is stored

Schema version 4 adds three tables to the existing local SQLite database:

- `experiments`: one row per experiment. It holds the engine and ownership, whether a cold load was requested, the number of measured passes requested, the workload id, the prompt hash, seed and token limit, the model name and hash, the runtime version and backend, a hardware hash, the requested settings, the observed settings, the summary, and an optional baseline link plus the single variable changed from that baseline.
- `experiment_passes`: one row per cold-load, warm-up or measured pass, with its state, metrics and failure text.
- `diagnostic_findings`: one row per advisor finding, including evidence and the inputs that were missing when a rule could not be evaluated.

Requested and observed settings are stored separately. `Settings::differences` names the fields where the runtime reported something other than what was asked for, so a comparison never silently trusts the request.

## Rules the store enforces

- One experiment may be running at a time. A stale running row blocks a new one until `recover_interrupted` marks it and its open passes as interrupted at the next start.
- Finished experiments and passes are final. A second finish, or a new pass on a finished experiment, is an error.
- Pass ordinals are unique per kind within an experiment.
- Only the newest 200 experiments are kept. Pruning removes their passes and findings, and a baseline link pointing at a pruned row is emptied rather than cascaded.
- A running experiment cannot be deleted.
- A row that fails to decode is counted in `Listing::unreadable` and skipped, so one damaged record never hides the rest of the history.
- Metrics that were not measured stay `None`. They are never stored as zero.

## Summaries

`summarize` reports the median, minimum, maximum and count of each metric over the successful measured passes. Failed or cancelled measured passes are counted as excluded and never averaged in. The cold load is summarized separately from the warm passes. Two spreads are called distinguishable only when their ranges do not overlap.

## Upgrade

A version 3 database upgrades in place and keeps its benchmark history and runtime profiles. The tests cover the upgrade and the older version 2 path.
