# 0017: Managed llama.cpp runtime

Date: 2026-10-27. Status: M2 implemented.

## Decision

The application can own exactly one `llama-server` ("Managed") next to externally owned runtimes ("Attached", currently
Ollama). Ownership decides what the application may do: it starts, stops and restarts only what it spawned; an Attached
runtime is read-only and is never signalled, reconfigured or deleted from.

- **Install.** The official portable Linux build is downloaded from the `ggml-org/llama.cpp` release assets, checked
  against the published SHA-256 digest, extracted by an application-owned reader with size, entry and path limits, and
  published atomically under the application data directory. No root, no package manager. Nothing is fetched until the
  user chooses a build.
- **Launch profile.** Typed fields only; the host is always `127.0.0.1`; there is no free-text argument field. The
  supervisor owns the flags that identify and observe the process (`--host`, `--port`, `--alias`, `--metrics`).
- **Identity before Ready.** Every launch gets a random alias; `Ready` requires the health endpoint and the model list to
  show that alias, so a different listener on the port can never look ready.
- **Process safety.** Own process group, cleared environment with a short allow-list, SIGTERM then SIGKILL to the group,
  the leader reaped last, a private record file so a crashed run's child is reclaimed only when proven ours, and one
  supervisor per data directory.
- **Quit.** The application stops its managed server on quit and on SIGINT, SIGTERM and SIGHUP.
- **Exports.** Continue.dev, Zed, OpenCode and systemd `--user` texts are generated from a validated profile and are never
  written, enabled or started by the application. A unit the user starts is externally owned.

## Consequences

Limits are explicit: one managed server, a bounded log ring, at most three automatic restarts in ten minutes. Multiple
instances of the application cannot supervise at once. Details: [installer](../03-development/llamacpp-install.md),
[supervisor](../03-development/runtime-supervisor.md), [client](../03-development/llamacpp-client.md),
[exports](../03-development/developer-exports.md), [interface](../03-development/managed-runtime-ui.md),
[evidence](../03-development/m2-acceptance.md).
