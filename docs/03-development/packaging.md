# Packaging: Flatpak and AppImage

`.github/workflows/release.yml` builds both packages. They are **not yet validated**: the workflow has been written
and checked for syntax, but no package built by it has been installed and run on a real machine. Treat the first
release that carries them as a trial.

## When it runs

| Trigger | What happens |
| --- | --- |
| Push of a tag `v*` | Both packages are built, then a `publish` job attaches them and a `SHA256SUMS` file to that tag's GitHub release (the release must already exist; create it first) |
| Pull request to `develop` that changes `packaging/` or the workflow | Both packages are built and kept as workflow artifacts for 7 days; nothing is released |
| Manual run | Same as a pull request |

Only the `publish` job has write access, and it runs no project code. Every third-party action is pinned to a commit.
The downloaded helper tools (the Cargo source generator, linuxdeploy and its GTK plugin) are pinned and their SHA-256
sums are checked before use.

## Flatpak

Manifest: `packaging/flatpak/io.github.dexterhere.Kvlm.yml`, GNOME runtime 51, Rust from the `rust-stable` SDK
extension. The workflow generates `cargo-sources.json` from `Cargo.lock` so the build needs no network. Install a
downloaded bundle with `flatpak install --user kvlm-<version>.flatpak`.

Sandbox permissions, and why:

| Permission | Reason |
| --- | --- |
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc` | Show the window |
| `--device=dri` | Let hardware probes and a managed llama.cpp build see the graphics card |
| `--share=network` | Reach Ollama, LM Studio and vLLM on the host's loopback, and run the downloads and update check the user starts. Nothing is requested at startup |
| `--talk-name=org.freedesktop.secrets` | Keep the Hugging Face token in the desktop keyring |

Not granted, so these behave differently from a native install: no general file access (pick models with the file
chooser), and no way to install a `systemd --user` unit or write editor configuration outside the sandbox. NVIDIA
probing depends on the matching Flatpak GL driver extension and is untested here.

## AppImage

Built on Ubuntu 24.04 with `linuxdeploy` and its GTK plugin, bundling GTK 4 and libadwaita. It runs on distributions
with a glibc at least as new as Ubuntu 24.04's (2.39); older systems should use the Flatpak or build from source.
Make it executable and run it; it needs FUSE 2, or run it with `--appimage-extract-and-run`. It does not integrate
into the launcher by itself.

## Trying it

Open a pull request that touches `packaging/` (or run the workflow by hand), then download the `flatpak` and
`appimage` artifacts. Report what happens on your distribution; see
[test coverage](test-coverage.md) for what is still unverified.
