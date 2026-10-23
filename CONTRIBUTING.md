# Contributing

Thank you for helping build a useful local AI tool for the Linux community.
Documentation fixes, reproducible bug reports, accessibility improvements, hardware
testing, and code contributions are all welcome.

## Before starting

1. Read the [Code of Conduct](CODE_OF_CONDUCT.md).
2. Follow the [development guide](docs/03-development/getting-started.md).
3. Check the [roadmap](docs/04-roadmap/milestones.md) and existing issues or pull requests
   on the project's eventual hosting service to avoid duplicating work.
4. Discuss large features, dependency additions, and architecture changes first.

Work on the current M0 foundation before taking on later runtime integrations.
Do not label an unimplemented feature as working or present sample telemetry as real.
No paid service, cloud account, GPU, or model download is needed to contribute to
the foundation or run unit tests.

## Code and tests

- Use Rust 2024 and the existing four-crate boundaries. Add dependencies only when needed.
- Keep GTK mutations on the GLib main thread. Send typed commands/events to Tokio;
  move synchronous database, NVML, and filesystem probing to `spawn_blocking`.
- Respect attached-runtime ownership. Never stop external processes or modify system units.
- Cover nontrivial behavior with focused tests. Use mock HTTP servers for routine CI;
  keep live runtime and GPU tests explicit and opt-in.
- Preserve validation, accessibility, cancellation, and error handling.
- Include relevant user-facing documentation when behavior changes.

Run the checks listed in [getting started](docs/03-development/getting-started.md)
before submitting. Explain anything you could not test. See
[tests/README.md](tests/README.md) for fixture and hardware-test guidance.

## Commits and pull requests

Follow the [Git workflow](docs/03-development/git-workflow.md): small, frequent,
meaningful commits with imperative subjects such as `fix: reject attached runtime stops`.
Commit only work you intend to include. Never commit credentials, model weights,
local databases, or generated build output.

Do not add `Co-authored-by` trailers or generated assistant attribution. Each commit
uses its actual committer's configured author identity; this rule does not permit
removing existing copyright notices or misrepresenting authorship.

Describe the problem, resulting behavior, validation, and material limitations in
the pull request. Screenshots help with visual changes; never include private prompts,
tokens, hostnames, or user paths in reports without reviewing them.

## Community and licensing

Assume good intent, give specific feedback, and help new contributors reproduce
problems. A contribution does not guarantee acceptance; maintainers review fit,
correctness, safety, and maintenance cost.

By contributing, you offer your changes under GPL-3.0-or-later. No copyright
assignment or CLA is required. Keep third-party license notices and identify the
origin of reused material.
