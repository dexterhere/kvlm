# M1 acceptance evidence

Date: 2026-10-27. Status: **M1 is implemented on `develop`; acceptance is not signed off.**
One required check is pending (the token restart check below), and sign-off is the project
owner's decision. This page records what was run, what it showed, and what it does not claim.

M1's definition of done ([plan](../04-roadmap/m1/Planned.md)) is search, details, fit,
duplicate detection, an interrupted-then-resumed download with a verified checksum, a stored
token surviving restart, and a gated-access refusal, with CI green.

## Reference machine

The M0 reference machine ([M0 record](m0-acceptance.md)): Linux, Hyprland session, Intel Core
i5-13420H, NVIDIA GeForce RTX 4050 Laptop GPU, 16 GB RAM, gnome-keyring as the Secret Service.
A GNOME Shell session is not installed, so GNOME-session behavior is not claimed.

## Live run

Command: `cargo run --locked -p workstation-runtimes --example m1_acceptance -- <empty dir> catalog`
(then `gated`). It talks to huggingface.co, uses its own database and model store in the given
directory, and prints no token or file content. The directory was removed afterwards.

| Capability | Result |
| --- | --- |
| Search | 30 repositories returned for "tiny", first page, anonymous |
| Details | Smallest checksummed single file among the first results: `Ling-3.0-tiny-imatrix.gguf` in `bloomer010/Ling-3.0-tiny-GGUF`, 44,016,704 bytes, SHA-256 listed, license `mit`, pinned to commit `9abc60203407…` |
| Header | First bytes read: 664 tensors, weights 43,965,980 bytes. No architecture is declared in this file, so a fit would be `Unknown`, as designed |
| Interrupted download | Paused at 15,663,735 of 44,016,704 bytes; one partial file kept |
| Resume | Completed at 44,016,704 bytes; published as `blobs/sha256-566f65b2…37c9` only after the digest matched; partial file removed |
| Duplicate detection | Scanning the store found 1 verified local copy |
| Gated refusal | `google/gemma-2b` (access: manual). An anonymous file request returned `AccessRequired`, and the message tells the user to accept the terms on huggingface.co and add a token; the application did not try to accept anything |

The run also showed that the search list does not reliably say which repositories are gated,
while a repository's detail does. The exact access therefore appears once a repository is
expanded; the list should not be read as proof that a repository is open.

## Token survives restart: pending

The same example has two separate-process steps that store a throwaway token only if none is
saved, read it back from a fresh process, and remove it:

```sh
cargo run --locked -p workstation-runtimes --example m1_acceptance -- /tmp/x keyring-save
cargo run --locked -p workstation-runtimes --example m1_acceptance -- /tmp/x keyring-check
```

The expected output is `token survived restart: true`, `in memory after load: true`, and
`still present afterwards: false`. These steps write to the user's real desktop keyring, so
the automated session did not run them; the permission was refused and that was respected.
Until a person runs them and records the output here, "stored token surviving restart" is
not verified. Unit tests with an in-memory store cover the surrounding logic (save, load,
clear, failure and timeout paths), but they do not exercise gnome-keyring.

## Automated checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked`, `python3 scripts/check_repository.py`, and the Python
helper tests pass, and CI (`foundation`, `git-policy`) was green on each phase PR (#20 to
#27). Opt-in GTK smoke tests for the explorer, downloads and token panels passed on this
machine, run one at a time.

## Not claimed

- No manual walkthrough of the interface with a human operator was recorded; the GTK smoke
  tests drive the panels with typed fixtures.
- GNOME Shell behavior, accessibility with a screen reader, and keyboard-only operation have
  not been checked.
- Only one real download (44 MB) was exercised live. Multi-gigabyte transfers, a full disk,
  and a flaky network are covered by tests with a local server, not by live runs.
- Fit was exercised live only as far as reading a header; qwen2 and llama fit accuracy is
  M0's accepted baseline.
- Sharded models, other runtimes' registries, and importing a download into Ollama are
  deferred by the plan.

## Sign-off

Awaiting the token restart output above and the project owner's decision. Merging the phase
PRs into `develop` is not acceptance, and promotion to `staging` and `main` follows the Git
workflow only after sign-off.
