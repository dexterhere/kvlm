# 0001: Workspace foundation

Status: accepted. Date: 2026-10-23.

## Context

The repository began with a single product specification. The requested work is
the complete directory and contribution setup, not implementation of every milestone.
The project is intended as an open-source contribution for the community.

## Decision

- Preserve the original specification under `docs/01-product/` and order documentation
  by product, architecture, development, roadmap, and decisions.
- Use the four crates specified for M0–M4, with Rust edition 2024 and shared dependency versions.
- Provide a minimal real GNOME shell and typed worker bridge. Keep unimplemented
  modules documentation-only instead of exposing fake or panicking APIs.
- Keep normal tests free of GPU, inference-runtime, and model-download requirements.
- Use Cargo for development and Meson for native installation integration.
- License the project GPL-3.0-or-later, as explicitly chosen by the project owner.
- Accept community contributions under the same license with no CLA or assignment.
- Make small, meaningful commits without co-author trailers or assistant attribution.

## Consequences and verification

The scaffold is useful for development but not an operational inference manager.
Domain dependencies are added when their implementations need them. The specification's
extra catalog/database boxes are conceptual components, not additional crates.
Tests must be registered under real Cargo packages; a virtual workspace does not
execute standalone root test files.

Check workspace compilation, formatting, Clippy, tests, local documentation links,
desktop metadata, and a staged native installation. Live GPU and runtime validation
belongs to M0 and later, not this setup milestone.
