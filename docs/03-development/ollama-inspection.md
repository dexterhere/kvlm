# Ollama model inspection

Date: 2026-10-25. Scope: selection and read-only inspection in Runtimes, alongside
the existing Overview CPU scenario workflow. This dependent branch carries unmerged
PRs #2 and #6, including their prerequisites; none are merged by this work.

## Use the workflow

1. Start your existing Ollama service separately. The application does not install,
   start, stop, or reconfigure it.
2. Open Runtimes and select Refresh (`Ctrl+R` while that view is visible). No runtime
   HTTP request occurs automatically at application startup on this combined branch.
3. Select Inspect beside an installed model. The app reads `/api/show` and takes a
   fresh `/api/ps` snapshot without loading or running that model.
4. Compare declared training context and parameter defaults with loaded context and
   allocation. Missing fields stay Unknown. Inspect again for fresh readings.

An empty loaded list means the selected model has no matching loaded reading at that
observation. Missing, ambiguous, or mismatched name/digest association stays unverified.
Reported allocation is not independently measured RSS, GPU usage, or proof of spill.
Metadata and allocation queries are not an atomic snapshot and may race external changes.

Each endpoint may fail independently; usable results remain visible. Removed models,
HTTP errors, timeouts, and busy queues offer a retry. Refresh clears selection and
invalidates pending results; stale inventory cannot be used for a new inspection.
The UI displays bounded plain text, never evaluates templates, and never follows a
remote URL. See [decision 0008](../05-decisions/0008-ollama-model-inspection.md).

## Interpretation limits

- Declared parameters describe model defaults; they do not establish loaded settings.
- Actual K/V types, micro-batch size, concurrency, and runtime compatibility stay unknown.
- Positive remote metadata marks Cloud Routed; missing metadata leaves routing unverified.
- A runtime manifest digest is not a verified local GGUF digest. Overview file selection
  remains independent; inspection does not alter its inputs or grant positive fit.
- No inference, benchmark, download, load/unload, process signal, or database write occurs.

## Validation

Run the repository-required checks from [getting started](getting-started.md).
Ordinary tests use mock HTTP on ephemeral loopback ports, synthetic metadata, and
typed worker commands. They require no installed Ollama, GPU, model, or Internet
connection once Cargo dependencies are cached.

Validated on the combined feature tip:

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: 93 passed; two display tests ignored as intended.
- Repository validator and nine Python helper tests: passed.
- AppStream and strict GSettings validation: passed.
- Both opt-in GTK flow tests passed separately on an isolated GTK Broadway display,
  with GLib criticals treated as fatal. The new test drives Refresh and Inspect,
  checks distinct training/default/loaded contexts, exact reported allocation,
  stale replies, failure/retry, recovery, and populated 360px layout.
- Independent read-only review found no actionable findings. Root also reviewed
  request encoding, ownership, proxy/redirect policy, response bounds, and error
  suppression at the integrated tip.

The sandbox denies local socket binds; HTTP tests required permission to bind
loopback mock ports. The application still runs without root. The Broadway test
session emitted portal-backend availability warnings; neither product test emitted
a GTK critical or failed. This is automated widget-flow evidence, not visual,
native GNOME, keyboard, or screen-reader acceptance. A real Ollama model and full
M0 benchmark/restart acceptance remain unchecked. Meson is unavailable in the local
PATH; the PR's hosted foundation check must validate its build and staged install.

## Optional GTK checks

With a working GTK display, run these separately so each process initializes GTK
on its own test thread:

```sh
cargo test -p workstation-app --locked \
  views::runtimes::runtimes_smoke::installed_model_inspection_flow -- --ignored --exact
cargo test -p workstation-app --locked \
  views::model_fit::smoke::desktop_fit_widget_flow -- --ignored --exact
```

The inspection widget test uses typed fixtures, not HTTP. HTTP wire behavior and
worker cancellation are covered separately by the normal runtime and bridge tests.
