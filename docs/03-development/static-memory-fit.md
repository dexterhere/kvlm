# Static memory fit: core calculator

This branch implements the supplied-metadata calculator, not desktop fit badges or
automatic model architecture discovery. Hardware PR #1 and Ollama PR #2 remain unmerged.
The four-crate layout and original application shell remain intact.

## Run the offline example

```sh
cargo run --locked -p workstation-core --example memory_fit
cargo test --locked -p workstation-core --test fit_calculation_tests
```

The example labels its inputs synthetic. It does not inspect this computer, read a
model, contact an endpoint, or run a benchmark. It supplies a 4 GiB artifact, 32 layers,
32 query heads, two KV heads, 128-element key/value heads, 4096 embedding length, F16
cache, microbatch 512, and 6 GiB available dedicated memory. Runtime compatibility is
an explicit fixture assumption.

| Context | Weights | KV cache | Buffers | Allowance | Estimated total | Fixture classification |
| --- | --- | --- | --- | --- | --- | --- |
| 8192 | 4294967296 | 268435456 | 268435456 | 367001600 | 5198839808 | Comfortable |
| 32768 | 4294967296 | 1073741824 | 268435456 | 367001600 | 6004146176 | Constrained |

All amounts are bytes. This is not evidence about a named 7B model: models with the
same parameter count can have very different KV dimensions and allocations.

## API contract

Use `workstation_core::fit::memory_math::assess_fit` with `ModelInput`, `RuntimeConfig`,
`ExecutionTarget`, and `RuntimeCompatibility`. The result includes a typed class/reason,
optional four-part breakdown, and optional headroom limits. Input errors are separate
from Unknown estimates. No output grants benchmark verification.

Only pass `Some(TransformerShape)` for complete metadata describing the supported,
uniform full-attention, single-sequence contiguous cache layout. Leave unsupported or
missing metadata absent. Only pass Supported compatibility after validating the actual
artifact/backend/cache/layout combination; the calculator cannot establish that itself.
Available-memory readings must be current. A result contains no freshness guarantee.

The [decision](../05-decisions/0004-static-memory-fit.md) defines exact arithmetic,
classification thresholds, the shared-memory cap, and why `llmfit` was evaluated but
not added. File bytes, buffers, and runtime allowances remain estimates of residence.

## Validation record

Date: 2026-10-24.

- Sixteen independently authored fit integration tests passed.
- `cargo test --workspace --locked`: 18 tests passed on this independent foundation
  base (16 fit, two application bridge); hardware/Ollama tests live in their own PRs.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all --check`: passed, including the example.
- Repository link/identity validation and four Python tests: passed.
- Meson 1.3.2 configure/build, five native metadata/repository tests, and installation
  into repository-local `stage/usr`: passed. No host `/usr` files were changed.
- The offline example produced the reference results above.
- Independent code review found no actionable findings within the declared scope.

The independent branch includes the existing small AppStream developer/homepage
correction so Ubuntu packaging does not depend on merging either earlier PR.

Tests cover separate/asymmetric K/V formats and dimensions, quantization block
overhead, context padding, scratch-buffer floor/growth, backend allowances, inclusive
CPU/GPU/shared limits, `u64::MAX` capacities, missing versus zero readings, runtime
support states, invalid head groups, and checked overflow. Tests require no GPU,
models, inference process, network, or additional dependencies.

Not checked: live runtime allocation calibration, actual model metadata extraction,
desktop presentation/accessibility, and combined hardware/Ollama integration. Full
M0 Step 4 and M0 completion remain pending. The next slice should establish validated
GGUF/runtime metadata and plan compatibility, then wire this API into real fit badges.
