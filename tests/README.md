# Tests

Run Rust tests with `cargo test --workspace --locked`. Put unit tests beside the
implementation and integration tests under the owning crate's `tests/` directory.
This root is not a Cargo package; standalone Rust files here would not execute.

Application tests exercise Tokio command/event delivery, cancellation, and pure view
state/formatting without initializing GTK. Mock Ollama HTTP tests live beside the
adapter in `crates/workstation-runtimes/src/ollama/tests.rs`; they start only in-process,
ephemeral loopback servers. Future memory-fit tests belong in `workstation-core`.

Shared sanitized data belongs in [fixtures](fixtures/README.md). Routine CI must
not need a GPU, installed runtime, network service, gated token, or model download.
Live runtime/hardware tests should be explicitly opt-in and document their setup.

The repository validator has standard-library Python regression tests:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
```
