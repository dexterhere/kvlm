# Shared fixtures

Store only small, deterministic, sanitized inputs shared by multiple crates here:
hardware snapshots, catalog metadata, or recorded HTTP response bodies.
Keep fixtures that belong to one crate in that crate's `tests/fixtures/` directory.

Do not commit model weights, runtime binaries, API tokens, user prompts, machine
identifiers, private paths, or benchmark databases. Prefer constructed examples
over unreviewed captures. Hardware fixtures currently live under
`crates/workstation-core/tests/fixtures/`; this shared directory remains empty
until fixtures are needed by multiple crates.
