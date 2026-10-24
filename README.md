# GNOME Local AI Workstation

A community-built GNOME desktop application for understanding what local AI models
your hardware can run, managing inference runtimes safely, and measuring performance.

**Status: development foundation.** The workspace and four-view application shell
are in place, plus a supplied-metadata static memory-fit calculator and bounded
local GGUF metadata inspection in core. Hardware
and attached-Ollama discovery are separate, unmerged PRs. Desktop fit badges, inference,
downloads, database storage, and benchmarks are not implemented on this branch yet.
This is an independent community project;
it is not an official GNOME application or a GNOME Circle member.

## Goals

- Explain model fit across dedicated VRAM, shared GPU memory, and system RAM.
- Attach safely to existing runtimes or supervise application-owned processes.
- Compare reproducible benchmarks and explain performance bottlenecks.
- Export endpoint configurations for developer tools.

The project is a local inference control plane, not a chatbot, custom inference
engine, or privileged system daemon. No usage analytics or crash-report uploads.

## Get started

Use Linux, Rust stable (1.92+), GTK 4.12+, Libadwaita 1.4+,
a C toolchain, and pkg-config. See the
[development guide](docs/03-development/getting-started.md) for system dependencies.

```sh
cargo run --locked -p workstation-app
```

The shell does not require Ollama, a GPU, or model downloads. It makes no network
requests at startup. Cargo may download build dependencies.

Exercise the core calculator without a GPU, model, or runtime:

```sh
cargo run --locked -p workstation-core --example memory_fit
```

The example clearly labels synthetic inputs. The API estimates weights, separate K/V
cache storage, buffers, and runtime allowance, then classifies one selected CPU/GPU
placement. Missing metadata or unverified compatibility remains Unknown; shared memory
is never added to host RAM. This does not grant benchmark verification. See the
[calculator guide](docs/03-development/static-memory-fit.md) for scope and checks.

For local GGUF inspection, see the [metadata guide](docs/03-development/gguf-metadata.md).
This reports declarations and tensor layout; runtime compatibility remains unverified.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/check_repository.py
```

## Repository

Source repository: [dexterhere/kvlm](https://github.com/dexterhere/kvlm).

| Path | Responsibility |
| --- | --- |
| `crates/workstation-core/` | Domain types, hardware probes, fit, models, SQLite, exports |
| `crates/workstation-runtimes/` | Runtime adapters and owned-process supervision |
| `crates/workstation-bench/` | Benchmark execution, workloads, resource sampling |
| `crates/workstation-app/` | GTK4/Libadwaita shell and Tokio/GLib bridge |
| `data/` | Desktop launcher, AppStream metadata, settings schema, icons |
| `docs/` | Ordered product, architecture, development, roadmap, decision documents |
| `tests/` | Shared fixture guidance; executable Rust tests live with their owning crate |
| `scripts/` | Repository validation |

Start with the [documentation index](docs/README.md), then read the
[master specification](docs/01-product/gnome_local_ai_workstation_master_specification.md).
The specification describes the target product, not a list of completed features.

## Contribute

Contributions to code, documentation, accessibility, testing, and hardware support
are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), the
[Code of Conduct](CODE_OF_CONDUCT.md), and the
[milestone roadmap](docs/04-roadmap/milestones.md). Small, focused pull requests are
easier to review. Discuss changes to scope or safety boundaries before implementing them.

See [SUPPORT.md](SUPPORT.md) for questions and [SECURITY.md](SECURITY.md) for
vulnerability reporting. Private reporting and a verified maintainer contact still
need to be configured before accepting sensitive reports.

## License

GPL-3.0-or-later. See [COPYING](COPYING) for the full license. Contributions are
accepted under the same license, with no copyright assignment or contributor
license agreement. Third-party models and runtime binaries retain their own licenses.
The AppStream metadata file, including contributions to that file, is CC0-1.0 so
software catalogs can redistribute its descriptions; see [the CC0 text](LICENSES/CC0-1.0.txt).
