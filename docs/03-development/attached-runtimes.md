# Attached runtimes: LM Studio and vLLM

LM Studio and vLLM are runtimes the user runs themselves ("Attached"). The application can look at what they serve; it
cannot start, stop, load, unload or configure them, and the capability table says so
(`workstation-runtimes/src/traits.rs`).

## What a check does

The Runtimes page has an "Other runtimes (attached)" section with a card per runtime: a port (defaults 1234 and 8000), one
Check button and a read-only result. Nothing is requested until Check is pressed, one check runs at a time per card, and
nothing polls. A check is a burst of GET requests to `127.0.0.1:<port>`:

| Runtime | Requests | Result |
| --- | --- | --- |
| LM Studio | `/api/v0/models`; if that is 404, `/v1/models` | Model ids with load state, quantization and context lengths. The fallback lists names only and says load state is unknown |
| vLLM | `/health`, `/v1/models`, `/metrics` (concurrently) | Health, served models with their longest sequence, running and waiting requests, KV cache use |

Each part of a vLLM check succeeds or fails on its own; a failed part says why and is never shown as zero. A name the
server did not report reads "not reported".

## Safety

- The client can only be built for IPv4 loopback, ignores proxy settings, follows no redirect (a 302 is an error),
  sends no credentials and no referrer, and has a 5 s deadline.
- Bodies are capped (2 MiB for JSON, 512 KiB and 20,000 lines for metrics) while they are read; a declared or streamed
  oversize body is refused.
- Errors are typed and carry no response text. A 401 or 403 reads "asks for an API key"; keys are not stored for these
  runtimes.
- Only known JSON fields and known metric names are read. Model ids and labels lose control characters, Unicode line
  separators and bidirectional controls and are cut to 200 characters; lists are capped (1000 for LM Studio, 100 for
  vLLM) and reported as truncated; a card shows the first 20 models and counts the rest, so status lines always fit.
- A metric series that differs only by label is summed (counters, request gauges) or its highest value is used (cache
  use). Non-finite and negative values are ignored, as is a cache fraction outside 0 to 1 and a sum that overflows. Label values may
  contain spaces, braces and escaped quotes. A metrics body with more than 20,000 lines is refused instead of read partly,
  because a partial sum would pass for a total.

## What is not done

- **No benchmark or inference** goes through these adapters. Streaming inference is not in their capability table: a
  loopback port does not prove local execution (the runtime may route a request elsewhere, and an unauthenticated port
  trusts every local user), so a benchmark would first need an explicit "local execution cannot be proven" confirmation
  in the interface. That flow is not built yet.
- Changing the port discards any pending answer for the old port, and closing the window stops checks in flight.
- **No periodic metrics scraping.** A user presses Check again to refresh; this keeps "never in the background" true.
- Neither runtime was available on the reference machine, so the adapters are covered by tests against local mock
  servers shaped like the documented responses, not by a live run. Response shapes can differ between releases; unknown
  fields are ignored and missing ones stay unknown.
