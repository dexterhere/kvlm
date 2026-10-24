# 0005: Bounded GGUF metadata inspection

Status: accepted. Date: 2026-10-24.

## Context

Static fit work needs a way to inspect a local GGUF's declared architecture and
tensor layout without reading weight payloads or executing model-supplied code.
GGUF is extensible, and a syntactically valid file can contain metadata or tensor
layouts this application does not understand. Declared architecture values also do
not establish a runtime's selected context, memory use, or compatibility.

The reference points are the [GGUF specification](https://github.com/ggml-org/ggml/blob/master/docs/gguf.md)
and llama.cpp [b11798 at commit 9febe63d5a379f8568bae7116a5b2ba844a4a4a6](https://github.com/ggml-org/llama.cpp/commit/9febe63d5a379f8568bae7116a5b2ba844a4a4a6),
checked 2026-10-24. GGUF v3's tensor offsets are relative to the aligned tensor-data
section. Tensor storage size depends on each tensor's declared type and dimensions;
file-level quantization labels do not determine every tensor's type.

## Decision

- Implement a local, read-only metadata inspector in `workstation-core` for bounded,
  little-endian, single-file GGUF v3 and flat metadata arrays. Reject split-file
  metadata and nested arrays as unsupported by this subset.
- Bound header and directory size, entry counts, strings, keys, names, arrays, and
  alignment before allocating or traversing attacker-controlled lengths. Reject
  malformed encodings, duplicate identifiers, invalid tensor layouts, arithmetic
  overflow, truncation, and ranges outside the file.
- Compute tensor bytes from known per-tensor GGML types with checked arithmetic;
  do not read tensor payload data.
- Preserve architecture metadata only when declared as typed, positive `u32` fields.
  Do not coerce even spec-valid `u64` declarations into `u32`.
  Missing fields remain `None`; do not derive values from tensor shapes or names,
  apply implicit defaults, or infer metadata from filenames.
- Populate the LLaMA-specific record only for exact `general.architecture=llama`.
  Keep other architecture identifiers as raw optional strings.
- Treat layout warnings as indicators for follow-up only. Presence can block a
  future narrow fit path; absence does not prove dense full attention, causal
  attention, or lack of hybrid, sliding-window, recurrent, or auxiliary behavior.
- Keep inspection separate from model-fit classification. Declared training context
  is not a runtime request, actual runtime configuration, or memory estimate.

## Consequences and validation

The bounded subset may reject future or less common valid GGUF encodings. This is
preferable to silently misreading them or promoting uncertain architecture data.
Supporting more encodings requires updating the parser contract and its adversarial
tests against the then-current primary sources. The example and development guide
show how to inspect one file and explain the limits.

Workspace formatting, Clippy, 13 Rust tests, repository validation, 4 Python tests,
and 5 Meson checks passed, along with a staged install and synthetic CLI checks.
See the [validation notes](../03-development/gguf-metadata.md#validation-for-this-slice).
No real-model, runtime, or hardware acceptance is claimed; M0 remains incomplete.
