# Repository layout

```text
.
├── Cargo.toml / Cargo.lock / rust-toolchain.toml
├── meson.build
├── README.md / CONTRIBUTING.md / SECURITY.md / CODE_OF_CONDUCT.md
├── SUPPORT.md / CHANGELOG.md / COPYING / LICENSE / AGENTS.md
├── .github/                 # CI and community contribution templates
├── crates/
│   ├── workstation-core/src/
│   │   ├── hardware/       # CPU, NVML, DRM sysfs, Vulkan
│   │   ├── fit/            # Memory math, llmfit bridge, advisor
│   │   ├── models/         # Family, release, artifact, catalog, download, dedup
│   │   ├── db/             # Schema, queries, migrations
│   │   └── export/         # Continue, Zed, OpenCode, Neovim, systemd, shell
│   ├── workstation-runtimes/src/
│   │   └── traits.rs / supervisor.rs / ollama.rs / llamacpp.rs / lmstudio.rs / vllm.rs
│   ├── workstation-bench/src/
│   │   └── runner.rs / workloads.rs / sampler.rs
│   └── workstation-app/src/
│       └── main.rs / application.rs / window.rs / bridge.rs / views/
├── data/                    # Desktop, AppStream, GSettings, scalable icon
├── docs/
│   ├── 01-product/          # Original master specification
│   ├── 02-architecture/    # Architecture and safety boundaries
│   ├── 03-development/     # Setup, layout, Git workflow
│   ├── 04-roadmap/         # Status and milestone order
│   └── 05-decisions/       # Accepted decisions and unresolved questions
├── scripts/                 # Small repository checks
├── site/                    # Static project site, no build step
└── tests/                   # Python repository checks and fixtures
```

`site/` holds the one-page project site deployed to GitHub Pages by
`.github/workflows/pages.yml`. Everything is self-contained: no JavaScript, external fonts,
or CDN. Assets use relative URLs because the site lives under `/kvlm/`. `tests/test_site.py`
validates the page metadata, local links, asset copies, screenshot size, and the version
badge against `Cargo.toml`. Update `docs/assets/logo.svg` first, then copy it to
`site/assets/logo.svg`; the test compares the files byte for byte.

Each source directory is registered as a Rust module. Planned leaf modules contain
documentation only, not stub functions that panic or return made-up results.
Add dependencies as a module gains real behavior. SQLite migration SQL belongs
under `crates/workstation-core/src/db/migrations/` when the first migration exists.

Executable integration tests belong in the owning crate's `tests/` directory.
The workspace root is a virtual Cargo manifest; root `tests/*.rs` would not run.
This intentionally corrects the illustrative test placement in the master specification.
Use root `tests/fixtures/` only for genuinely shared sanitized data.

Never store downloaded models, runtime binaries, credentials, benchmark databases,
or build artifacts in the source tree.
