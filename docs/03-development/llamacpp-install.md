# Portable llama.cpp install

Status: implemented (M2 phase 2b), not yet wired into the interface (phase 2f).

The application can install an official, portable `llama-server` build from
`github.com/ggml-org/llama.cpp` without root. Every step follows an explicit user action; nothing
downloads at startup.

## What is checked

1. **Release selection.** `GET /repos/ggml-org/llama.cpp/releases?per_page=20` (8 MiB cap). The newest
   non-draft release that offers a wanted backend wins. `releases/latest` is not used (upstream points it at
   a nightly marker, and every build is flagged pre-release). Archive names and download URLs are built from
   the tag (`b<digits>`); nothing is copied from the response except size, state and digest.
2. **Digest required.** An archive without a `sha256:` digest, in a state other than `uploaded`, or larger than
   1 GiB is never offered. Backends: CPU always, Vulkan when a Vulkan ICD file exists, CUDA 12.8 when the
   NVIDIA kernel module is from the 570 series or newer (it needs the separate `cudart` bundle, so its size
   is the sum of two archives).
3. **Download.** Redirects are followed by hand (at most 4) and only to `github.com`,
   `objects.githubusercontent.com` and `release-assets.githubusercontent.com` over https on port 443; DNS
   answers that are not public addresses are refused. The body is hashed while streaming; size and SHA-256
   must equal the published values.
4. **Extraction** (`llamacpp/extract.rs`). Streaming gzip+tar into a private staging directory on the same
   filesystem. One top-level directory is stripped; names are limited to `[A-Za-z0-9._+-]`, 128 bytes, depth 4.
   Files are created `O_EXCL|O_NOFOLLOW`. Only regular files, directories and relative symlinks to a sibling
   file that exists in the archive are accepted; hard links, devices, duplicates and cycles are refused.
   Caps: 2,000 entries and 2.5 GiB of **decompressed** output.
5. **Run check.** `llama-server --version` must exit 0 within 10 s in a cleared environment. A build that does
   not run (missing system libraries, wrong glibc) is never published.
6. **Publish.** Atomic no-replace rename to `runtimes/llamacpp/<tag>-<backend>/`. A directory lock
   (`.lock`, `flock`) serialises installs and removals across instances; staging leftovers from a crash are
   removed under the lock.

## Removal

`remove_install` refuses the build a managed process is running from, names that are not plain build names,
and anything that is not a real directory (a symlink is never followed). Only directories recorded in
`runtime_installs` are passed to it.

## Live check

```sh
cargo run --locked -p workstation-runtimes --example m2_acceptance -- <empty-abs-dir> install cpu
```

Recorded on 2026-10-27: build `b11905` CPU (22,497,278 bytes) selected, verified against the published
digest, extracted, `llama-server --version` reported `0.7.0-dev (build 11905)`.
