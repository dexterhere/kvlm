# Quick Benchmark

Date: 2026-10-25. This dependent feature carries unmerged PR #7 and its prerequisites.
No integration branch is changed and no PR is merged by this work.

## Run one explicit benchmark

1. Separately run an Ollama service with cloud features disabled. The app only reads
   its status; it never changes service configuration. See the official
   [cloud-disable guidance](https://docs.ollama.com/faq#how-do-i-disable-ollamas-cloud-features).
   The protocol was inspected in v0.40.11; versions without cloud status and local
   digest reference support are blocked rather than retried with weaker requests.
2. Refresh Runtimes to obtain installed models. No runtime request is automatic at
   app startup. This feature does not install Ollama or download models.
3. Open Benchmarks and explicitly choose an installed model. Known cloud-routed and
   duplicate-name entries are excluded; only the first 100 candidate choices appear.
   A missing digest disables Run; malformed identity or missing local-only evidence
   is rejected by preflight before generation.
4. Select Check and Run. This may load a model and allocate memory. One built-in
   text request uses context 2048, at most 128 output tokens, seed 42, temperature 0,
   and `think: false`. Model/runtime support for those requested options is not
   independently measured; incompatible models may reject the request.
5. Read client first-response-text latency and server-reported generation speed
   separately. Cancel stops waiting and closes the request; it does not unload a
   model or guarantee an immediate stop inside the externally owned server.

Selection changes and inventory refresh results cancel active runs and clear old
results. Failures, cancellation, and deadlines show partial observations as partial;
only a fully validated stream produces a completed result. An already-completed
request can win a race with cancellation. The app retains no generated output text.
Results are session-only and disappear on restart.

## Interpret the measurements

- Client first response text includes connection, queueing, model loading, and any
  thinking before response text; it excludes preflight. Chunks can contain multiple
  tokens, so this is not token-level TTFT or ITL.
- Generation speed is `eval_count * 1e9 / eval_duration_ns` from the server. Missing
  or zero counters leave it Unknown; it is not derived from response chunk counts.
- Load duration is separately server-reported. Existing load/cache state is uncontrolled;
  this run is neither a cold/warm comparison nor a repeated calibrated benchmark.
- Runtime manifest digests identify this request; they do not verify a local GGUF
  file or hardware identity. No Benchmark Verified badge or positive fit follows.
- There is no SQLite storage, resource sampling, peak RAM/VRAM, or restart verification.

The execution-time guard trusts the attached loopback server's implementation and
reported cloud-disable state. It is not a network sandbox. Every run uses a fresh
inventory check and an immutable `sha256:<digest>:local` request with no tag fallback.
See [decision 0009](../05-decisions/0009-quick-benchmark.md) for the detailed contract.

## Validation

Ordinary tests need no installed runtime, model download, GPU, or Internet service.
HTTP tests use ephemeral loopback ports; the development sandbox requires permission
to bind them. The product itself remains rootless.

- Required formatting, workspace Clippy with warnings denied, and repository validation passed.
- `cargo test --workspace --locked`: 112 passed; three display tests ignored normally.
- Nine Python helper tests passed.
- All three GTK flow tests passed separately on an isolated Broadway display with
  GLib criticals fatal. The new test drives selection, full/closed queues, cancellation,
  completion, stale replies, recovery, and a populated long-name selector at 360px.
- Independent review and root review covered the guard, transport bounds, timing,
  concurrency, cancellation, and UI state. No unresolved findings remained.

The initial GTK check caught an invalid ComboRow parent; a PreferencesGroup fixed
it before the passing run. Portal backends emitted display-availability warnings
in the isolated session. This is automated widget evidence, not native GNOME,
keyboard, screen-reader, real-model, or resource-calibration acceptance.

Run the optional flow check with a working GTK display:

```sh
cargo test -p workstation-app --locked \
  views::bench_lab::smoke::quick_benchmark_selection_cancel_and_completion_flow -- --ignored --exact
```

HTTP/parser tests cover status/identity/capability failures without generation, immutable
request fields, malformed/truncated/trailing/remote/error records, bytewise UTF-8,
CRLF, absent final newline, byte/record limits, content type, and header/idle timeouts.
Runner tests separate server metrics from client latency and cover partial failure,
cancellation, sender loss, and deadlines. Bridge tests cover concurrent commands,
duplicate rejection, panic recovery, and shutdown dropping active requests.

No real inference was run during implementation. SQLite persistence, real runtime
acceptance, native GNOME validation, and the full M0 definition of done remain open.
