# 0003: Read-only attached Ollama discovery

Status: accepted. Date: 2026-10-24.

## Context

M0 Step 3 needs version, installed inventory, and loaded allocations from an existing
Ollama service. The full illustrative adapter trait also includes inference and
process/model mutations whose request contracts are not resolved yet.

## Decision

Implement only real discovery on `OllamaAdapter`, without speculative control methods
or a fake complete trait. Its public constructor fixes HTTP to `127.0.0.1:11434`.
Reqwest provides asynchronous HTTP with default features disabled, no TLS dependency
for this loopback-only slice, no proxies, no redirects, five-second request deadlines,
and two-MiB response limits checked while streaming. No returned URL is followed.

Require a usable `/api/version` response, then read `/api/tags` and `/api/ps`
concurrently. Each inventory can fail independently; missing arrays are errors,
while empty arrays are real empty lists. Missing optional metadata stays unknown.
The UI uses plain text, limits each section to 100 rows and each displayed field to
1024 characters, and announces truncation. Refresh failures mark retained cards stale.

Nonempty `remote_model` or `remote_host` marks Cloud Routed. Missing fields do not
prove local execution. Discovery never invokes inference, so an execution-time
local-only guard is still required before adding inference operations.

Treat `size`/`size_vram` from `/api/ps` as Ollama-reported allocations. Derive CPU
remainder only with checked subtraction; missing or inconsistent splits stay unknown.
Do not infer OS RSS, independent GPU measurements, or proven PCIe spill.

## Consequences and verification

Externally owned processes and model files remain untouched. This slice does not
provide custom endpoints, service startup, downloads, load/unload, benchmarks, or storage.
HTTP mocks cover wire errors, partial inventory, redirects, limits, and timeout phases.
Bridge tests cover cancellation; pure UI tests cover stale/retry state and formatting.
Real installed/loaded-model acceptance requires a separately running Ollama instance.

## Upstream evidence

- [Installed models](https://docs.ollama.com/api/tags) and
  [loaded models](https://docs.ollama.com/api/ps).
- [API wire types](https://github.com/ollama/ollama/blob/main/api/types.go) and
  [manifest listing](https://github.com/ollama/ollama/blob/main/server/model_list.go)
  expose optional remote metadata directly in `/api/tags`.
- [Scheduler allocation reporting](https://github.com/ollama/ollama/blob/main/server/sched.go)
  and [CLI remainder calculation](https://github.com/ollama/ollama/blob/main/cmd/cmd.go)
  support cautious allocation wording and checked subtraction.

These sources were checked on 2026-10-24; the protocol is version-sensitive.
