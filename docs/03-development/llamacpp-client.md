# llama.cpp HTTP client

Status: implemented (M2 phase 2d), not yet wired into the interface (phase 2f).

`workstation-runtimes::llamacpp::client::LlamaClient` talks to the one managed `llama-server`. It is built from a
port only: the host is fixed to `127.0.0.1`, proxies are disabled and redirects are never followed, so a profile
cannot point it at another machine.

## Calls

- `identity(alias)` returns `Ready`, `Loading` or `Down`. `Ready` needs `/health` = 200 and `/v1/models` listing the
  launch alias; the supervisor uses this for readiness.
- `props()`, `slots()` and `metrics()` read loaded-model facts. Metrics parsing keeps known series only; anything
  missing stays unknown.
- `busy()` is `None` when the slots endpoint is unavailable, never a guess.
- `stream_chat(request, deadline, cancel, on_event)` posts a streaming chat completion and parses server-sent events.

## Streaming limits

Single line 64 KiB, total body 8 MiB, 100,000 events, an overall deadline, and cancellation checked every 100 ms.
Events are `Content` or `Reasoning`. The result reports event counts, time to first content, finish reason, usage and
the server's own timings. Network chunks are never counted as tokens.

## Tests

Unit tests use a std-only loopback mock. The end-to-end test streams a completion from the fake `llama-server` under
the real supervisor. No model, GPU or internet is needed.
