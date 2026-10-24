# Changelog

Changes worth knowing about are recorded here. There is no published release yet.

## Unreleased

### Added

- Read-only CPU topology/instruction flags, available RAM, NVIDIA NVML, and AMD DRM memory probes.
- Overview hardware cards with asynchronous startup detection and manual/keyboard Refresh.
- Explicit partial-probe notices, stale snapshot retention after refresh failure, and refresh coalescing.
- Fixture-backed hardware tests and an opt-in local snapshot example.
- Protected `develop → staging → main` workflow with required branch-policy CI,
  local commit checks, and merge-commit promotion.
- Rust 2024 workspace with four crates and the specified module layout.
- Minimal GNOME application shell with four views and a Tokio/GLib event bridge.
- GNOME desktop metadata, settings schema, and development icon.
- Cargo and Meson build foundations, CI checks, and repository validation.
- Community policies, contribution templates, GPL-3.0-or-later licensing, and Git conventions.

### Changed

- Moved the original master specification into `docs/01-product/` without altering its contents.

Runtime adapters, model catalog/downloads, fit calculations, SQLite persistence,
configuration exports, and benchmark execution remain planned.
