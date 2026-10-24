# Tests

Run Rust tests with `cargo test --workspace --locked`. Put unit tests beside the
implementation and integration tests under the owning crate's `tests/` directory.
This root is not a Cargo package; standalone Rust files here would not execute.

The initial application tests exercise real Tokio command/event delivery and worker
channel shutdown without initializing GTK. Future memory-fit tests belong in
`crates/workstation-core/tests/`; mock Ollama HTTP tests belong in
`crates/workstation-runtimes/tests/`. Add them when the behavior is implemented.

Shared sanitized data belongs in [fixtures](fixtures/README.md). Routine CI must
not need a GPU, installed runtime, network service, gated token, or model download.
Live runtime/hardware tests should be explicitly opt-in and document their setup.

GGUF integration tests in `crates/workstation-core/tests/gguf_metadata_tests.rs`
build synthetic byte fixtures, including malformed headers and tensor ranges.
They need no model downloads and verify that inspection avoids tensor payload reads.

The repository validator has standard-library Python regression tests:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
```
