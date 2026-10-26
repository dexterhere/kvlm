# M1 plan: Model Explorer, Hugging Face catalog, and deduplicator

Companion documents: [Implementation](Implementation.md) · [Diagrams](Diagrams.md) ·
[Milestones](../milestones.md)

## What M1 delivers

M0 answers "can my machine run the model I already have?". M1 answers "which model
should I get, will it fit, and do I already have it?". A developer opens the Model
Explorer, searches public Hugging Face for GGUF models, browses them as
**Family → Release → Artifact**, sees file size, quantization, license, revision and
checksum, sees a conditional memory-fit estimate for their hardware, learns whether the
same file already sits in their Ollama or Hugging Face storage, and can download what is
missing with progress, pause, resume and integrity verification. Gated models work
through a token the application keeps in the desktop keyring.

## Who it is for

Linux developers running local models on their own hardware (the M0 audience). They
decide which quantization of which model to try before spending gigabytes of disk and
bandwidth, and they already have models from Ollama or other tools.

## Scope boundaries

- Hugging Face only. The Ollama registry is deferred; local Ollama files are still read
  to detect duplicates.
- GGUF files only. Safetensors, EXL2 and AWQ are listed as unsupported, not hidden.
- Sharded GGUF files (`-00001-of-0000N`) are shown as sharded; fit is Unknown and
  download is disabled until a validated sharding contract exists.
- Nothing contacts the network at startup. Every request follows an explicit user
  action (search, inspect, download, validate token). Previously fetched results remain
  browsable offline and are labelled with their age.
- The application never changes, moves, links or deletes files it does not own (Ollama
  blobs, Hugging Face cache). It reports duplicates; it does not merge storage.
- No model is loaded into a runtime and no Ollama import is performed in M1.
- Unknown stays Unknown: missing size, license, checksum or metadata is displayed as
  such and never guessed. A file without a published checksum cannot be downloaded.

## Phases and pull requests

Each phase is one reviewable pull request into `develop`, built in this order.

| Phase | Deliverable | User-visible result |
| --- | --- | --- |
| 1 | Read-only Model Explorer | Search, Family → Release → Artifact browser, loading/error/retry/offline states |
| 2 | Artifact details | Size, quantization, license, revision, checksum, bounded GGUF metadata, conditional fit estimate |
| 3 | Local discovery and deduplication | "Already on disk" with a verified content digest and the file location |
| 4 | Resumable downloads | Progress, cancel, resume after interruption, checksum verification, safe partial files |
| 5 | Gated-model access | Token stored in the keyring, clear access-required guidance |
| 6 | M1 acceptance | Recorded evidence on the reference machine, documentation, milestone status |

## Trust and safety decisions

- Downloads are pinned to a specific repository revision, so a file cannot change
  between what the user inspected and what is fetched.
- A download is accepted only if its SHA-256 matches the checksum Hugging Face
  publishes; a mismatch deletes the partial file and reports the failure.
- Tokens live only in the desktop keyring. If no keyring is available, token features
  are disabled with an explanation; there is no plaintext fallback.
- Remote names (repository, filename) are untrusted. They are displayed but never used
  to build file paths.
- The application sends no analytics and nothing beyond what the explicit action needs.

## Deferred and why that is fine

| Deferred | Reason |
| --- | --- |
| Ollama registry search | Not in the M1 milestone text; separate undocumented source |
| Sharded GGUF download and fit | No validated contract for combined size and layout |
| Safetensors/EXL2/AWQ | No M1 runtime can use them |
| Reusing a found file through Ollama import or llama-server | Belongs to M2 runtime management |
| Hard disk-space preflight | Needs a safe filesystem query; write failures are handled instead |
| Cross-device storage merging (links) | Mutates user storage semantics; report-only in M1 |

## Done means

M1 is complete when the reference machine shows search, details, fit, duplicate
detection, an interrupted-then-resumed download with a verified checksum, a stored
token surviving restart, and a gated-access refusal, all recorded in an M1 acceptance
document with its limits, and CI is green. Merging is not completion.
