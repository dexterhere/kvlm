# M1 implementation

Companion documents: [Planned](Planned.md) · [Diagrams](Diagrams.md)

Follows the existing rules in `AGENTS.md`: four crates through M4, `unsafe_code`
forbidden, GTK objects only on the GLib thread, typed `CoreCommand`/`UiEvent` bridge
with correlation ids, blocking work through `spawn_blocking`, tests that need no GPU,
model, runtime, or internet.

## Module boundaries

| Crate | New or changed | Responsibility |
| --- | --- | --- |
| `workstation-core` | `models/{family,release,artifact,catalog}.rs` | Pure types: `ModelFamily`, `ModelRelease`, `ModelArtifact`, `ArtifactFormat`, `Sha256Digest`, `RepoId`, `Revision`, validation and normalization from parsed API data |
| | `models/dedup.rs` | Scan roots, size prefilter, streaming SHA-256, cancellable; digest cache lookups |
| | `models/download.rs` | State machine types and pure resume/verify rules (no network) |
| | `fit/` | Adapter running `assess_gguf_fit` from remote GGUF metadata for a default scenario |
| | `db/` | Migration v1 → v2 and queries for catalog cache, digests, downloads |
| `workstation-runtimes` | `huggingface/` | HTTPS client, response parsing, size and time bounds, token header handling |
| | `download.rs` | Streaming ranged downloader writing partial files |
| | `secrets.rs` | Secret Service wrapper behind a small trait with a fake for tests |
| `workstation-bench` | none | |
| `workstation-app` | `views/catalog.rs` (+ `catalog_smoke.rs`), `bridge.rs`, preferences row | Three-tier browser, details pane, downloads list, token entry |

Network adapters live in `workstation-runtimes` because it already owns `reqwest` and
`tokio`; `workstation-core` stays free of network and async dependencies. Update the
crate description and `docs/02-architecture/README.md` accordingly.

## Dependencies

| Dependency | Where | Reason |
| --- | --- | --- |
| `reqwest` feature `rustls-tls` (webpki roots), `stream`, `json` | runtimes | HTTPS to Hugging Face; no system OpenSSL |
| `futures-util` | runtimes | stream a response body chunk by chunk |
| `oo7` | runtimes | Secret Service; choose its tokio and crypto features during the Phase 5 spike |
| `sha2` | already in core | digests |

Each new crate needs a license check against GPL-3.0-or-later compatibility before the
PR, recorded in the decision record. No other dependency is added.

## Data model

`PRAGMA user_version` moves from 1 to 2 through one atomic `IMMEDIATE` transaction.
The existing refusal of unrelated databases is kept. A database with version 2 is
accepted; the older binary refuses it with the existing "Unsupported" message. Existing
`benchmark_runs` rows are untouched. All tables are `STRICT`.

```sql
catalog_queries(
  query_key TEXT PRIMARY KEY,            -- normalized query + sort + cursor
  result_json TEXT NOT NULL CHECK(length(CAST(result_json AS BLOB)) <= 4194304),
  fetched_at_unix_ms INTEGER NOT NULL
);
catalog_artifacts(
  artifact_id TEXT PRIMARY KEY,          -- 'hf:<repo>@<rev>:<filename>'
  repo_id TEXT NOT NULL, revision TEXT NOT NULL, filename TEXT NOT NULL,
  format TEXT NOT NULL, quantization TEXT, size_bytes INTEGER,
  sha256 TEXT, license TEXT, gated TEXT, shard_count INTEGER,
  fetched_at_unix_ms INTEGER NOT NULL
);
artifact_metadata(
  artifact_id TEXT PRIMARY KEY REFERENCES catalog_artifacts(artifact_id),
  gguf_json TEXT NOT NULL CHECK(length(CAST(gguf_json AS BLOB)) <= 262144),
  fetched_at_unix_ms INTEGER NOT NULL
);
local_file_digests(
  path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
  inode INTEGER NOT NULL, sha256 TEXT NOT NULL, verified_at_unix_ms INTEGER NOT NULL
);
downloads(
  id INTEGER PRIMARY KEY, artifact_id TEXT NOT NULL REFERENCES catalog_artifacts(artifact_id),
  state TEXT NOT NULL CHECK(state IN ('queued','running','paused','verifying','completed','failed','cancelled')),
  bytes_done INTEGER NOT NULL, etag TEXT, failure_code TEXT,
  created_at_unix_ms INTEGER NOT NULL, updated_at_unix_ms INTEGER NOT NULL
);
```

Rules stated as invariants:

- A `catalog_artifacts` row is keyed by revision; an artifact at one revision is never
  rewritten with different size or checksum. A new revision is a new row.
- The spec's `catalog_artifacts` draft columns for architecture facts (`block_count`,
  `head_count_kv`, and so on) live only in `artifact_metadata`, because they come from a
  separate, user-triggered inspection and may be absent.
- `local_file_digests` is a cache only. A row is used only if size, mtime, and inode all
  match the current `stat`; otherwise the file is rehashed.
- `downloads` rows never store tokens, URLs, or remote filenames as paths.

## Hugging Face client

- Base URL `https://huggingface.co`, fixed. A test-only constructor accepts a loopback
  `http://127.0.0.1:port` base; production code rejects any non-HTTPS base.
- Search: `GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30&search=…`;
  next page from the response `Link` header cursor. Parameters go through the query
  builder, never string concatenation.
- Detail: `GET /api/models/{repo}?blobs=true` for siblings with size and LFS SHA-256,
  license, gating, base model, and the top-level `gguf` summary.
- Limits: 10 s connect timeout, 30 s per-read idle timeout, at most 5 redirects,
  HTTPS-only redirects, search and detail bodies capped at 4 MiB, and an error body
  read only up to 4 KiB. `429` honors `Retry-After` up to a displayed maximum of one
  hour; no automatic retry loop.
- `Authorization` is set with `HeaderValue::set_sensitive(true)` only on requests whose
  host is `huggingface.co`. reqwest drops it on cross-host redirects, and the tests
  assert that to CDN hosts it is absent.

### Normalization

- `RepoId` matches `^[A-Za-z0-9][A-Za-z0-9._-]{0,95}/[A-Za-z0-9][A-Za-z0-9._-]{0,95}$`.
  `Revision` is 40 lowercase hex. Anything else drops the entry with a counted warning.
- Family: `cardData.base_model` when present, otherwise the repo name with GGUF and
  quantization suffixes removed. A repo with neither becomes family "Ungrouped".
  Derivation source is stored so the UI can state it was derived.
- Release: repo + revision. Artifact: one `.gguf` sibling. Quantization comes from the
  filename token (`Q4_K_M`, `IQ3_XS`, `F16`, …); unrecognized becomes Unknown.
- A filename matching `-NNNNN-of-NNNNN.gguf` is a shard; the group is one artifact with
  `shard_count`, download disabled, fit Unknown.
- Non-`.gguf` siblings are counted as "other formats" and not turned into artifacts.
- Files without `lfs.sha256` have no checksum: listed, not downloadable, not
  dedup-matchable.

## Artifact details and fit

- Inspection is a separate explicit command. It issues one ranged request for the first
  16 MiB at the pinned revision (the existing `HEADER_LIMIT`) and runs
  `inspect_reader` over the bytes. A truncated header yields "metadata not available",
  not a guess. Results are cached in `artifact_metadata`.
- Fit uses `assess_gguf_fit` with the file size as the explicit weight estimate and a
  displayed default scenario: 8192 context, F16 K/V, microbatch 512, the uniform
  attention assumption, against the current fresh hardware reading. It is shown as
  conditional, only for the architectures the fit engine supports; otherwise Unknown.

## Deduplication

- Roots: `$OLLAMA_MODELS/blobs` or `~/.ollama/models/blobs`; `$HF_HUB_CACHE`, else
  `$HF_HOME/hub`, else `$XDG_CACHE_HOME/huggingface/hub`, else `~/.cache/huggingface/hub`
  (the `models--*/blobs/*` entries). The app's own `models/blobs`. Missing or unreadable
  roots are reported as not scanned, not as empty.
- Walk regular files only; do not follow symlinks; canonicalized path must stay under
  its root. Candidates: size equals the artifact's published size. Only candidates are
  hashed, streaming in 1 MiB reads on `spawn_blocking`, checking a cancel flag between
  reads. A match requires digest equality; a filename claiming a digest is never trusted.
- Output is a list of `LocalMatch{artifact_id, path, root}`. Nothing is copied, linked,
  or removed.

## Downloader

- Storage: `$XDG_DATA_HOME/local-ai-workstation/models/`, mode `0700`; partial files in
  `partial/<sha256>.part` and final files `blobs/sha256-<hex>`, mode `0600`. File names
  derive only from the verified digest.
- Preconditions: artifact has a SHA-256, size, valid `RepoId`/`Revision`, and is not a
  shard. If a verified local match exists the UI offers "already available" instead.
- URL is `resolve/{revision}/{filename}` with the filename percent-encoded as a path
  segment from the catalog value; the filename never reaches the filesystem.
- Single active download per application; the part file is opened and exclusively
  locked with `File::try_lock` so a second instance cannot interleave writes.
- Resume: hash the existing part file from the start (one pass), then request
  `Range: bytes=<offset>-` with `If-Range: <etag>`. Accept `206` only if
  `Content-Range` starts at the offset and the total equals the expected size. A `200`
  restarts from zero. Any other mismatch fails the download.
- Never write past the expected size; a longer stream fails the download.
- Completion: flush, fsync, final digest compared in constant time to the expected
  value, then rename into `blobs/` and fsync the directory. A mismatch deletes the part
  file and records `integrity`.
- Progress events are throttled to at most 4 per second. Cancel keeps the part file;
  the user can discard it explicitly.
- Disk full or write errors keep the part file and report a clear retryable failure.

## Secrets

- A `SecretStore` trait (`get`, `put`, `clear`, `available`) wraps `oo7`; a fake backs
  the tests. Item attributes are `application=org.gnome.LocalAIWorkstation` and
  `service=huggingface`; the token is the secret value only.
- The token type implements `Debug` and `Display` as redacted and zeroizes on drop
  where the dependency allows. It is never logged, put in an error string, an event
  payload, SQLite, or settings.
- If the Secret Service is unavailable, locked, or denied, token features are disabled
  with the reason shown. There is no file or environment-variable fallback.
- Token validation (`GET /api/whoami-v2`) runs only on an explicit "Validate" action.
- Gated artifacts: `gated` is shown from the API. A `401`/`403` from search detail,
  inspect, or download becomes `AccessRequired`; the message explains accepting the
  license on the website and adding a token. The app never opens the license flow
  or accepts terms for the user.

## Bridge

New commands, each carrying `id: u64`: `SearchCatalog`, `InspectArtifact`,
`ScanLocalFiles` (with cancel receiver), `StartDownload`, `ResumeDownload`,
`CancelDownload`, `DiscardPartial`, `StoreToken`, `ClearToken`, `ValidateToken`,
`LoadCachedCatalog`, `LoadDownloads`. Matching `UiEvent` variants return typed
`Result<…, UserFacingError>` values. Progress events replace, never append. Stale ids are
dropped by the view as in the existing fit and benchmark views. Nothing issues a request
on startup; `LoadCachedCatalog` and `LoadDownloads` read SQLite only.

## UI

View 2 is an `AdwNavigationSplitView`: left, search entry and the Family → Release →
Artifact list (`AdwExpanderRow` rows); right, the details pane (size, quantization,
license, revision, checksum, source link text, metadata, fit, "Already on disk",
download controls). States: idle, loading (spinner, cancellable), error with Retry,
offline with cached-age label, empty. A downloads section lists active and finished
transfers. Every control has an accessible label; progress exposes a text value.
Destructive actions (discard partial) need confirmation.

## Edge cases

| Case | Resolution |
| --- | --- |
| Search response is slow, oversized, or malformed | Timeout / 4 MiB cap / parse error becomes a typed error with Retry; cache untouched |
| `429` | Show `Retry-After`; no automatic retry |
| Two searches overlap | Latest id wins; stale events dropped |
| Repo renamed or revision force-moved | Artifact is keyed by revision; old rows stay valid until refreshed |
| File changes on the server between inspect and download | URL pinned to revision; digest checked at the end |
| Server ignores `Range` | `200` response restarts from zero |
| Part file is longer than expected or corrupt | Hash prefix, mismatch at end fails, part discarded |
| Process killed mid-download | `downloads` row is `running`; on restart it is shown `paused` with the part file |
| Two app instances | `try_lock` fails; second shows "download in use by another instance" |
| Hostile `filename` such as `../x` or absolute path | Never used as a path; URL segment is percent-encoded; names for the store derive from the digest |
| Redirect to an HTTP or foreign host | HTTP refused; token not forwarded to other hosts |
| Local file is changed during hashing | Size and mtime re-checked after hashing; if changed the result is discarded |
| Symlink pointing outside a scan root | Not followed |
| Huge directory | Bounded entry count (default 100 000) with a "partial scan" flag |
| Secret Service locked or missing | Feature disabled with reason; public downloads continue |
| Token revoked or expired | `401` becomes `AccessRequired`; no retry with a stale token |
| Disk full | Write error preserved as retryable failure; part kept |
| Gated model, no token | `AccessRequired` with guidance |
| Database v2 opened by an older binary | Refused with the existing message |

## Test matrix

| Area | Test | Level |
| --- | --- | --- |
| Normalization | valid, invalid repo/revision, shard, non-GGUF, missing sha, family fallback | core unit |
| Quantization parser | table of filenames | core unit |
| Migration | v1 → v2 keeps runs; refuse unrelated; refuse v3; idempotent | core integration |
| DB queries | cache upsert, immutability by revision, size caps | core integration |
| HF client | mock server: pagination, 429, 401/403, oversize, malformed, timeout, redirect to CDN drops token, HTTP base refused | runtimes integration |
| Inspect | truncated header gives Unknown; supported arch gives conditional fit | core + runtimes |
| Dedup | size prefilter; digest cache hit/invalidate; symlink escape; unreadable root; cancel; filename-claims-digest ignored | core integration |
| Downloader | fresh, resume 206, server 200, bad Content-Range, over-long stream, digest mismatch deletes part, kill-and-resume, lock contention, ENOSPC | runtimes integration |
| Secrets | fake store round trip; redaction in Debug; unavailable path | runtimes unit |
| Bridge | stale id dropped; progress throttle; cancel | app unit |
| GTK smoke | browser states, details, downloads list, token row (Broadway) | app, ignored by default like M0 |
| Acceptance | real search, real small download interrupted and resumed, real gated refusal, restart | opt-in, recorded |

## Pull requests, branches, and agent assignment

Branch names use `feat/` or `chore/`. Each PR is small enough to review alone, targets
`develop`, uses conventional commits at coherent checkpoints, and carries no
co-author or assistant attribution. Promotion to `staging`/`main` happens once, after
Phase 6.

| PR | Branch | Contents | Implementer | Reviewer |
| --- | --- | --- | --- | --- |
| 0 | `chore/m1-plan` | these documents, README index, milestone note, decision records | Opus (decisions) | Codex (security read) |
| 1 | `feat/m1-catalog-explorer` | core types, HF client, cache + migration, view, states | Sonnet (client, migration, bridge); Haiku (fixtures, mock server, smoke scaffolding, docs) | Codex |
| 2 | `feat/m1-artifact-details` | inspect command, fit adapter, details pane | Sonnet | Codex |
| 3 | `feat/m1-local-dedup` | scanner, digest cache, UI badge | Sonnet (scanner); OpenCode (tests, docs) | Codex |
| 4 | `feat/m1-resumable-downloads` | downloader, state, UI | Sonnet (security-sensitive) | Codex + Opus final read |
| 5 | `feat/m1-gated-access` | secrets, token UI, access states | Sonnet | Codex + Opus final read |
| 6 | `chore/m1-acceptance` | evidence, docs, milestone status | Haiku (draft), Opus (sign-off wording) | Opus |

Model roles: Opus makes architecture decisions and final security reads, and writes
acceptance wording; Sonnet writes security- and correctness-critical code; Haiku does
mechanical work with a bounded brief (fixtures, boilerplate, doc tables, formatting
fixes). Delegate output is a proposal until the diff and tests are read. Each phase
ends with fmt, clippy with warnings denied, the workspace tests, the repository
validator, and a green CI run before merge.
