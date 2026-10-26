# 0019: Application ID and public contacts

Date: 2026-10-27. Status: **open, owner decision required.** The software side is done; nothing here has been
registered or published.

## Problem

The application ID `org.gnome.LocalAIWorkstation` is a placeholder. IDs under `org.gnome.` identify applications that
are part of the GNOME project; this project is independent and not a GNOME Circle member, so shipping under that name
would misstate its origin and could collide with a real GNOME application. The ID is also the D-Bus name, the
GSettings schema path, the desktop and AppStream identity, the icon name, the Flathub submission key, and the keyring
attribute that finds a saved Hugging Face token.

## Options

| Option | Example | Notes |
| --- | --- | --- |
| Code-hosting ID | `io.github.dexterhere.LocalAIWorkstation` | The Flathub convention for projects hosted on GitHub; ownership is verifiable through the repository. Depends on the repository path staying `dexterhere/...` |
| Domain-based ID | `org.example.LocalAIWorkstation` | Needs a domain the owner controls and will keep; verifiable by DNS |
| Keep the placeholder | `org.gnome.LocalAIWorkstation` | Acceptable only for local development builds; not for a tag, a package or a store submission |

Recommendation: the code-hosting ID, because it needs no new infrastructure. The owner decides.

## What is in place

The ID lives in one Rust constant (`APP_ID` in `workstation-core`), one Meson variable and the names of four data
files. `scripts/rename_app_id.py NEW_ID` prints every file it would rename or rewrite and, with `--apply`, performs the
change with `git mv` and a text replace; it refuses malformed IDs and IDs under `org.gnome.`, `org.freedesktop.` and
`org.kde.`. `scripts/check_repository.py` checks that the desktop, AppStream, schema, icon and constant agree, so a
partial rename fails the repository check. A token saved under the old ID is not found after a rename and must be saved
again (the project has no stable release, so no migration is provided).

## Public contacts

`SECURITY.md`, `CODE_OF_CONDUCT.md` and `SUPPORT.md` deliberately publish no private address. The owner must enable
private vulnerability reporting on the repository or publish a monitored address, and name a conduct contact, before
inviting public reports. Invented addresses are not acceptable. The AppStream `<url>` and `<developer>` entries should
be filled in at the same time.

## Steps for the owner

1. Choose the ID and run `python3 scripts/rename_app_id.py <id>`, review, then `--apply`.
2. Run `cargo test --workspace --locked`, `python3 scripts/check_repository.py`, `glib-compile-schemas --strict
   --dry-run data` and `appstreamcli validate --no-net data/<id>.metainfo.xml.in`.
3. Enable private reporting or publish a contact; update the three community files and the AppStream URLs.
4. Only then tag a release or submit to a store.
