# M2 plan: hybrid runtime manager and managed llama.cpp

Date: 2026-10-27. Status: planned; implementation follows phase by phase. Source: the
[master specification](../../01-product/gnome_local_ai_workstation_master_specification.md)
section 5 and milestone M2.

## Outcome

The Runtimes page distinguishes **Attached** runtimes (started elsewhere, externally owned) from
**Managed** ones (started and owned by this application). For llama.cpp the user can install an
official portable `llama-server` build without root, define a launch profile, start, stop and
restart it, read bounded logs, serve an installed GGUF, stream a completion, and export developer
configuration. Attached runtimes can never be terminated or modified.

## Non-negotiables

- **Ownership guard in types, not UI.** Only a `ManagedRuntime` value (created by this application's
  supervisor) exposes stop, restart and kill. Attached values have no such methods, so a bug in the
  UI cannot signal an external PID. The supervisor signals only the child handles it spawned and never
  a PID read from a file or process list.
- **Loopback by default.** `bind_host` is `127.0.0.1`. Any other host is refused in M2 (no "listen on
  all interfaces" option). No API key is needed on loopback, but the profile rejects hosts that are not
  loopback literals.
- **No shell.** The command line is built as an argument vector from validated fields. The model path
  must be a regular file inside the application's model store or another path the user picked
  explicitly; it is passed as one argument. Free-form extra arguments are limited to an allow-list of
  flag names; values are validated. The child environment is cleared except for a small allow-list.
- **Verified installs only.** Runtime archives come from `github.com/ggml-org/llama.cpp` releases over
  https, from an allow-listed set of hosts, with no credentials. The asset's SHA-256 (published by the
  release API) must match before anything is extracted. Extraction rejects absolute paths, `..`,
  symlinks, hard links, devices, and enforces entry-count and total-size caps (zip-slip and zip-bomb
  defenses). The install directory is created 0700 and a version is published by atomic rename.
- **Bounded everything.** Log ring buffer (fixed lines and bytes per line), restart attempts (bounded with
  backoff), reader tasks (one per stream, ended with the child), health polling (single task, timeout per
  probe), response sizes. No unbounded queues between the child and the UI.
- **UI never blocks.** Spawning, waiting, polling and file work run on Tokio tasks or `spawn_blocking`;
  the GTK thread only receives `UiEvent`s. Progress/log events are coalesced.
- **Clean shutdown.** Managed children are terminated when the application exits (SIGTERM, then SIGKILL
  after a grace period). `kill_on_drop` covers a panic; a crash of the application itself is handled on
  next start by refusing to adopt unknown processes (it reports a port in use rather than killing).
- Missing data stays unknown. The application never claims `Ready` from process liveness alone: `Ready`
  needs `GET /health` to return 200.

## Design

### Data model (`workstation-core`)

- `runtime::OwnershipMode { Attached, Managed }`, `RuntimeLifecycleState` (as in the specification:
  NotInstalled, Detected, Starting, Ready, Busy, Stopping, Stopped, Failed with last log lines).
- `LaunchProfile`: name, model path or artifact id, host (loopback only), port, context length,
  GPU layers, K/V cache types, flash attention, ubatch size, threads, extra allowed flags. Strict
  `validate()` with ranges and enumerations; unit tests for every rejection.
- SQLite schema v3: `runtime_profiles` (as in the specification, minus the foreign key to a catalog table
  that does not exist; the artifact is referenced by id and path), and `runtime_installs` (engine, version,
  backend, path, sha256, installed_at). Stable ids; no secrets stored.

### Installer (`workstation-runtimes::llamacpp::install`)

1. `GET https://api.github.com/repos/ggml-org/llama.cpp/releases/latest` (explicit action, 4 MiB cap).
2. Choose the asset by platform and backend: Linux x86_64 `ubuntu-x64` (CPU) and `ubuntu-vulkan-x64`
   (Vulkan); CUDA bundles for Linux are not published, so CUDA users get Vulkan or a user-supplied path.
   The choice is shown to the user with the reason; nothing is silently downloaded.
3. Download to a partial file in the install store with the same size caps, no auto redirect except to
   allow-listed GitHub asset hosts, hash while streaming, require the API's `digest` (`sha256:`) to match.
4. Extract into a temporary directory with the safety rules above, make only `llama-server`
   (and its shared libraries) executable, verify `llama-server --version` runs with a timeout and clean
   environment, then rename into `runtimes/llamacpp/<tag>-<backend>/`.
5. Record the install; allow removal of one version (the application deletes only directories it created).

### Supervisor (`workstation-runtimes::supervisor`)

State machine owned by one Tokio task per managed runtime, driven by commands (`Start`, `Stop`,
`Restart`) and child events (exit, health result). Pre-flight: binary exists and is inside the install
store, model file readable, port free (bind probe on loopback; a conflict is reported, never "fixed" by
killing). Spawn with piped stdout/stderr, new process group, cleared environment. Two reader tasks feed a
shared ring buffer (`LogBuffer`: at most 2,000 lines, each cut at 4 KiB, control characters replaced).
Health: poll `/health` every 500 ms while Starting (503 = loading), every 5 s while Ready; start timeout
configurable (default 180 s). Failure recovery: unexpected exit moves to Failed with the last 50 log lines;
optional bounded auto-restart (default off; max 3 within 10 minutes with exponential backoff). Stop:
SIGTERM, wait 5 s, SIGKILL, reap; `Stopping` always ends in `Stopped` or `Failed`.

### Adapter (`workstation-runtimes::llamacpp`)

`GET /health`, `GET /props` (loaded model, context), `GET /slots` where enabled, `GET /metrics` parser
for Prometheus text (bounded body, known metric names only). Streaming inference over the OpenAI-
compatible `POST /v1/chat/completions` with `stream: true` (SSE, line-length and total caps, cancel on
drop) and the native timings object when present. Loopback-only client with no proxy and no redirects.

### Exports (`workstation-core::exports`)

Pure functions that turn a profile into text: Continue.dev `config.yaml` model entry, Zed `settings.json`
OpenAI-compatible provider entry, OpenCode `opencode.json` provider, and a `systemd --user` unit running
`llama-server` with the same argument vector (quoted per systemd rules). Output is shown and can be copied
or saved only to a path the user chooses; the application never edits existing editor config files and
never enables or starts a unit.

### UI and bridge

Runtimes page gains a "llama.cpp (Managed)" card: install/remove, profile editor (validated fields),
Start/Stop/Restart, lifecycle label with state, endpoint, loaded model, and a log dialog (ring buffer,
monospace, selectable, refreshed from coalesced events). Attached cards show "Attached, externally owned"
and no stop control. New `CoreCommand`/`UiEvent` pairs follow the existing id/stale-drop rules.

## Phases (one PR each, merged to `develop` when CI is green)

| Phase | Branch | Contents |
| --- | --- | --- |
| 2a | `feat/m2-runtime-profiles` | ownership/lifecycle types, `LaunchProfile` validation, argument builder, schema v3, store |
| 2b | `feat/m2-llamacpp-install` | release client, checksum, safe extraction, install store, tests with a local server and crafted archives |
| 2c | `feat/m2-supervisor` | process supervisor, ring log buffer, state machine, port pre-flight, stop/restart/recovery, tests with a fake child program |
| 2d | `feat/m2-llamacpp-adapter` | health/props/slots/metrics, streaming completion, tests with a local server |
| 2e | `feat/m2-dev-exports` | Continue.dev, Zed, OpenCode, systemd user unit generators and tests |
| 2f | `feat/m2-runtime-ui` | bridge commands, Runtimes managed card, profile editor, log dialog, smoke tests |
| 2g | `chore/m2-acceptance` | live run with a real `llama-server` and a small GGUF: startup failure, port conflict, shutdown, restart, real serving; evidence document |

## Edge cases covered by tests

Port already bound; model path missing or a directory or a symlink out of the allowed root; start while
starting; stop while stopping; child exits at startup; child ignores SIGTERM; child floods stdout;
child writes control characters and very long lines; health stays 503 past the timeout; two start
commands race; application shutdown with a running child; archive with `../` entries, absolute paths,
symlinks, duplicate names, oversized entries and a bad checksum; GitHub returns a redirect to a foreign
host; profile with a non-loopback host, shell metacharacters, negative or huge numbers, unknown flags.

## Deferred

Remote or LAN binding, API keys, multiple concurrent managed servers (one per profile is allowed by the
data model, but the UI manages one in M2), CUDA builds compiled by the application, automatic runtime
updates, adopting an already running `llama-server`.
