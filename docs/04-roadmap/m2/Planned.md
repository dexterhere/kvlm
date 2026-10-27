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
- **No shell, no free-form flags.** The command line is an argument vector built from validated fields. The model
  path must be a regular file (no-follow open) inside the application's model store, or a path the user chose
  explicitly; it is one argument. There is no free-text extra-argument field: optional flags are a closed enum
  (threads, parallel slots, no-mmap, mlock) with typed values. `--host`, `--port`, `--model`, `--alias`,
  `--api-key*`, config/preset loading, `--hf-*`/download options and any alias of them are supervisor-owned and
  can never come from the profile; each flag is emitted exactly once. The child environment is cleared except
  for a small allow-list. Only binaries inside the application's install store are ever launched (there is no
  "use my own binary" option in M2).
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
- **Clean shutdown and crash containment.** Each child starts in its own process group
  (`CommandExt::process_group(0)`), so a stop signals the whole group with the safe `rustix` API. On normal exit
  and on SIGINT/SIGTERM of the application the supervisor sends SIGTERM, waits, then SIGKILLs the group and
  reaps it. `kill_on_drop` covers a panic but not an abort or SIGKILL, so the supervisor also writes a private
  state record (`runtimes/managed.json`, 0600: pid, `/proc/<pid>/stat` start time, expected executable path,
  launch generation). On the next start the application terminates a recorded process **only** if the pid is
  alive, its start time matches, and `/proc/<pid>/exe` is the recorded binary inside the install store; anything
  else is left alone and reported as a port conflict. Orphans from a crash are therefore reclaimed without ever
  signalling a pid the application cannot prove it started.
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
2. Release selection (checked against upstream on 2026-10-27): `releases/latest` is not usable (it points at a
   nightly marker) and every build is flagged "pre-release", so the client lists `GET /repos/ggml-org/llama.cpp/releases?per_page=20`
   (metadata cap 8 MiB) and takes the newest non-draft release that has the wanted Linux `x86_64` asset **with a
   published `sha256:` digest**; a missing digest means refusal. Linux assets are `.tar.gz`
   (`llama-<tag>-bin-ubuntu-x64`, `-vulkan-x64`, `-cuda-12.8-x64` plus a separate `cudart-` runtime bundle,
   `-rocm-*`). The backend offered follows detection: CPU always; Vulkan when a Vulkan ICD exists; CUDA 12.8 only
   when the NVIDIA driver reports a compatible CUDA version (the cudart bundle is large and is shown with its size
   before any download). glibc/architecture are checked before offering anything. Nothing downloads without the
   user's explicit choice.
3. Download to a partial file in the install store (archive cap 1 GiB, extracted cap 2.5 GiB, entry cap 2,000),
   redirects followed by hand only to allow-listed GitHub asset hosts over https, SHA-256 hashed while streaming
   and required to equal the API digest before extraction.
4. Extract with a streaming gzip+tar reader into a private staging directory under the install root (same
   filesystem): only regular files and directories are created, via no-follow creates; the only symlinks allowed are
   relative links to a sibling file name (shared-library version links) and are checked after extraction;
   byte limits count actual decompressed output. Generated directory names match `[A-Za-z0-9._-]{1,64}`. Make only
   the server and libraries executable, verify `llama-server --version` runs (timeout, cleared environment), then
   publish with an atomic no-replace rename into `runtimes/llamacpp/<tag>-<backend>/`. Removal refuses while a managed
   process runs from that version and only deletes a directory the application created (recorded in `runtime_installs`).
   Tests cover interrupted publication, disk full, concurrent install/remove and removal of an active version.
5. Record the install; allow removal of one version (the application deletes only directories it created).

### Supervisor (`workstation-runtimes::supervisor`)

State machine owned by one Tokio task per managed runtime, driven by commands (`Start`, `Stop`,
`Restart`) and child events (exit, health result). Pre-flight: binary exists and is inside the install
store, model file readable, port free (bind probe on loopback; a conflict is reported, never "fixed" by
killing). A free port is not an identity: every launch gets a random `--alias` (the launch generation) and readiness
requires `/health` = 200 **and** `/v1/models` listing exactly that alias **and** the child still alive, so another
listener answering `/health` can never make a launch look Ready; a startup timeout kills the group and reports why.
Spawn with piped stdout/stderr, new process group, cleared environment. Two reader tasks feed a
shared ring buffer (`LogBuffer`: at most 2,000 lines, each cut at 4 KiB, control characters replaced).
Health: poll `/health` every 500 ms while Starting (503 = loading), every 5 s while Ready; start timeout
configurable (default 180 s). Failure recovery: unexpected exit moves to Failed with the last 50 log lines;
optional bounded auto-restart (default off; max 3 within 10 minutes with exponential backoff). Stop:
SIGTERM, wait 5 s, SIGKILL, reap; `Stopping` always ends in `Stopped` or `Failed`.

### Capability contract (first, phase 2a)

The `RuntimeAdapter` contract of specification 5.3 is defined once as typed capabilities (probe, list models,
loaded models, start/stop managed, load/unload, stream inference). Operations an engine or ownership mode does not
support return a typed `Unsupported` error, never a silent no-op. `Busy` is derived from real signals
(`/slots` or `/metrics` request counters), not assumed. The existing Ollama adapter is mapped onto it without
behaviour change; Attached values simply lack the managed operations.

### Adapter (`workstation-runtimes::llamacpp`)

`GET /health`, `GET /props` (loaded model, context), `GET /slots` where enabled, `GET /metrics` parser
for Prometheus text (bounded body, known metric names only). Streaming inference over the OpenAI-
compatible `POST /v1/chat/completions` with `stream: true` (SSE, line-length and total caps, cancel on
drop) and the native timings object when present. Loopback-only client with no proxy and no redirects.

### Exports (`workstation-core::exports`)

Pure functions that turn a profile into text: Continue.dev `config.yaml` model entry, Zed `settings.json`
OpenAI-compatible provider entry, OpenCode `opencode.json` provider, and a `systemd --user` unit running
`llama-server` with the same argument vector. JSON is produced by `serde_json`; YAML scalars are emitted as JSON
double-quoted strings (valid YAML), so names and paths cannot break structure. For systemd every argument is quoted
and escaped for backslash, double quote, `%` (to `%%`) and `$` (to `$$`); control characters and newlines are rejected,
and `ExecStart` must be an absolute path. Tests parse the output with real parsers and, when `systemd-analyze` exists,
run `systemd-analyze --user verify` on the generated unit with hostile names and paths. Existing files are never
overwritten; the unit is never enabled or started by the application; a unit the user installs and starts is
externally owned, not Managed. Output is shown and can be copied
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
| 2a | `feat/m2-runtime-profiles` | capability contract, ownership/lifecycle types, `LaunchProfile` validation, closed flag enum and reserved-flag rules, argument builder, schema v3, store |
| 2b | `feat/m2-llamacpp-install` | release listing, backend detection, checksum, tar.gz safe extraction, install store, tests with a local server and crafted archives |
| 2c | `feat/m2-supervisor` | process supervisor, process groups, state record and orphan reclaim, ring log buffer, state machine, port pre-flight, stop/restart/recovery, tests with a fake child program |
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
