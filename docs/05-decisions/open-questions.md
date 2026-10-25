# Open specification questions

The original specification is preserved. Resolve these questions before implementing
the affected feature; they do not block the directory foundation.

| Area | Question to resolve |
| --- | --- |
| Fit classification (§4.5) | Initial resident-placement CPU/GPU limits and gap handling are defined in [0004](0004-static-memory-fit.md); explicit offload plans and calibration remain open. |
| Unified memory (§4.1, §4.5) | [0004](0004-static-memory-fit.md) caps by both mappings and host RAM without addition. Live allocation overlap/calibration remains open. |
| KV math (§4.4) | [0004](0004-static-memory-fit.md) covers standard uniform attention, separate K/V blocks and one padded sequence. Other layouts, slots and concurrency remain unsupported. |
| Artifact sizing (§4.4, §6.1) | Single-GGUF file bytes are an explicit resident-weight estimate. Shards, safetensors, repacking and measured residence still need a validated metadata contract. |
| Architecture tables (§4.4) | `llmfit-core` 1.1.17 (MIT) was evaluated and deferred; [0004](0004-static-memory-fit.md) explains why. [0005](0005-gguf-metadata.md) inspects declarations and [0006](0006-gguf-fit-integration.md) permits explicit scenarios; real layout/compatibility evidence remains open. |
| Domain contracts (§4.2, §5.3) | Complete the omitted request/profile/event types; `ComputeBackend` with `String` payloads cannot derive `Copy`. |
| Benchmark persistence (§4.5, §8) | [0010](0010-benchmark-history.md) defines immutable requested snapshots, terminal outcomes, runtime-version availability, and restart history. Artifact/hardware verification, effective settings, retention, and repeated-run aggregation remain open. |
| Token timing (§7.1–7.2) | [0009](0009-quick-benchmark.md) records first response-text latency and separate server decode counters. Per-token ITL, thinking-only workloads, and repeated cold/warm protocols remain open. |
| Diagnostics (§7.3) | Avoid describing intended CPU offload or APU shared memory as proven PCIe spill; define evidence and confidence. |
| Runtime import (§6.3) | Validate version-specific Ollama create/import semantics and which process can access a local blob path. |
| Runtime downloads (§5.1) | Confirm available official bundles, checksum provenance, backend support, and archive extraction protections. |
| Local-only mode (§5.4, §9.2) | [0009](0009-quick-benchmark.md) requires affirmative server cloud-disable status plus immutable local digest references for Quick Benchmark. Other runtimes, older protocols, and OS-level network isolation remain unresolved. |
| Distribution (§9) | Confirm the application namespace; `org.gnome.LocalAIWorkstation` is provisional and does not imply GNOME affiliation. The selected repository is `https://github.com/dexterhere/kvlm`, configured as origin on 2026-10-24. |
| Public contribution | Configure real private security/conduct contacts and verify hosting/reporting settings before publication. |

Treat runtime flags, wire formats, third-party integrations, and packaging examples
as version-sensitive. Verify against official upstream documentation when implementing
them rather than assuming every illustrative snippet works unchanged.
