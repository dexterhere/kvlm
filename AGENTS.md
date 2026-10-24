# Repository instructions

## Read first

- Start with `README.md` and `docs/README.md`.
- Product source of truth: `docs/01-product/gnome_local_ai_workstation_master_specification.md`.
- Read `docs/02-architecture/README.md` and `docs/05-decisions/open-questions.md`
  before implementing domain behavior. The specification is a target, not current status.
- Follow `CONTRIBUTING.md`, `SECURITY.md`, and `docs/03-development/git-workflow.md`.

## Scope and architecture

Keep four workspace crates through M4. Catalog, fit, hardware, SQLite, and exports
belong in `workstation-core`; runtime HTTP/process work in `workstation-runtimes`;
benchmark work in `workstation-bench`; GTK/GObject work in `workstation-app`.
Do not add speculative dependencies or turn planned modules into fake implementations.

GTK changes run on the GLib main thread. Use typed commands/events across Tokio
and GLib; synchronous probing and database work use `spawn_blocking`. Never block
the UI for network requests, model operations, or GPU probes.

Attached runtimes are externally owned. Never terminate them, alter system units,
or directly delete their models. Managed runtimes default to loopback. Never add
telemetry, root requirements, plaintext credentials, or untrusted model-code execution.

## Workflow

- Branch from `develop`; submit feature PRs to `develop`, then promote
  `develop → staging → main`. Never commit directly to those three branches or
  skip stages. Read `docs/03-development/git-workflow.md` before Git operations.
- Make small, frequent commits at coherent checkpoints when implementing work.
- No `Co-authored-by` trailers or generated assistant attribution. Preserve existing
  copyright and use the configured Git identity. Never invent an identity.
- Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked`, and `python3 scripts/check_repository.py`.
- Run metadata and Meson checks when changing packaging; see the development guide.
- Add meaningful tests for nontrivial behavior. Normal tests must not need a GPU,
  model downloads, external inference processes, or internet access once dependencies are cached.
- Keep docs under their numbered section and update the documentation index when needed.
- Report exactly what passed and what could not be checked. Do not declare M0
  complete without its real-hardware acceptance evidence.
