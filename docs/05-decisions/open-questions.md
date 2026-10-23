# Open specification questions

The original specification is preserved. Resolve these questions before implementing
the affected feature; they do not block the directory foundation.

| Area | Question to resolve |
| --- | --- |
| Fit classification (§4.5) | What is the complete CPU-only classification, and how are values between stated thresholds classified? |
| Unified memory (§4.1, §4.5) | How do GPU-mappable shared memory and physical host RAM share one capacity budget without double-counting? |
| KV math (§4.4) | How are block overhead, concurrency/slots, separate K/V data types, and nonstandard architectures accounted for? |
| Artifact sizing (§4.4, §6.1) | How are sharded safetensors bundles and on-disk versus resident memory represented? |
| Architecture tables (§4.4) | Which maintained `llmfit` integration/API and license will be used, and what happens with missing metadata? |
| Domain contracts (§4.2, §5.3) | Complete the omitted request/profile/event types; `ComputeBackend` with `String` payloads cannot derive `Copy`. |
| Benchmark persistence (§4.5, §8) | Define success/failure/cancellation, immutable profile snapshots, runtime versions, and repeated-run aggregation. |
| Token timing (§7.1–7.2) | Streaming chunks may contain multiple tokens; distinguish chunk arrival timing from true per-token ITL. |
| Diagnostics (§7.3) | Avoid describing intended CPU offload or APU shared memory as proven PCIe spill; define evidence and confidence. |
| Runtime import (§6.3) | Validate version-specific Ollama create/import semantics and which process can access a local blob path. |
| Runtime downloads (§5.1) | Confirm available official bundles, checksum provenance, backend support, and archive extraction protections. |
| Local-only mode (§5.4, §9.2) | Define how cloud routing is recognized and blocked when runtime metadata is incomplete. |
| Distribution (§9) | Confirm the application namespace and repository URL; `org.gnome.LocalAIWorkstation` is provisional and does not imply GNOME affiliation. |
| Public contribution | Configure real private security/conduct contacts and public hosting before publication. |

Treat runtime flags, wire formats, third-party integrations, and packaging examples
as version-sensitive. Verify against official upstream documentation when implementing
them rather than assuming every illustrative snippet works unchanged.
