# Documentation

Read these sections in order when learning the project. Repository-wide community
policies stay at the root so contributors and hosting platforms can find them.

| Order | Document | Purpose |
| --- | --- | --- |
| 01 | [Master specification](01-product/gnome_local_ai_workstation_master_specification.md) | Complete product and technical target, preserved from the original document |
| 02 | [Architecture](02-architecture/README.md) | Crate boundaries, threading, data flow, safety |
| 03 | [Getting started](03-development/getting-started.md) | Dependencies, build, run, validation, installation |
| 03 | [Repository layout](03-development/repository-layout.md) | Where code, fixtures, metadata, and documentation belong |
| 03 | [Git workflow](03-development/git-workflow.md) | Small commits, authorship, review, publication |
| 03 | [M0 acceptance](03-development/m0-acceptance.md) | Real Qwen CPU/CUDA estimates, native benchmark, restart badge, accepted scope, and follow-ups |
| 03 | [Foundation validation](03-development/validation.md) | Checks performed and practical limits |
| 03 | [Hardware validation](03-development/hardware-validation.md) | Hardware feature checks and live reference evidence |
| 03 | [Ollama discovery validation](03-development/ollama-validation.md) | Read-only attached-runtime discovery and validation limits |
| 03 | [Ollama model inspection](03-development/ollama-inspection.md) | Selection, declarations, fresh loaded readings, and validation limits |
| 03 | [Benchmark memory sampling](03-development/benchmark-memory-sampling.md) | Bounded background memory peaks, coverage, and attribution limits |
| 03 | [Benchmark history](03-development/benchmark-history.md) | SQLite snapshots, outcomes, restart recovery, and storage limits |
| 03 | [Quick Benchmark](03-development/quick-benchmark.md) | Local-only guard, one explicit streaming request, timings, cancellation, and limits |
| 03 | [Model downloads](03-development/model-downloads.md) | Explicit download, pause, resume, discard, and what stops a transfer |
| 03 | [Hugging Face access](03-development/huggingface-access.md) | Gated models, keyring-only token, and what is sent where |
| 03 | [M1 acceptance evidence](03-development/m1-acceptance.md) | Live catalog, resume, dedup and gated-refusal results |
| 03 | [Portable llama.cpp install](03-development/llamacpp-install.md) | Verified, rootless llama-server install, safe extraction, removal rules |
| 03 | [Managed runtime supervisor](03-development/runtime-supervisor.md) | Lifecycle, identity-checked readiness, bounded logs, process groups, crash reclaim |
| 03 | [llama.cpp HTTP client](03-development/llamacpp-client.md) | Loopback-only readiness, props/slots/metrics, bounded streaming chat |
| 03 | [Developer exports](03-development/developer-exports.md) | Continue.dev, Zed, OpenCode and systemd --user text generators |
| 03 | [Managed runtime in the interface](03-development/managed-runtime-ui.md) | Runtimes card: install, profiles, start/stop, logs, shutdown |
| 03 | [Static memory fit](03-development/static-memory-fit.md) | Offline example, core API, validation, and remaining integration |
| 03 | [GGUF metadata inspection](03-development/gguf-metadata.md) | Local inspection, merged-file split markers, validation evidence, and limits |
| 03 | [GGUF fit scenarios](03-development/gguf-fit.md) | Local declarations with explicit settings, assumptions, and unknown compatibility |
| 03 | [Desktop CPU/CUDA fit workflow](03-development/desktop-cpu-fit.md) | File selection, explicit inputs, fresh RAM, asynchronous estimates, and validation limits |
| 04 | [Milestones](04-roadmap/milestones.md) | Current state and M0–M4 delivery order |
| 04 | [M1 plan](04-roadmap/m1/Planned.md) | Model Explorer, Hugging Face catalog, deduplication, downloads: [implementation](04-roadmap/m1/Implementation.md) and [diagrams](04-roadmap/m1/Diagrams.md) |
| 04 | [M2 plan](04-roadmap/m2/Planned.md) | Hybrid runtime manager, managed llama.cpp, supervisor, developer exports |
| 04 | [M3 plan](04-roadmap/m3/Planned.md) | Benchmark lab, cold/warm protocol, A/B comparison, diagnostic advisor |
| 04 | [M4 plan](04-roadmap/m4/Planned.md) | LM Studio and vLLM adapters, accessibility, packaging, and what needs hardware or the owner |
| 04 | [Next feature: hardware discovery](04-roadmap/next-feature-hardware-discovery.md) | Implementation sequence, scope, and acceptance checks for M0 Step 2 |
| 05 | [Decisions](05-decisions/README.md) | How to record architecture decisions |
| 05 | [Foundation decision](05-decisions/0001-workspace-foundation.md) | Accepted setup choices |
| 05 | [Hardware discovery decision](05-decisions/0002-hardware-snapshots.md) | Hardware snapshots and probe limits |
| 05 | [Attached Ollama decision](05-decisions/0003-attached-ollama-discovery.md) | Loopback discovery and external ownership |
| 05 | [Static memory-fit decision](05-decisions/0004-static-memory-fit.md) | Supported layouts, byte accounting, and placement thresholds |
| 05 | [GGUF metadata decision](05-decisions/0005-gguf-metadata.md) | Bounded declarations and tensor layout without runtime or fit claims |
| 05 | [GGUF fit integration decision](05-decisions/0006-gguf-fit-integration.md) | Metadata gates and scenario-only estimates |
| 05 | [Desktop CPU fit decision](05-decisions/0007-desktop-cpu-fit.md) | GTK workflow, fresh readings, and stale-reply handling |
| 05 | [Ollama inspection decision](05-decisions/0008-ollama-model-inspection.md) | Read-only inspection, identity association, and unknown runtime settings |
| 05 | [Quick Benchmark decision](05-decisions/0009-quick-benchmark.md) | Digest-pinned local execution and honest single-request measurements |
| 05 | [Benchmark history decision](05-decisions/0010-benchmark-history.md) | Versioned local SQLite storage and immutable run records |
| 05 | [Memory sampling decision](05-decisions/0011-benchmark-memory-sampling.md) | Observed memory peaks and additive history compatibility |
| 05 | [Qwen2 and GPU fit](05-decisions/0012-qwen2-gpu-fit.md) | Scoped metadata and conditional CPU/CUDA memory classes |
| 05 | [Benchmark verification](05-decisions/0013-benchmark-verification.md) | Exact manifest/hardware identity and persistent verification |
| 05 | [Hugging Face catalog](05-decisions/0014-huggingface-catalog.md) | Explicit public catalog access, pinned revisions, rustls, bounded client |
| 05 | [Resumable downloads](05-decisions/0015-resumable-downloads.md) | Private locked store, verified publication, Range resume, explicit start |
| 05 | [Hugging Face token](05-decisions/0016-huggingface-token.md) | Keyring-only token, encrypted transport, first-hop-only use |
| 05 | [Open questions](05-decisions/open-questions.md) | Specification gaps to resolve before affected implementation |

Also read [contributing](../CONTRIBUTING.md), [security](../SECURITY.md),
[conduct](../CODE_OF_CONDUCT.md), [support](../SUPPORT.md), and [changelog](../CHANGELOG.md).

The master specification retains its original filename and target text at its new
location, with dated implementation notes linking validated decisions. The original
import is preserved in Git history. Do not keep a second editable copy at the
repository root. Update this index when adding or moving documents; use relative
links within repository files.
