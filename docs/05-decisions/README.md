# Architecture decisions

Record consequential choices in numbered files such as `0002-memory-accounting.md`.
Include status, context, decision, consequences, and validation requirements. Keep
routine implementation details in code or the relevant development document.

- [0001: Workspace foundation](0001-workspace-foundation.md) — accepted.
- [0004: Static memory fit](0004-static-memory-fit.md) — implemented core policy;
  0002 and 0003 belong to the separate hardware and Ollama PRs.
- [Open questions](open-questions.md) — unresolved specification details, not decisions.

When a decision changes the master specification, explicitly update the relevant
section and explain the change. Do not silently treat illustrative snippets as
compiled contracts or claim an unresolved choice has been approved.
