# Getting started

## Prerequisites

- Linux, Rust stable (edition 2024; Rust 1.92+), Cargo, rustfmt, and Clippy.
- GTK 4.12+ and Libadwaita 1.4+ headers and libraries.
- C compiler, linker, and pkg-config.
- Python 3.11+ for repository checks.
- Meson 1.3+ and Ninja for desktop installation (Cargo development does not need Meson).
- `glib-compile-schemas`, `desktop-file-validate`, and `appstreamcli` for metadata checks.

Examples of native development packages; names and available versions depend on the distro:

```sh
# Debian/Ubuntu
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  meson ninja-build libglib2.0-bin desktop-file-utils appstream python3

# Fedora
sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel \
  meson ninja-build glib2 desktop-file-utils appstream python3

# Arch Linux
sudo pacman -S --needed base-devel pkgconf gtk4 libadwaita meson ninja \
  desktop-file-utils appstream python
```

Installing OS packages may require administrator privileges; running the application
does not. Install Rust using your distribution or the official
[Rust installation instructions](https://www.rust-lang.org/tools/install).
The checked-in toolchain file requests stable with rustfmt and Clippy. No inference
runtime or GPU SDK is needed for this shell. NVIDIA libraries must eventually be
loaded dynamically, not made a compile-time requirement.

## Build and run

From the repository root:

```sh
pkg-config --modversion gtk4 libadwaita-1
cargo build --workspace --locked
cargo run --locked -p workstation-app
```

Run from a graphical session. A headless build can run unit tests without showing
a window. Overview shows real hardware snapshots with Refresh and `Ctrl+R`. The
local [CPU scenario panel](desktop-cpu-fit.md) inspects user-selected GGUF metadata
and estimates explicitly configured inputs on a background worker. The
Runtimes page provides manual Refresh for attached Ollama discovery and Inspect
for per-model declarations and fresh loaded readings. See the
[inspection guide](ollama-inspection.md). Models and Benchmarks remain placeholders;
no inference results are fabricated. Source-tree execution
does not require installed desktop metadata or a compiled settings schema.

## Check changes

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/check_repository.py
python3 -m unittest discover -s tests -p 'test_*.py'
glib-compile-schemas --strict --dry-run data
appstreamcli validate --no-net data/org.gnome.LocalAIWorkstation.metainfo.xml.in
```

`Cargo.lock` is committed because this workspace ships an application. Use `--locked`
for normal builds and CI; update dependencies intentionally and commit lockfile changes.
For offline builds, first cache dependencies with `cargo fetch --locked`, then add
`--offline`. CI does not download models or connect to an inference runtime.

Validate the generated `.desktop` file with `meson test` after Meson setup below;
`desktop-file-validate` rejects the source template's `.in` extension. AppStream
uses the verified GitHub repository URL and developer namespace; no severity
override is needed.

## Build and install with Meson

```sh
meson setup build --prefix="$HOME/.local"
meson compile -C build
meson test -C build --print-errorlogs
meson install -C build
```

Meson calls Cargo with the committed lockfile, installs the executable, launcher,
icon, AppStream metadata, settings schema, and license texts. Run the installed binary as
`~/.local/bin/local-ai-workstation`. For a staging installation:

```sh
DESTDIR="$PWD/stage" meson install -C build
```

Meson builds Cargo's development profile by default; use `--buildtype=release`
with `meson setup` for a release build. Its Cargo target directory is
inside the build tree so it does not interfere with regular Cargo builds. This is
a native build foundation, not completed `.deb`, `.rpm`, AppImage, or Flatpak packaging.

## Common problems

For an explicit local hardware check, run:

```sh
cargo run --locked -p workstation-core --example probe_hardware
```

This opt-in example prints a JSON snapshot, including device identifiers; review
and redact it before sharing. Normal tests use sanitized fixtures and never run
this live probe. NVIDIA requires a working runtime `libnvidia-ml.so.1`, not CUDA
development headers. AMD reads kernel DRM sysfs. Missing libraries/nodes are
reported as probe notices. Intel/Vulkan memory probing remains future work.

- Missing `gtk4.pc` or `libadwaita-1.pc`: install development packages and inspect `pkg-config`.
- Display errors: launch from a graphical session; compilation and unit tests are headless.
- First build cannot download crates: check network access or prepopulate Cargo's cache.
- Installed settings are not found: ensure the installation prefix is in the session's
  XDG data search path. The scaffold currently does not read settings.
- Memory-constrained Rust builds: use `CARGO_BUILD_JOBS=1` to reduce parallel compilation.
