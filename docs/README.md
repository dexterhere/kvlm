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
| 03 | [Foundation validation](03-development/validation.md) | Checks performed and practical limits |
| 03 | [Static memory fit](03-development/static-memory-fit.md) | Offline example, core API, validation, and remaining integration |
| 03 | [GGUF metadata inspection](03-development/gguf-metadata.md) | Local inspection example, supported subset, and limits |
| 03 | [GGUF fit scenarios](03-development/gguf-fit.md) | Local declarations with explicit settings, assumptions, and unknown compatibility |
| 04 | [Milestones](04-roadmap/milestones.md) | Current state and M0–M4 delivery order |
| 04 | [Next feature: hardware discovery](04-roadmap/next-feature-hardware-discovery.md) | Implementation sequence, scope, and acceptance checks for M0 Step 2 |
| 05 | [Decisions](05-decisions/README.md) | How to record architecture decisions |
| 05 | [Foundation decision](05-decisions/0001-workspace-foundation.md) | Accepted setup choices |
| 05 | [Static memory-fit decision](05-decisions/0004-static-memory-fit.md) | Supported layouts, byte accounting, and placement thresholds |
| 05 | [GGUF metadata decision](05-decisions/0005-gguf-metadata.md) | Bounded declarations and tensor layout without runtime or fit claims |
| 05 | [GGUF fit integration decision](05-decisions/0006-gguf-fit-integration.md) | Metadata gates and scenario-only estimates |
| 05 | [Open questions](05-decisions/open-questions.md) | Specification gaps to resolve before affected implementation |

Also read [contributing](../CONTRIBUTING.md), [security](../SECURITY.md),
[conduct](../CODE_OF_CONDUCT.md), [support](../SUPPORT.md), and [changelog](../CHANGELOG.md).

The master specification retains its original filename and target text at its new
location, with dated implementation notes linking validated decisions. The original
import is preserved in Git history. Do not keep a second editable copy at the
repository root. Update this index when adding or moving documents; use relative
links within repository files.
