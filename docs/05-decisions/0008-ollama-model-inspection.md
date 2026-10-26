# 0008: Read-only installed Ollama model inspection

Status: accepted. Date: 2026-10-25.

## Context

Discovery lists installed and loaded models, while Overview estimates an explicitly
configured local GGUF scenario. Selecting a runtime model needs better visibility
without treating model defaults as actual runtime configuration or proving support.

## Decision

An explicit Inspect action sends JSON `POST /api/show` with the selected model name
and `verbose: false`, alongside a fresh `GET /api/ps`. This POST only reads metadata;
it does not invoke generation, loading, downloading, or other mutations. Reuse the
fixed IPv4 loopback client, disabled proxies/redirects, five-second deadlines, and
two-MiB response limits. Bound names to 1024 bytes and reject control characters.
JSON encoding preserves names without interpolating them into paths or shell commands.
Never expose raw error bodies, Modelfiles, templates, or system prompts in the UI.

Keep failures from the two endpoints independent. Display architecture, declared
training context, capabilities, quantization, and declared parameter text separately
from loaded context and allocation. Do not parse declared defaults into actual
K/V types, micro-batch size, concurrency, or layout. Missing data stays unknown.
No positive fit or benchmark classification follows from inspection.

Associate selected-model loaded readings only when name and nonempty digest uniquely
match the installed snapshot and fresh loaded list. This is an association of runtime
reports, not cryptographic verification of local GGUF bytes. Do not match the Overview
file by name, import anything, or fill scenario inputs automatically. These API reads
are separate observations and cannot establish an atomic artifact snapshot.

Nonempty remote metadata from either installed metadata or inspection means Cloud
Routed. Missing remote metadata never proves local execution; an execution-time,
fail-closed guard remains necessary before adding benchmarks. Do not follow remote URLs.

GTK owns selection state and a request generation. Refresh invalidates selection and
stale replies; one inspection remains in flight at a time. Bounded queues report
busy/unavailable states and allow retry. Runtime events must not alter hardware/fit
loading state. All widget updates remain on GLib; HTTP remains on Tokio.

## Consequences and verification

The combined desktop carries the still-unmerged prerequisite PRs and targets develop.
No process control, inference, new dependency, persistence, or verified runtime fit
is added. Mock HTTP tests cover request encoding, partial failures, invalid metadata,
limits, redirects, and timeouts. Bridge and view tests cover correlation, shutdown,
selection invalidation, matching, and retry. Native GNOME and real Ollama acceptance
remain separate from mock evidence.

## Upstream evidence

Checked on 2026-10-25 against the official
[show-model API](https://docs.ollama.com/api-reference/show-model-details),
[loaded-model API](https://docs.ollama.com/api/ps), and
[installed-model API](https://docs.ollama.com/api/tags). Model metadata and runtime
allocation semantics are version-sensitive; absent fields are not inferred.
