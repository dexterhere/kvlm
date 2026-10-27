#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build Kvlm from this checkout and install it for the current user. Never uses sudo.
#
#   scripts/install.sh              check tools, build, install to ~/.local
#   scripts/install.sh --check      only report what is missing
#   scripts/install.sh --uninstall  remove what a previous install put under the prefix
#   scripts/install.sh --prefix DIR install somewhere else (must be writable by you)
#   scripts/install.sh --yes        do not ask before building
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
prefix="$HOME/.local"
mode=install
assume_yes=no
app_id=io.github.dexterhere.Kvlm

while [ $# -gt 0 ]; do
  case $1 in
    --check) mode=check ;;
    --uninstall) mode=uninstall ;;
    --yes|-y) assume_yes=yes ;;
    --prefix) prefix=${2:?--prefix needs a directory}; shift ;;
    -h|--help) sed -n '3,9p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '%s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

if [ "$mode" = uninstall ]; then
  say "Uninstalling from $prefix"
  rm -f "$prefix/bin/local-ai-workstation" \
        "$prefix/share/applications/$app_id.desktop" \
        "$prefix/share/metainfo/$app_id.metainfo.xml" \
        "$prefix/share/glib-2.0/schemas/$app_id.gschema.xml" \
        "$prefix/share/icons/hicolor/scalable/apps/$app_id.svg"
  rm -rf "$prefix/share/licenses/local-ai-workstation"
  have glib-compile-schemas && glib-compile-schemas "$prefix/share/glib-2.0/schemas" 2>/dev/null || true
  have update-desktop-database && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
  say "Removed the program, desktop entry, schema and icon."
  say "Your data is untouched: ~/.local/share/local-ai-workstation (history), the keyring entry and any exported unit files."
  exit 0
fi

# Detect the package manager only to print the right command; this script never runs it.
if have apt-get; then
  hint='sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev meson ninja-build libglib2.0-bin desktop-file-utils python3'
elif have dnf; then
  hint='sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel meson ninja-build glib2 desktop-file-utils python3'
elif have pacman; then
  hint='sudo pacman -S --needed base-devel pkgconf gtk4 libadwaita meson ninja desktop-file-utils python'
else
  hint='install a C compiler, pkg-config, GTK 4.12+, libadwaita 1.4+, meson, ninja and Python 3.11+'
fi

missing=()
for tool in cargo cc pkg-config meson ninja python3 glib-compile-schemas; do
  have "$tool" || missing+=("$tool")
done
if have pkg-config; then
  pkg-config --atleast-version=4.12 gtk4 2>/dev/null || missing+=("gtk4>=4.12")
  pkg-config --atleast-version=1.4 libadwaita-1 2>/dev/null || missing+=("libadwaita>=1.4")
fi
if have python3 && ! python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
  missing+=("python3>=3.11")
fi

if [ ${#missing[@]} -gt 0 ]; then
  say "Missing: ${missing[*]}"
  say "Install them with your package manager, for example:"
  say "  $hint"
  have cargo || say "Rust: https://www.rust-lang.org/tools/install"
  exit 1
fi
say "All build tools found."
[ "$mode" = check ] && exit 0

mkdir -p "$prefix" 2>/dev/null || true
if [ ! -w "$prefix" ]; then
  say "$prefix is not writable by you. Choose another with --prefix, or use the default under your home." >&2
  exit 1
fi

say "Kvlm will be built from $root and installed under $prefix."
say "Nothing is downloaded except Rust crates listed in Cargo.lock. Nothing runs as root."
if [ "$assume_yes" != yes ]; then
  read -r -p "Continue? [y/N] " reply
  case $reply in y|Y|yes) ;; *) say "Cancelled."; exit 1 ;; esac
fi

cd "$root"
if [ -d build ]; then
  meson setup build --prefix="$prefix" --buildtype=release --reconfigure
else
  meson setup build --prefix="$prefix" --buildtype=release
fi
meson compile -C build
meson install -C build

say ""
say "Installed. Start it from your application menu (Local AI Workstation), or run:"
say "  $prefix/bin/local-ai-workstation"
case ":$PATH:" in *":$prefix/bin:"*) ;; *) say "Note: $prefix/bin is not on your PATH." ;; esac
say "Remove it later with: scripts/install.sh --uninstall"
