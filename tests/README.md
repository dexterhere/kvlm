# Tests

Run Rust tests with `cargo test --workspace --locked`. Put unit tests beside the
implementation and integration tests under the owning crate's `tests/` directory.
This root is not a Cargo package; standalone Rust files here would not execute.

The initial application tests exercise real Tokio command/event delivery and worker
channel shutdown without initializing GTK. Supplied-metadata memory-fit tests live in
`crates/workstation-core/tests/fit_calculation_tests.rs`; they run without hardware,
models, or network access. Mock Ollama HTTP tests belong in
the separate runtime feature until its PR is integrated.

Shared sanitized data belongs in [fixtures](fixtures/README.md). Routine CI must
not need a GPU, installed runtime, network service, gated token, or model download.
Live runtime/hardware tests should be explicitly opt-in and document their setup.

GGUF integration tests in `crates/workstation-core/tests/gguf_metadata_tests.rs`
build synthetic byte fixtures, including malformed headers and tensor ranges.
They need no model downloads and verify that inspection avoids tensor payload reads.

`crates/workstation-core/tests/gguf_fit_tests.rs` exercises inspected synthetic
GGUFs through the calculator adapter, including missing dimensions, unconfirmed
layout, non-single-sequence plans, explicit K/V geometry, and budget-only rejection.

The repository validator has standard-library Python regression tests:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
```
