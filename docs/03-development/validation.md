# Foundation validation

Recorded on 2026-10-23 for the initial project setup. This is evidence for the
development foundation, not acceptance of the complete M0 inference workflow.

## Environment

Linux x86_64, Rust/Cargo 1.97.1, GTK 4.24.1, Libadwaita 1.10.0, GLib 2.90.1,
Python 3.13, Meson 1.12.1, and Ninja 1.13.2. Meson/Ninja were installed into a
temporary isolated environment for these checks, not added as Rust dependencies
or installed as system packages.

## Passed

- `cargo check --workspace --locked --offline`.
- `cargo build --locked --offline --package workstation-app`.
- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- `cargo test --workspace --locked --offline`: two worker-channel tests passed.
- `python3 scripts/check_repository.py`: local Markdown target paths and shared
  application identity/license metadata are consistent. Heading anchors are not checked.
- Python unittest discovery: four checks cover relative/encoded Markdown links,
  debug/release build artifact copying, and preserving an existing output on build failure.
- Meson native debug build and all five Meson checks: repository, Python helpers,
  desktop entry, AppStream metadata, and GSettings schema.
- A `DESTDIR` installation staged the executable, launcher, AppStream metadata,
  settings schema, icon, and license texts without installing into the host prefix.
- The master specification is byte-for-byte identical to the initial Git checkpoint
  after its move to `docs/01-product/`.
- Commit history contains no co-author trailers.

## GUI smoke test

Started the real application using GTK's Broadway backend on a temporary,
loopback-only test display and a separate D-Bus session. Verified all four pages,
worker-ready status, the narrow layout with bottom navigation, and `Ctrl+1`
navigation. `Ctrl+Q` shut the application down with exit status 0. The test tab and
temporary display server were closed afterward.

The isolated display emitted a GDK duplicate-frame-timing warning while resizing,
and its session services emitted portal/permission notices. The application still
rendered, navigated, and exited successfully. This is not evidence that every
native Wayland/X11 desktop or accessibility configuration works without warnings.

## Remaining validation

- The declared Rust 1.92 minimum and GTK 4.12/Libadwaita 1.4 minimum were not
  exercised directly; the foundation was built using the versions above.
- Hosted CI has not run: no public remote was configured during setup.
- Optimized release builds, distro packages, Flatpak, translations, and full
  accessibility review remain outside this foundation validation.
- Hardware probing, inference adapters, model downloads, memory-fit math, SQLite
  storage, and benchmarking remain unimplemented and therefore untested.
- AppStream's missing homepage is explicitly informational until public hosting
  exists. Remove the targeted override after adding the real URL. Developer ID
  metadata is also pending the publication/namespace decision.
