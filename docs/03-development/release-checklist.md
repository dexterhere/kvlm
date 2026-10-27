# Release checklist

Releases are cut by the owner. Nothing here is automated, and no step may be marked done without evidence.

## Before tagging

1. Branch flow is complete: `develop` → `staging` → `main`, every pull request green.
2. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, `python3 scripts/check_repository.py` and the Python tests pass.
3. Packaging: `glib-compile-schemas --strict --dry-run data`,
   `appstreamcli validate --no-net data/org.gnome.LocalAIWorkstation.metainfo.xml.in`, then
   `meson setup build --prefix="$HOME/.local" --buildtype=release && meson compile -C build &&
   meson test -C build --print-errorlogs`, and a `DESTDIR` install that lists the binary, desktop file,
   metainfo, schema, icon and licence texts.
4. The application ID decision in [ADR 0019](../05-decisions/0019-application-id.md) is made and applied
   with `scripts/rename_app_id.py`. A store or Flathub submission needs a namespace the owner controls.
5. Hardware validation records exist for the machines listed in
   [Hardware validation procedures](hardware-validation-procedures.md), or the release notes say which are missing.
6. Manual passes are recorded: GNOME session, screen reader, large text, reduced motion
   ([accessibility](accessibility.md)).
7. Security and conduct contact addresses in `SECURITY.md` and `CODE_OF_CONDUCT.md` are real and monitored.

## Cutting the release

1. Set the version in `Cargo.toml` and `meson.build`, and the release `version` and `date` in the metainfo
   `<releases>` block (it currently carries a development entry, not a published release).
2. Move the `Unreleased` changelog entries under the version.
3. Tag from `main`, publish the GitHub release, and attach source only unless a binary build has been
   verified on a clean machine.

## The update check

`Overview → Updates → Check for updates` makes one HTTPS request to the GitHub latest-release feed for this
repository, only when pressed. It sends no identifier or version of the machine, follows no redirect, reads at
most 256 KiB, and shows the version and a release link built from the validated tag. It never downloads or
installs anything. Pre-release tags are not offered. Publish a non-draft, non-pre-release GitHub release for
the check to see it.
