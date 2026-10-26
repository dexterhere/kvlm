# Tests

Run Rust tests with `cargo test --workspace --locked`. Put unit tests beside the
implementation and integration tests under the owning crate's `tests/` directory.
This root is not a Cargo package; standalone Rust files here would not execute.

The initial application tests exercise real Tokio command/event delivery and worker
channel shutdown without initializing GTK. Supplied-metadata memory-fit tests live in
`crates/workstation-core/tests/fit_calculation_tests.rs`; they run without hardware,
models, or network access. Mock Ollama HTTP tests live in `crates/workstation-runtimes/src/ollama/tests.rs`.
They bind ephemeral loopback ports without connecting to an installed runtime or the
Internet, and cover discovery plus read-only model inspection.

Shared sanitized data belongs in [fixtures](fixtures/README.md). Routine CI must
not need a GPU, installed runtime, network service, gated token, or model download.
Live runtime/hardware tests should be explicitly opt-in and document their setup.

Hardware fixtures and probe tests live in `crates/workstation-core`; see the
[hardware validation record](../docs/03-development/hardware-validation.md).

GGUF integration tests in `crates/workstation-core/tests/gguf_metadata_tests.rs`
build synthetic byte fixtures, including malformed headers and tensor ranges.
They need no model downloads and verify that inspection avoids tensor payload reads.

`crates/workstation-core/tests/gguf_fit_tests.rs` exercises inspected synthetic
GGUFs through the calculator adapter, including missing dimensions, unconfirmed
layout, non-single-sequence plans, explicit K/V geometry, and budget-only rejection.

App tests cover fit request generations, validation, fresh-memory error semantics,
queue preservation, background execution, and shutdown. The ignored GTK widget-flow
test is opt-in and needs a display; see the [desktop guide](../docs/03-development/desktop-cpu-fit.md#optional-gtk-widget-flow-check).
Normal CI does not open GTK windows. The display test uses synthetic files and the
real fit worker; it bypasses native file-chooser interaction.

The repository validator has standard-library Python regression tests:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
```

The ignored Ollama inspection widget-flow test drives real Refresh/Inspect controls
with typed fixtures; see the [inspection guide](../docs/03-development/ollama-inspection.md#optional-gtk-checks).
It checks selected-model rendering, stale-result handling, retries, and narrow layout.

Quick Benchmark HTTP/parser tests live in
`crates/workstation-runtimes/src/ollama/generation/tests.rs`. Runner tests cover
independent metrics, partial failures, cancellation, sender loss, and deadlines;
bridge tests cover concurrent commands, duplicate rejection, panics, and shutdown.
The ignored GTK benchmark test uses typed fixtures and actual controls, not an
inference service. See the [Quick Benchmark guide](../docs/03-development/quick-benchmark.md).
