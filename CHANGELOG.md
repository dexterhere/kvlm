# Changelog

Changes worth knowing about are recorded here. The first pre-release is 0.1.1.

## Unreleased

### Changed

- Plainer, calmer interface: the first page is "My computer" with a short summary and a starting
  model size, and the memory calculator sits in a collapsed section. The llama.cpp card explains
  what llama.cpp is and what Kvlm does and does not control.
- The setup guide shows one card at a time (no sliding neighbours), with a colour-scheme choice, a plain explanation of
  models and runtimes, and Hugging Face token steps.

- Clearer page names: My computer, Models ("Find models"), Runtimes, Speed test and Experiments, each
  with a one-line explanation, and plainer status text.

- Less on screen at once: Runtimes shows one runtime at a time (Ollama, llama.cpp, LM Studio and
  vLLM), and Speed test and Experiments keep options, verification and past results in collapsed
  sections.

### Added

- A release workflow that builds a Flatpak bundle and an AppImage and attaches them, with checksums, to
  a version tag's release. Both built on a pull request; neither has been installed and run on a real machine.
- Preferences (Ctrl+,): colour scheme (follow the system, light or dark), reopen the setup guide,
  and the fixed privacy statements.
- "How to get a token" steps with a button that opens the Hugging Face token page, on the Models
  page and in the setup guide.
- The setup guide shows the hardware snapshot the window already takes and a rough starting model
  size. It makes no extra probe or request.

## 0.1.2 - 2026-10-27 (beta pre-release)

### Changed

- The application ID is now `io.github.dexterhere.Kvlm` (ADR 0019). A Hugging Face token saved
  under the old placeholder ID must be saved again.
- Security and conduct contacts are published; GitHub private vulnerability reporting is enabled.
- The README explains the name Kvlm (KV cache + language model).

### Added

- `scripts/install.sh`: user-level build and install with tool checks and uninstall; never uses sudo.
- First-run setup guide, reopenable from the main menu.
- `scripts/compare_runtimes.py` and the first measured runtime numbers on the reference machine
  ([runtime benchmarks](docs/03-development/runtime-benchmarks.md),
  [test coverage](docs/03-development/test-coverage.md)).

## 0.1.1 - 2026-10-27 (beta pre-release)

First tagged pre-release, built from source. M4 is implemented with validation pending; see
[beta status](docs/03-development/beta-status.md).

### Changed

- README rewritten around the Kvlm name, with architecture, threading, ownership and branch
  diagrams, and interface wireframes in `docs/02-architecture/ui-wireframes.md`.

### Added

- Opt-in update check (one request when asked), AppStream release and description updates, and a
  release checklist.
- Ctrl+K quick search and an opt-in accessibility audit of every page.
- Read-only LM Studio and vLLM discovery on the Runtimes page.
- Qwen2 architecture metadata and conditional CPU/CUDA memory-policy classes with
  fresh budgets, four-part breakdowns, and explicit layout assumptions.
- Exact Ollama manifest/hardware benchmark verification restored after restart,
  versioned hardware fingerprints, and real Qwen GTK/CUDA acceptance checks.
- Immutable SQLite benchmark history and sampled system/device memory peaks.
- Explicit single-model Quick Benchmark with affirmative local-only checks, immutable
  manifest digest requests, bounded streaming, independent client/server metrics,
  cancellation, and partial failure outcomes.

- Installed Ollama model selection, read-only metadata inspection, and fresh loaded
  context/allocation with explicit unknowns and stale-reply protection.
- Attached Ollama discovery.

- Local GGUF CPU scenario estimates in Overview, with editable settings, fresh RAM,
  asynchronous inspection, explicit assumptions, and stale-result protection.
- Read-only Linux CPU, RAM, NVIDIA and AMD hardware discovery in Overview.
- GGUF-to-calculator scenario adapter and CLI, with explicit layout assumptions,
  sequence and metadata gates, and compatibility kept unverified.
- Bounded, read-only local GGUF v3 metadata inspection, tensor-range validation,
  adversarial fixtures, and a CLI example; no runtime compatibility or fit claim.
- Supplied-metadata static memory-fit calculator with checked byte arithmetic,
  separate K/V formats, CPU/GPU/shared placement limits, explicit unknown/error
  states, an offline reference example, and independent integration tests.
- Rust 2024 workspace with four crates and the specified module layout.
- Minimal GNOME application shell with four views and a Tokio/GLib event bridge.
- GNOME desktop metadata, settings schema, and development icon.
- Cargo and Meson build foundations, CI checks, and repository validation.
- Community policies, contribution templates, GPL-3.0-or-later licensing, and Git conventions.

### Changed

- Moved the original master specification into `docs/01-product/` without altering its contents.

Verified runtime settings, model catalog/downloads, configuration exports, managed
runtime control, process attribution, and controlled benchmark comparisons remain planned.
M0 acceptance for the measured GTK/CUDA baseline is recorded in [the reference record](docs/03-development/m0-acceptance.md).
