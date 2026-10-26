# GGUF metadata inspection

The `workstation-core` GGUF inspector reads a bounded header and tensor directory
from one local GGUF v3 file. It does not load tensor payloads, execute model code,
make network requests, or authenticate the file. `file_bytes` and computed tensor
bytes describe file layout; they are not a digest, authenticity check, or a resident
memory estimate.

Run the example with exactly one local path:

```sh
cargo run --locked -p workstation-core --example inspect_gguf -- /path/to/model.gguf
```

The inspector accepts little-endian, single-file GGUF v3 with flat metadata arrays.
Nested arrays and split files are outside this reader's supported subset and return
an error. It applies caps of 16 MiB for the header and tensor directory, 8,192
metadata entries, 65,536 tensors, 256-byte keys, 64-byte tensor names, 1 MiB
strings (128 bytes for architecture identifiers), 1,000,000 array elements, and
power-of-two alignment from 8 bytes through 1 MiB. Alignment defaults to 32 bytes.
Malformed types, invalid UTF-8 or booleans,
duplicate keys or tensor names, overflow, truncation, misalignment, out-of-bounds
data, and overlapping tensor ranges are rejected. Tensor dimensions must contain
1–4 positive values. Valid gaps between tensor ranges and trailing file data are
permitted.

## Retained split markers

llama.cpp's [split/merge tool at the checked reference commit](https://github.com/ggml-org/llama.cpp/blob/9febe63d5a379f8568bae7116a5b2ba844a4a4a6/tools/gguf-split/gguf-split.cpp)
retains the first shard's metadata when merging and resets `split.count` to zero.
The inspector accepts these markers only when all three are present and consistent:

| Key | Required type | Accepted value |
| --- | --- | --- |
| `split.no` | `u16` | `0` |
| `split.count` | `u16` | `0` (merged) or `1` (single file) |
| `split.tensors.count` | `i32` | Nonnegative, equal to the header tensor count |

Files without any split markers remain supported. Partial, mistyped, inconsistent,
or unknown `split.*` metadata is rejected. Counts greater than one remain unsupported
shards, even if their tensor data fits inside the selected file. All accepted files
still undergo the same bounded metadata and tensor-directory validation. The reader
does not search for siblings, join shards, or establish semantic completeness.

## Tensor and architecture declarations

The supported tensor types are F32, F16, Q4_0, Q4_1, Q5_0, Q5_1, Q8_0, Q2_K,
Q3_K, Q4_K, Q5_K, Q6_K, and BF16. Tensor byte sizes use the declared type's block
size and stored block byte size, with checked arithmetic. A `general.file_type`
label such as Q4_K_M does not replace per-tensor types: mixed tensor types are
possible.

Architecture-specific fields are reported only when their declared GGUF key is a
`u32` scalar with a positive value. Other encodings, including a spec-valid `u64`
value that fits in `u32`, are not coerced and cause inspection to fail for these
recognized keys, even for a non-LLaMA architecture. The reader does not fill in
architecture defaults, derive fields from tensor dimensions, or guess from file
or tensor names. Missing optional architecture fields remain `None`; invalid values fail
inspection.
`context_length` is the declared training context, not a requested runtime context.
The `attention` metadata object is present only for exact `llama` or `qwen2`
architecture identifiers and contains only the matching namespace. Other architectures
remain raw identifiers. The fit adapter may apply the narrowly supported Qwen2
head-length default; the inspector itself preserves missing declarations.

Warnings identify metadata categories that need more analysis. Their absence does
not prove that a model is dense, full-attention, causal, non-hybrid, or runtime
compatible. This inspection API does not produce a fit result, establish actual
runtime configuration, or count KV cache and inference buffers. The architecture
and layout constraints are recorded in [decision 0005](../05-decisions/0005-gguf-metadata.md).

The format details were checked against the [GGUF specification](https://github.com/ggml-org/ggml/blob/master/docs/gguf.md)
and llama.cpp at [b11798, commit 9febe63d5a379f8568bae7116a5b2ba844a4a4a6](https://github.com/ggml-org/llama.cpp/commit/9febe63d5a379f8568bae7116a5b2ba844a4a4a6)
on 2026-10-24. The GGUF specification evolves; this implementation intentionally
supports a narrower subset. Runtime and hardware acceptance are separate from inspection.

## Validation for this slice

On 2026-10-24, formatting, workspace Clippy with warnings denied, and workspace
tests passed: 11 GGUF integration tests and 2 existing bridge tests. Tests cover
all supported tensor block formats, hostile counts and arrays, the actual 16 MiB
header limit, arithmetic overflow, overlap and truncation, symlinks and nonregular
paths, isolated warning markers, and a reader that detects payload reads.

The repository validator and 4 Python regression tests passed. Meson 1.3.2 setup,
build, all 5 checks, and a staged `/usr` installation passed. Meson still reports
the foundation's two deprecated `configure_file(copy:)` uses. The staged install
does not run desktop cache updates on the host.

The example accepted a synthetic empty GGUF; malformed input, missing arguments,
and a FIFO returned failure, with the FIFO check completing before a five-second
timeout. No model weights were downloaded or executed. No GUI integration or
real-model acceptance was checked. M0 remains incomplete.

The file API uses the already locked `libc` crate for Unix `O_NONBLOCK`, plus checks
before and after opening that the target is regular. This avoids hanging on a FIFO
replacement without unsafe Rust; symlinks to regular cache blobs remain supported.

### Merged-file regression validation, 2026-10-26

The local Qwen2.5 Coder 7B Instruct Q4_K_M file previously failed with
`Unsupported GGUF: split metadata`. Its declarations are `split.no=0`,
`split.count=0`, and `split.tensors.count=339`. Read-only inspection now succeeds:
4,683,073,536 file bytes, 339 tensors, 4,677,120,000 tensor bytes, 32-byte alignment,
and architecture `qwen2`. No weight payload was loaded or executed. The fit example
also completes inspection and reports `UnsupportedArchitecture` / `Unknown`;
Qwen2 fit calculations are outside this change.

The positive regression failed before the parser fix and passed afterward. The
14 GGUF parser tests include merged and single-file markers, wrong types, partial
metadata, nonzero shard indexes, negative or mismatched totals, unknown keys,
real shards, every truncated prefix, overlapping ranges, and payload-read detection.
All 144 normal Rust tests and 9 Python tests passed, as did workspace formatting,
Clippy with warnings denied, and the repository validator. Four opt-in graphical
tests were skipped. No packaging files changed; local Meson and live GUI/inference
checks were not repeated. Independent parser review found no actionable issues.
This is inspection evidence, not M0 completion.

### Qwen2 fit follow-up

[Decision 0012](../05-decisions/0012-qwen2-gpu-fit.md) supersedes the earlier
Qwen2 fit limitation. Architecture-scoped declarations now feed conditional CPU and
CUDA estimates. See the [real 8K/32K evidence](m0-acceptance.md#real-fit-workflow).
