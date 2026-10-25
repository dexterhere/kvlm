# 0009: Guarded single-request Ollama benchmark

Status: accepted. Date: 2026-10-25.

## Context

Model inspection cannot establish local execution from missing remote fields. A
benchmark sends a prompt and may load a model, so it needs an execution-time guard,
bounded streaming, explicit cancellation, and honest measurements before storage.

## Decision

Keep four crates. Runtime HTTP, local-only checks, framing, and arrival timestamps
belong in `workstation-runtimes`. The fixed workload, outcomes, cancellation/deadline,
and derived rate belong in `workstation-bench`. GTK selection stays in the app.

Immediately before each run:

1. Require a full SHA-256 manifest digest and reject known cloud-routed selections.
2. Require `GET /api/status` to report `cloud.disabled: true`. Missing, malformed,
   unavailable, or false status blocks generation. Never change or restart Ollama.
3. Read version and installed inventory afresh. Require one exact selected name
   with the same canonical digest and no positive remote metadata.
4. Inspect `sha256:<full-hex>:local` through `/api/show`. Require explicit completion
   capability and no positive remote metadata.
5. Generate using that same immutable local digest reference. Never retry with a
   mutable tag or drop the local selector if an older server rejects the request.

This trusts the attached loopback server's protocol implementation. It is not OS
network isolation or independent hashing of its files. The digest refers to an
Ollama manifest, not the GGUF selected in Overview. The local source selector also
guards against tag replacement between inventory checks and execution.

The built-in `quick-text-v1` workload requests 128 maximum output tokens, context
2048, seed 42, temperature 0, streaming, and no thinking. There is no editable
prompt, warm-up, repeated pass, forced unload, or `keep_alive` override. Inference
may load/retain the model and allocate memory under the external server's policy.
Record requested settings without claiming they are independently observed values.

Streaming requires NDJSON, exact response model identity, no remote routing markers,
a recognized terminal reason (`stop` or `length`), response text, and a clean EOF.
Bound each record to 64 KiB, the body to 4 MiB, and nonblank records to 8192.
Handle arbitrary transport/UTF-8 boundaries, CRLF, and a final record without newline.
Malformed/truncated streams, runtime error records, trailing data, unknown completion
states, and invalid metric types fail without exposing raw response bodies.

Client time to first nonempty response text starts immediately before generation
dispatch; it excludes preflight and includes connection, queueing, model load, and
any thinking before response text. It is not token-level TTFT or ITL. Count response
records, not tokens. Derive server generation rate only from positive `eval_count`
and `eval_duration` in nanoseconds. Missing/zero counters leave the rate Unknown.
Keep server load duration separate; do not label the run cold or warm.

A 180-second runner deadline includes preflight. Generation also has a 180-second
HTTP deadline and 30-second header/idle waits; metadata keeps the existing five-second
deadlines and two-MiB bounds. Watch-channel cancellation works out of band even when
the command queue is full. Cancellation/timeout drops the generation future and
HTTP connection; it does not signal, unload, or guarantee an immediate halt of the
external server. Partial observations never become a completed benchmark result.

The bridge permits one benchmark task alongside other commands, reports panics,
rejects duplicate runs, and aborts its task set on shutdown. Selection/inventory
changes cancel and invalidate old UI results. No persistence, resource sampling,
artifact/hardware verification, positive fit, or Benchmark Verified badge is added.

## Verification and upstream evidence

Offline HTTP/parser tests, runner tests, bridge tests, and an opt-in GTK flow test
cover the boundaries above. See the [workflow guide](../03-development/quick-benchmark.md)
for actual evidence and limits. Real inference and full M0 acceptance remain open.

Verified against released [Ollama v0.40.11](https://github.com/ollama/ollama/releases/tag/v0.40.11),
commit `675150d6ad2747df1e218ef59de9194bc4a7e538`, on 2026-10-25:

- [Source selectors](https://github.com/ollama/ollama/blob/675150d6ad2747df1e218ef59de9194bc4a7e538/internal/modelref/modelref.go),
  [digest model resolution](https://github.com/ollama/ollama/blob/675150d6ad2747df1e218ef59de9194bc4a7e538/server/model_resolver.go),
  and [full digest references](https://github.com/ollama/ollama/blob/675150d6ad2747df1e218ef59de9194bc4a7e538/manifest/paths.go).
- [Status/show/generate handlers](https://github.com/ollama/ollama/blob/675150d6ad2747df1e218ef59de9194bc4a7e538/server/routes.go)
  enforce source intent and echo the request model in generated records.
- [Cloud-disable guidance](https://docs.ollama.com/faq#how-do-i-disable-ollamas-cloud-features),
  [generation fields](https://docs.ollama.com/api/generate), and
  [NDJSON streaming](https://docs.ollama.com/api/streaming).

Older versions are not assumed compatible. Unsupported status or digest selectors
block execution; source inspection does not substitute for real-service acceptance.
