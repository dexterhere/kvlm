# 0007: Desktop CPU GGUF fit scenario

Status: implemented as a local Overview workflow on the dependent feature branch.
Date: 2026-10-24.

## Context

The bounded GGUF inspector and supplied-input fit calculator can support a useful CPU
estimate before an inference runtime is connected. GGUF declarations cannot establish
runtime settings, a model's full attention semantics, or runtime compatibility. The
workflow must make its assumptions visible and keep blocking local I/O off the GTK
main thread.

## Decision

Add an Overview panel that selects a local GGUF with GTK's asynchronous file dialog,
then submits a typed scenario to the core worker. The user sees editable context and
microbatch presets (4096 and 512 tokens), separate K/V cache choices, a fixed one
sequence assumption, and an unchecked uniform-full-attention assumption. These values
are scenario inputs, not observed runtime configuration. The layout assumption can
permit a memory breakdown when the inspector and adapter gates pass; it cannot make
runtime compatibility known.

The worker inspects the selected regular file and takes a fresh CPU RAM reading for
each estimate. It uses `spawn_blocking`; it does not use the Overview's cached hardware
snapshot as fit input. Missing and zero-byte RAM readings remain distinct. The UI
clears metadata and assessments when the file, scenario, or hardware status changes.
Request IDs prevent stale replies from repopulating cleared results. A full command
queue and a closed worker channel have separate retry messages and both release the
reserved request ID.

The view renders metadata as plain text, presents layout warnings and an unknown
compatibility reason, and shows an optional four-component breakdown and the selected
CPU budget limits. It never presents a positive fit badge. A budget excess may be
reported as `Not recommended` for that supplied scenario only.

## Consequences and limits

This gives the user an end-to-end local file-to-scenario-to-static-estimate path without
running inference or contacting a runtime. It does not download or execute model code,
make network requests, alter runtime state, predict throughput, establish authenticity,
or provide benchmark verification. GPU placement, runtime compatibility, deployment,
and empirical validation remain out of scope. M0 remains incomplete.

The dependent feature branch carries the hardware-discovery and GGUF-inspection work
with this integration so the app can build against the complete interfaces. All
related pull requests remain unmerged and target `develop` under the repository Git
workflow; no PR merge is implied by this decision.

## Validation

Workspace formatting, Clippy with warnings denied, all 65 normal Rust tests, repository
validation, and 9 Python tests passed. The normal run ignores the graphical test;
Meson build, all five checks, and staged installation passed as well.
that GTK widget-flow test passed separately on an isolated Broadway display. It uses
private local path selection to avoid depending on an external file chooser. Its command and
configuration requirements are documented in the [desktop CPU fit guide](../03-development/desktop-cpu-fit.md#optional-gtk-widget-flow-check).

The rendered Overview was inspected through an isolated local GTK4 Broadway display.
The available browser controls did not exercise the rendered GTK surface, so this
does not claim an interactive file chooser test. The isolated portal emitted backend
warnings, with no application panic or GTK critical error in the successful test. Native
Wayland/X11, real-model, GPU, accessibility, and minimum-version acceptance remain
unchecked. No M0 completion or real-hardware acceptance is claimed.
