# Attached Ollama discovery validation

Date: 2026-10-24. Scope: M0 Step 3, read-only discovery on `codex/ollama-discovery`.

## Implemented behavior

Runtimes reads `/api/version`, `/api/tags`, and `/api/ps` from the existing service
at `http://127.0.0.1:11434`, on startup and manual Refresh (Ctrl+R). It reports version,
installed and loaded models, optional metadata, and runtime-reported byte allocations.
It does not install Ollama, start/stop services, run/load/unload models, download files,
or create a database. A refused connection does not establish whether Ollama is installed.

Version failure means unavailable. Installed and loaded inventories fail independently.
An empty list is distinct from an unavailable list. A full refresh failure preserves
the previous cards with a stale notice; retry can recover without restarting the app.
Five-second request deadlines include body reads; each response is limited to 2 MiB.
The UI shows at most 100 models per section and 1024 characters per displayed field,
with explicit truncation notices. Requests ignore environment proxies and reject redirects.

Positive remote metadata is marked Cloud Routed; missing metadata leaves routing
unverified. No model execution exists, and this is not yet an execution-time local-only
guard. Ollama's allocation remainder is not independently measured process RAM and
must not be described as proven memory spill. See [the decision](../05-decisions/0003-attached-ollama-discovery.md).

## Automated evidence

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: 19 tests passed (10 adapter, 4 bridge, 5 view).
- Python helper tests: 4 passed on the current foundation base.
- `python3 scripts/check_repository.py`: passed.
- Meson 1.3.2 configure/build and five tests (repository, Python, desktop, AppStream,
  GSettings): passed on this host. Installation into a repository-local `stage/usr`
  also passed; no files were installed into the host's `/usr`.

HTTP tests use only in-process servers on ephemeral loopback ports. They cover exact
GET paths, valid/empty/partial responses, malformed JSON and fields, missing arrays,
cloud metadata, checked memory subtraction, non-followed redirects, suppressed server
error bodies, declared/streamed size limits, unavailable endpoints, and slow headers/body.
Bridge tests cover typed delivery and cancellation on window closure. View tests cover
retry/stale state, exact byte formatting, unknown splits, row limits, and Unicode-safe
plain-text truncation. No test needs a model, inference process, GPU, or Internet service.

The sandbox on the development host denies loopback socket binds; HTTP tests passed
with execution permission to bind local test ports. This is a harness restriction,
not a runtime dependency on elevated OS privileges.

## GUI and live-service evidence

A direct read of `/api/version` found no service on port 11434. Live discovery against
a real Ollama installation was therefore not checked; no service or model was installed
for this task.

GTK Broadway checks used a disposable HTTP fixture, clearly labeled `fixture-only`.
Observed: unavailable service with retry, recovery to installed/loaded models, literal
markup-like model names, Cloud Routed warnings, exact CPU/GPU allocation values, one
failed inventory with the other still visible, stale cards after total failure, and
recovery to an empty inventory. Layout was checked at 1280, 600, and 360 pixels; narrow
navigation and footer remained reachable. This is not native GNOME/accessibility-reader
acceptance. The preview session emitted portal availability and Broadway frame-timing
warnings; its process ended with signal 15, so normal graphical quit was not verified.

Independent review identified excessive GTK row creation from large server lists;
bounded rendering fixed it. Follow-up review found no remaining actionable findings.
The checks above cover the integrated adapter, worker bridge, window, and documentation.

## Integration and remaining acceptance

This branch starts from the current `develop`. Hardware PR #1 remains unmerged.
Hardware tests and the Overview integration from that PR are
not present in this branch. Its branch-policy and AppStream fixes are also not inherited.
Resolve that merge, integrate from `develop`, and rerun the combined checks before
accepting the combined M0 hardware/runtime slice. Do not skip `develop`, `staging`, `main`.

For live acceptance, separately start an existing supported Ollama service, check version
and actual installed inventory, compare loaded allocations with `/api/ps`, then verify
unavailable/recovery behavior without signaling or changing that external process from
the app. Actual loaded-model evidence and native GNOME accessibility testing remain open.
Static fit, inference, benchmark persistence, and the broader M0 definition remain incomplete.
