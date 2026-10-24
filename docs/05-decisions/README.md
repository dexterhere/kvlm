# Architecture decisions

Record consequential choices in numbered files such as `0002-memory-accounting.md`.
Include status, context, decision, consequences, and validation requirements. Keep
routine implementation details in code or the relevant development document.

- [0001: Workspace foundation](0001-workspace-foundation.md) — accepted.
- [0002: Hardware discovery](0002-hardware-snapshots.md) — implemented snapshots.
- [0004: Static memory fit](0004-static-memory-fit.md) — implemented core policy;
  0003 belongs to the separate Ollama PR.
- [0005: Bounded GGUF metadata inspection](0005-gguf-metadata.md) — implemented inspector.
- [0006: GGUF fit integration](0006-gguf-fit-integration.md) — explicit scenario adapter.
- [0007: Desktop CPU fit](0007-desktop-cpu-fit.md) — asynchronous Overview workflow.
- [Open questions](open-questions.md) — unresolved specification details, not decisions.

When a decision changes the master specification, explicitly update the relevant
section and explain the change. Do not silently treat illustrative snippets as
compiled contracts or claim an unresolved choice has been approved.

- [0003: Attached Ollama discovery](0003-attached-ollama-discovery.md)
