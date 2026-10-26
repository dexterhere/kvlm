# GGUF supplied-input fit example

`gguf_fit` connects bounded local GGUF inspection to the supplied-metadata fit
calculator. It accepts one local GGUF path and explicit CPU scenario values. It does
not probe hardware, read runtime settings, contact a runtime, download or execute a
model, or measure resident memory.

```sh
cargo run --locked -p workstation-core --example gguf_fit -- \
  "$GGUF_PATH" "$CONTEXT_TOKENS" "$MICROBATCH_TOKENS" \
  "$KEY_CACHE" "$VALUE_CACHE" "$SEQUENCE_COUNT" "$AVAILABLE_RAM_BYTES"
```

The positional arguments are the GGUF path, requested context tokens, prompt
microbatch tokens, key cache type, value cache type, sequence count, and currently
available RAM bytes. Use `unknown` for sequence count or available RAM when that
value is not known. Cache types are `f16`, `q8_0`, and `q4_0`. `context_tokens` and
`microbatch_tokens` must be positive. No values are inferred from model names or
training context.

By default the example uses `Unconfirmed` layout. It preserves the file-size
estimate and declarations but does not promote them into a transformer shape. The
calculator therefore reports an unknown assessment when architecture evidence is
incomplete. Add `--assume-uniform-attention` only when you want to make the explicit
assumption that the artifact has uniform full attention and contiguous K/V cache
rows. This assumption is not a verification; runtime compatibility always remains
unverified in this example. Missing any of the six required shape fields, a non-LLaMA
architecture, warning markers, or sequence counts other than one prevent a shape
from being promoted. The declared GGUF training context is not the requested context.

All scenario values are supplied by the command line, including RAM availability.
The result may include an arithmetic breakdown while runtime compatibility remains
unknown. The fit class stays `Unknown`, except `NotRecommended` when the supplied
scenario exceeds its selected budget; that classification says nothing about other
placements or runtime compatibility. It is a static estimate under the stated
assumptions, not an observed runtime allocation, benchmark, or proof that the artifact
loads. The example covers one CPU placement and does not implement GPU, hardware
discovery, attached or managed runtime integration, fit badges, or the rest of M0
acceptance.

See [decision 0006](../05-decisions/0006-gguf-fit-integration.md) for the evidence
and promotion contract. The dependent static-fit and GGUF feature commits are carried
together on this integration feature branch; their pull requests have not been
merged.

## Validation

On 2026-10-24, workspace formatting, Clippy with warnings denied, and all 37 Rust
tests passed: 8 adapter integration tests, 16 calculator tests, 11 parser tests,
and 2 bridge tests. Adapter fixtures exercise the complete parsing-to-estimate
path, all six missing dimensions, all nine warning categories, unknown/multiple
sequences, invalid geometry, mixed K/V cache types, overflow, and budget precedence.

Twenty CLI checks using a synthetic GGUF passed, including unconfirmed/assumed
layout, absent RAM, unknown/multiple sequences, separate cache types, numeric
bounds, invalid cache/flag values, nonregular paths, malformed files, and allocation
overflow. The two-layer reference fixture yields 393,216 F16 KV bytes for 512
allocated tokens; Q8_0 keys plus Q4_0 values yield 143,360 bytes. These are synthetic
arithmetic checks, not evidence that a model loads.

The repository validator, 4 Python tests, and all 5 Meson checks passed. Meson 1.3.2
build and a staged installation passed; the two existing `configure_file(copy:)`
deprecation warnings remain. No real-model, GPU, runtime, or GUI integration was
checked. No inference or model downloads occurred. M0 remains incomplete.

## Qwen2 and GPU follow-up, 2026-10-26

The adapter now accepts exact `llama` and `qwen2` identifiers. Qwen2 missing K/V
lengths use the GGUF embedding/head default only when the ratio is integral; explicit
lengths retain precedence. Namespace isolation, one sequence, no layout warnings and
an explicit attention assumption remain required. Runtime compatibility stays Unknown.
The UI separately displays conditional memory-policy classes for fresh CPU/CUDA
budgets, without transferring benchmark evidence to a selected GGUF. See
[decision 0012](../05-decisions/0012-qwen2-gpu-fit.md) and
[reference acceptance](m0-acceptance.md). Historical limitations above describe the
initial slice, not the current Qwen2 path.
