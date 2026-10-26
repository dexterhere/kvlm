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
or tensor names. Missing optional LLaMA fields remain `None`; invalid values fail
inspection.
`context_length` is the declared training context, not a requested runtime context.
The `llama` metadata object is present only when `general.architecture` is exactly
`llama`; other architecture strings remain raw declared metadata and are not
promoted to LLaMA fields.

Warnings identify metadata categories that need more analysis. Their absence does
not prove that a model is dense, full-attention, causal, non-hybrid, or runtime
compatible. This inspection API does not produce a fit result, establish actual
runtime configuration, or count KV cache and inference buffers. The architecture
and layout constraints are recorded in [decision 0005](../05-decisions/0005-gguf-metadata.md).

The format details were checked against the [GGUF specification](https://github.com/ggml-org/ggml/blob/master/docs/gguf.md)
and llama.cpp at [b11798, commit 9febe63d5a379f8568bae7116a5b2ba844a4a4a6](https://github.com/ggml-org/llama.cpp/commit/9febe63d5a379f8568bae7116a5b2ba844a4a4a6)
on 2026-10-24. The GGUF specification evolves; this implementation intentionally
supports a narrower subset. No real-model, runtime, or hardware acceptance is claimed.

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
