# 0013: Exact benchmark verification

Date: 2026-10-26. Status: implemented on a dependent feature branch; integration pending.

## Decision

Benchmark history may identify a prior successful run for an installed Ollama model
and current hardware. This result verifies a saved request against its manifest and
hardware identity only. It does not guarantee future context, workload, backend,
runtime settings, memory use, or performance.

Ollama manifest digests use `canonical_manifest_digest`: accept 64 hexadecimal digits,
with or without the lowercase `sha256:` prefix, and normalize to lowercase prefixed
form. This identity is distinct from a GGUF file digest. Verification requires the
selected local model's exact name and canonical digest to match a fresh Ollama
discovery result. Cloud-routed, ambiguous, changed, or missing models do not qualify.

Hardware identity uses the `hardware-v2:` SHA-256 format. It includes host
architecture, CPU model, physical and logical core counts, available CPU feature
flags, total system RAM, and sorted GPU identity, name, vendor, dedicated VRAM and
shared-memory capacities, unified-memory flag, and CUDA compute capabilities. It
excludes free-memory readings, capture time, and driver version. Missing required
identity fields or an invalid GPU inventory yields no identity. Callers set
`profile_hash` from the recomputed identity.

`HistoryStore::verified_run` searches all records, independent of the recent-history
limit. SQL narrows candidates by exact canonical model and hardware fields, then
checks each JSON field against the 262,144-byte limit before typed decoding. It
revalidates both snapshot and completion manifest digests, the current profile's
stored hash against its recomputed identity, and both the snapshot's stored hash and
recomputed identity against that same current hash. It also requires exact loopback
endpoint `http://127.0.0.1:11434`, positive start time, nondecreasing finish time,
and nonblank runtime version. Only `Completed` records qualify, with response
chunks and elapsed time greater than zero, a first-response time no greater than
elapsed time, positive evaluation count and duration, and done reason `stop` or
`length`. Failed, cancelled, timed-out, unfinished, incomplete, malformed, oversized,
or inconsistent records never grant verification. Read errors preserve the database.

Hardware hashing is versioned without a schema migration. Existing raw hexadecimal
hash rows remain readable but cannot grant a badge under `hardware-v2`. No history
row is rewritten to upgrade its identity.

The Quick Benchmark view checks verification only for the current local installed
selection. It refreshes Ollama identity and hardware before querying history. UI
generation IDs discard stale replies after selection or inventory changes. Worker
errors leave the action retryable. The badge names the matching prior run and does
not claim fit for a future scenario.

## Consequences and limits

This policy establishes an exact saved-run match for one manifest and one
versioned hardware configuration. It does not establish artifact bytes for Ollama
manifests, effective runtime configuration, expected memory allocation, or continued
compatibility after software changes. Hardware identity omits free capacity by
design, so every check uses a fresh inventory for matching and never reuses a past
memory reading.

The implementation adds no schema migration and starts no inference during a check.
It does not expand M1 catalog behavior, add M3 benchmark workloads, or complete M0
hardware acceptance. See [benchmark history](0010-benchmark-history.md),
[Quick Benchmark](0009-quick-benchmark.md), and [hardware snapshots](0002-hardware-snapshots.md).
