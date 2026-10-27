# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`AGENTS.md` holds the binding repo rules (scope, safety, Git workflow); read it first. This file adds commands and architecture only.

## Project

GNOME Local AI Workstation: a GTK4/Libadwaita desktop control plane that explains local-model memory fit, attaches to inference runtimes (currently read-only Ollama), and runs quick benchmarks. It is not a chatbot or inference engine. The master specification (`docs/01-product/`) describes the *target*, not current status — verify against code and `README.md` before assuming a feature exists. Planned modules are documentation-only; do not add fake/stub implementations.

## Commands

```sh
cargo run --locked -p workstation-app                       # run the GUI (needs a graphical session)
cargo run --locked -p workstation-core --example memory_fit # synthetic fit calculator demo
cargo run --locked -p workstation-core --example probe_hardware  # live probe, prints device IDs (redact before sharing)

# Required checks before finishing work
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/check_repository.py                         # Markdown link targets + shared app identity/license metadata
python3 -m unittest discover -s tests -p 'test_*.py'        # Python helper tests

# Single test
cargo test --locked -p workstation-core --test gguf_fit_tests <name>
cargo test --locked -p workstation-runtimes <name>

# Packaging changes only
glib-compile-schemas --strict --dry-run data
appstreamcli validate --no-net data/io.github.dexterhere.Kvlm.metainfo.xml.in
meson setup build --prefix="$HOME/.local" && meson compile -C build && meson test -C build --print-errorlogs
```

Tests must not need a GPU, model downloads, a running inference process, or internet. Use `CARGO_BUILD_JOBS=1` on memory-constrained machines. Enable the commit-message hook with `git config core.hooksPath .githooks`.

## Architecture

Four workspace crates (keep to four through M4); workspace forbids `unsafe_code`.

- `workstation-core`: everything pure/domain — hardware probes (`hardware/`: CPU, NVML loaded dynamically, AMD DRM sysfs, `resources.rs` RAM/VRAM/GTT readings), fit math (`fit/`), GGUF parsing (`models/gguf.rs`), SQLite (`db/`), exports. Integration tests live in `crates/workstation-core/tests/`, not at the repo root (root manifest is virtual).
- `workstation-runtimes`: HTTP/process adapters. Only `ollama` exists; it is an *attached*, externally owned runtime — never terminate it, alter its units, or delete its models.
- `workstation-bench`: `runner.rs` (execution + cancellation), `sampler.rs` (background RAM/VRAM/GTT peak sampling), `history.rs` (persists run snapshots/outcomes via core's DB), `workloads.rs`.
- `workstation-app`: GTK shell. `bridge.rs` defines the typed `CoreCommand` (UI to Tokio) / `UiEvent` (Tokio to UI) enums and the worker; `views/` has one file per page (overview, model_fit, runtimes, bench_lab, benchmark_history, catalog placeholder) plus `*_smoke.rs` GTK smoke tests; `fit_workflow.rs`/`fit_state.rs` drive the CPU fit scenario.

Threading rule: GTK objects stay on the GLib main thread. Cross to Tokio only via `CoreCommand`/`UiEvent` (ids like `id: u64` correlate request and result so stale responses can be dropped). Synchronous probes and DB work go through `spawn_blocking`. Never block the UI on network, model, or GPU calls.

Design invariants that recur across modules: missing metadata yields `Unknown`, never an optimistic fit; shared GPU memory is never added to host RAM; declared model defaults stay separate from loaded settings; unmeasured memory channels are reported as unknown with coverage limits rather than zero; benchmark runs happen only after explicit user Check + Run with local-only preflight at execution time. No telemetry, root requirements, or plaintext credentials.

## Git

Flow is `feature branch from develop` -> `develop` -> `staging` -> `main`; never commit directly to those three or skip a stage. Make small commits at coherent checkpoints. Per `AGENTS.md`: no `Co-authored-by` trailers or assistant attribution in commits/PRs. CI `git-policy` rejects wrong PR routes and co-author trailers. Details: `docs/03-development/git-workflow.md`.

## Docs

Docs are numbered by section under `docs/` (01-product, 02-architecture, 03-development, 04-roadmap, 05-decisions); read `docs/02-architecture/README.md` and `docs/05-decisions/open-questions.md` before implementing domain behavior, and update `docs/README.md` when adding a doc. Feature guides in `docs/03-development/` (e.g. `quick-benchmark.md`, `benchmark-memory-sampling.md`) define scope and limits for each shipped capability.
