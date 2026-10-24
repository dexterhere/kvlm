# Changelog

Changes worth knowing about are recorded here. There is no published release yet.

## Unreleased

### Added

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

Hardware probes, runtime adapters, model catalog/downloads, desktop fit integration,
SQLite persistence, configuration exports, and benchmark execution remain planned.
