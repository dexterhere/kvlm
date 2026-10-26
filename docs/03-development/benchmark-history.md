# Benchmark history

Benchmark Lab retains up to the newest 100 saved runs in its history view across
application restarts. This dependent branch carries unmerged PR #8 and its
prerequisites; it does not merge or promote them.

Run [Quick Benchmark](quick-benchmark.md) as usual. Before inference, a private
SQLite database saves the selected model, fresh hardware snapshot, endpoint, and
requested workload settings. Afterwards, expand the saved entry to inspect its
outcome, runtime version when known, and separate client/server measurements.
Completed, failed, cancelled, and timed-out outcomes remain distinct. Missing
metrics stay Unknown. No prompt or generated response text is retained.

History loads automatically at startup and after a successful save; use Reload to
retry a storage error. Previously loaded rows remain visible but are marked stale
if a reload fails. Initial storage failure prevents inference. Final storage
failure leaves the current in-memory result visible and reports that saving failed.
The initial record may remain unfinished; reloading checks durable storage without
rerunning inference. A completed run can race with cancellation.

Closing the app or losing power can leave “Unfinished (running or interrupted)”
records. Another instance may still be running such a record, so startup does not
change it to failed or cancelled. The current result area remains session-only;
the history section is the durable record.

The database is `$XDG_DATA_HOME/local-ai-workstation/workstation.db`, or
`$HOME/.local/share/local-ai-workstation/workstation.db` by default. New directories
are 0700 and files 0600; existing directory permissions are preserved. History
rejects directories writable by group/others and database/companion files accessible
to group/others. Restored files need owner-only permissions before reopening. Records
include local hardware identifiers/names and reported model digests. There is no
upload or telemetry. Keep database contents and WAL companions out of Git. To back
up, close every app instance before copying the database and any companion files.
Do not delete or replace an unreadable database to make an error disappear; keep
it for recovery. Unknown/newer schemas are rejected without downgrade.

These snapshots do not verify local GGUF identity or effective runtime settings.
The subsequent [memory sampling slice](benchmark-memory-sampling.md) adds optional
observed RAM/VRAM/GTT peaks and coverage to new terminal records. Older records
remain unchanged. There is no Benchmark Verified badge, process attribution,
retention, export, deletion, or repeated-run aggregation. M0 remains incomplete.
See [decision 0010](../05-decisions/0010-benchmark-history.md) for storage details.

## Validation

Core tests reopen temporary databases, preserve all outcomes and unfinished rows,
exercise immutable completion and the newest-100 bound, and reject unsafe or
incompatible storage. Bench tests prove snapshot-before-inference, startup-write
failure blocking inference, final-write failure retaining the outcome, and aborted
workers leaving unfinished records. Bridge fixtures use an injected loader and
runner; normal tests never touch the user's database, a GPU, or a live model.

The optional GTK history flow uses typed fixtures, including long plain text,
unknown metrics, all outcomes, stale replies, reload recovery, and queued reloads:

```sh
cargo test -p workstation-app --locked \
  views::benchmark_history::smoke::history_load_retry_stale_save_and_outcome_flow -- --ignored --exact
```

Automated restart tests do not replace real-model, native GNOME, accessibility,
real-hardware, or resource-calibration acceptance.

At the integrated feature tip, formatting, Clippy with warnings denied, repository
validation, AppStream, desktop metadata, and GSettings checks passed. The Rust
workspace passed 128 tests, and nine Python helper tests passed. All four opt-in
GTK flow tests passed separately on an isolated Broadway display with GLib criticals
fatal. The history flow verifies plain long names, complete later metrics, narrow
layout, all outcomes, queued startup/reloads, stale replies, and storage recovery.
Independent review found an existing-file permission gap; rejecting unsafe restored
files and testing byte/mode preservation closed it. No review findings remain open.
Meson is unavailable in the local PATH; native build/install validation is delegated
to the PR's required foundation CI job. No real inference or hardware calibration
was run. Portal display warnings in the isolated GTK session are not native GNOME
or screen-reader acceptance.

## Exact benchmark verification, 2026-10-26

Benchmark Lab checks the current installed model against fresh Ollama inventory and
hardware, then looks for a successful saved request with matching manifest digests
and versioned hardware identity. It searches all matching history, including records
older than the newest 100 visible entries. The UI names the matching run and restores
the indicator after restart. Failed, cancelled, unfinished, malformed or mismatched
records cannot grant it. Old hardware hashes remain readable but unverified.

This is prior-request evidence for an Ollama manifest, not a GGUF file identity or a
guarantee for a future context, cache, backend or placement. See
[decision 0013](../05-decisions/0013-benchmark-verification.md) and the
[actual benchmark/restart evidence](m0-acceptance.md#real-benchmark-and-restart).
