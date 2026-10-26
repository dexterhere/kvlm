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
| | `models/gguf.rs` | New `inspect_prefix(bytes, declared_len)` entry point (see Artifact details) |
| | `db/` | Ordered migration steps (0→1, 1→2) and queries for catalog cache, digests, downloads |
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
| `reqwest` features `rustls-tls` (webpki roots) and `json` | runtimes | HTTPS to Hugging Face; no system OpenSSL. Bodies stream with `Response::chunk()`, so no `stream` feature or `futures-util` |
| `rustix` (feature `fs` only) | runtimes | Safe `statvfs` for the free-space reserve; `std` has no such query and `unsafe` is forbidden |
| `oo7` | runtimes | Secret Service; tokio runtime and a pure-Rust crypto backend, chosen in the Phase 5 spike. No `zeroize` crate: the token wrapper overwrites its buffer on drop on a best-effort basis and the docs say so |
| `sha2` | already in core | digests |

Each new crate needs a license check against GPL-3.0-or-later compatibility before the
PR, recorded in the decision record. No other dependency is added.

## Data model

`schema.rs` today creates the v1 table for a blank database and refuses any other
version, so raising `VERSION` alone would break existing databases or skip the new
tables. The migration becomes an ordered step list run inside one `IMMEDIATE`
transaction: `application_id` is checked first (unrelated databases still refused);
version 0 (blank file) runs step 0→1, which creates the v1 history table exactly as today, then 1→2, which creates the five v2 tables; version 1 runs 1→2; version 2 is a no-op; any higher
version is refused with the existing "Unsupported" message and the file is kept. Each
step either completes or the transaction rolls back. After migrating, the expected shape
of every v1 and v2 table is checked, not only `benchmark_runs`. Existing run rows are
untouched. All tables are `STRICT`.

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
  path TEXT PRIMARY KEY, dev INTEGER NOT NULL, inode INTEGER NOT NULL, size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL, ctime_ns INTEGER NOT NULL, sha256 TEXT NOT NULL, verified_at_unix_ms INTEGER NOT NULL
);
downloads(
  id INTEGER PRIMARY KEY, artifact_id TEXT NOT NULL,       -- no foreign key: cache eviction must not touch downloads
  repo_id TEXT NOT NULL, revision TEXT NOT NULL, filename TEXT NOT NULL,
  expected_sha256 TEXT NOT NULL, expected_size INTEGER NOT NULL,   -- copied from the catalog at start
  state TEXT NOT NULL CHECK(state IN ('queued','running','paused','verifying','completed','failed','cancelled')),
  bytes_done INTEGER NOT NULL, failure_code TEXT,
  created_at_unix_ms INTEGER NOT NULL, updated_at_unix_ms INTEGER NOT NULL
);
```

Rules stated as invariants:

- A `catalog_artifacts` row is keyed by revision; an artifact at one revision is never
  rewritten with different size or checksum. A new revision is a new row.
- The spec's `catalog_artifacts` draft columns for architecture facts (`block_count`,
  `head_count_kv`, and so on) live only in `artifact_metadata`, because they come from a
  separate, user-triggered inspection and may be absent.
- `local_file_digests` is a cache only. A row is used only if device, inode, size, mtime,
  and ctime all match the current `stat` (ctime cannot be reset by users, so a restored
  mtime does not hide an edit); otherwise the file is rehashed. Cached results are shown
  as "verified at <time>".
- The catalog cache is bounded: at most 200 `catalog_queries` rows and 16 MiB of cached
  JSON in total, evicted oldest `fetched_at` first. `catalog_artifacts` rows not
  referenced by a cached query are evicted the same way. `downloads` keeps its own copy
  of what it needs and is never evicted.
- `downloads` rows never store tokens, URLs, or remote filenames as paths.

## Hugging Face client

- Base URL `https://huggingface.co`, fixed. A test-only constructor accepts a loopback
  `http://127.0.0.1:port` base; production code rejects any non-HTTPS base.
- Search lists repositories only: `GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30&search=…`;
  next page from the response `Link` header cursor, followed only if its origin equals
  the configured base. Parameters go through the query builder. Search rows do not
  carry siblings, sizes, or checksums, so they are not shown as artifacts.
- Detail is fetched per repository on an explicit expand. The first call (`GET /api/models/{repo}?blobs=true`) learns the commit `sha`; every later call, including inspect and download, uses `GET /api/models/{repo}/revision/{sha}?blobs=true` and requires the returned `sha` to equal the pinned one, so size, checksum, inspection, and download refer to one immutable tuple. The detail response carries siblings with size and LFS SHA-256, license, gating, base model,
  and the top-level `gguf` summary. At most one request is in flight; details are cached
  so re-expanding is free. This avoids 30 detail calls per page and rate limits.
- Destinations: only `https://huggingface.co:443` receives the token. API calls follow no
  redirects. Download redirects are followed to `https` URLs only, without embedded
  credentials, a non-default port only for the configured test base, and never to a
  literal IP address, `localhost`, or a private/link-local/loopback range after
  resolution; the allowed CDN suffixes (`.hf.co`, `.huggingface.co`) are checked on the
  final host. Pagination rebuilds the request from a validated cursor against the fixed
  API origin instead of following a URL from the response.
- Signed CDN URLs are bearer capabilities: automatic `Referer` is disabled, errors are
  mapped to typed failures with the URL stripped (`Error::without_url`), raw response
  bodies never appear in user-facing text, and tests assert a signed URL cannot appear
  in a displayed or logged error.
- Limits: 10 s connect timeout, 30 s per-read idle timeout, at most 5 redirects,
  HTTPS-only redirects, search and detail bodies capped at 4 MiB, and an error body
  read only up to 4 KiB. Every endpoint, including inspection and token validation, has a streamed byte cap and an overall deadline (search/detail 30 s, token check 15 s, inspection 120 s); a `Range` header is not trusted to bound the body, and a `200` to a ranged inspection request is read only up to the cap and then dropped. `429` honors `Retry-After` up to a displayed maximum of one
  hour; no automatic retry loop.
- `Authorization` is set with `HeaderValue::set_sensitive(true)` only on requests to the
  configured base origin (`huggingface.co` in production). reqwest drops it on cross-host
  redirects. A custom redirect policy allows HTTPS only; the loopback test base uses a
  variant that also permits `http` to loopback. Tests check stripping with
  `127.0.0.1` versus `localhost` as the two hosts.

### Normalization

- `RepoId` matches `^[A-Za-z0-9][A-Za-z0-9._-]{0,95}/[A-Za-z0-9][A-Za-z0-9._-]{0,95}$`.
  `Revision` is 40 lowercase hex. Anything else drops the entry with a counted warning.
- Family: `cardData.base_model` when present, otherwise the repo name with GGUF and
  quantization suffixes removed. A repo with neither becomes family "Ungrouped".
  Derivation source is stored so the UI can state it was derived.
- Filename (`rfilename`) may contain `/` for repositories with folders. It is split on
  `/` and each segment percent-encoded; empty, `.`, `..`, backslash, NUL, control
  characters, `:` and names over 255 bytes make the entry invalid and it is dropped.
  The artifact id is `hf:<repo>@<revision>:<filename>`; banning `:` keeps it unambiguous.
- Release: repo + revision. Artifact: one `.gguf` sibling. Quantization comes from the
  filename token (`Q4_K_M`, `IQ3_XS`, `F16`, …); unrecognized becomes Unknown.
- A filename matching `-NNNNN-of-NNNNN.gguf` is treated as a shard; the group is one artifact with
  `shard_count`, download disabled, fit Unknown. The filename is only a hint: Phase 2 inspection also reads the split fields, and a file whose header declares splits is blocked from download even if named like a single file. Quantization read from a filename is shown as "from filename" until the header's file type confirms it.
- Non-`.gguf` siblings are counted as "other formats" and not turned into artifacts.
- Files without `lfs.sha256` have no checksum: listed, not downloadable, not
  dedup-matchable.

## Artifact details and fit

- Inspection is a separate explicit command. It issues one ranged request for the first
  16 MiB at the pinned revision and reads the total length from `Content-Range`, which
  must equal the catalog `size_bytes`.
- `inspect_reader` derives the file length by seeking to the end and rejects tensor ranges
  beyond it, so it cannot run on a prefix. M1 adds `inspect_prefix(bytes, declared_len)`
  in core: the same parser, with the declared total as the file length, erroring on any
  read past the prefix. Truncated metadata yields "metadata not available", not a guess.
  The 16 MiB bound reuses the existing `HEADER_LIMIT`, which becomes a public constant.
  Results are cached in `artifact_metadata`.
- Fit calls `assess_gguf_fit(metadata, scenario, target, layout)`; weights come from the
  metadata's tensor bytes, not from the file size. The displayed default scenario is
  8192 context, F16 K/V, microbatch 512, the uniform attention assumption, against the
  current fresh hardware reading. It is conditional. The engine supports only `llama`
  and `qwen2`, so other architectures (for example qwen3, gemma3, phi3) show Unknown;
  the UI states this coverage limit.

## Deduplication

- Roots: `$OLLAMA_MODELS/blobs`, `~/.ollama/models/blobs`, and the system install path `/usr/share/ollama/.ollama/models/blobs` (reported as not scanned when unreadable); `$HF_HUB_CACHE`, else
  `$HF_HOME/hub`, else `$XDG_CACHE_HOME/huggingface/hub`, else `~/.cache/huggingface/hub`
  (the `models--*/blobs/*` entries). The app's own `models/blobs`. Missing or unreadable
  roots are reported as not scanned, not as empty.
- Walk regular files only; do not follow symlinks; canonicalized path must stay under
  its root. Files are opened with `O_NOFOLLOW | O_NONBLOCK` through
  `OpenOptionsExt::custom_flags` (no `unsafe`), then `fstat` must still show a regular
  file with the walk's device, inode, and size. This closes the swap-to-symlink or FIFO
  race, as `inspect_file` already does. Candidates: size equals the artifact's published size. Only candidates are
  hashed, streaming in 1 MiB reads on `spawn_blocking`, checking a cancel flag between
  reads. A match requires digest equality; a filename claiming a digest is never trusted.
- Work is bounded: one scan at a time (a new request cancels the old one), at most 100 000 entries, 256 GiB of bytes hashed, and 10 minutes per scan, with an explicit partial-scan status. Cached digests are hints; the post-hash `fstat` must match the pre-hash `fstat` (device, inode, size, mtime, ctime) and the pathname must still resolve to the same inode, or the result is discarded.
- Output is a list of `LocalMatch{artifact_id, path, root}`. Nothing is copied, linked,
  or removed.

## Downloader

- Storage: `$XDG_DATA_HOME/local-ai-workstation/models/`, mode `0700`; partial files in
  `partial/<sha256>.part` and final files `blobs/sha256-<hex>`, mode `0600`. File names
  derive only from the digest. Ownership is enforced, not assumed: the application's
  directories must be owned by the current user, not symlinks, and mode `0700`; every
  open uses `O_NOFOLLOW` and then `fstat` must show a regular file with a link count of
  1 owned by the user (a hard link to an external blob is refused); a final file is
  published with `link`-then-`unlink` or `renameat2(NOREPLACE)` semantics so an existing
  `blobs/sha256-<hex>` is never overwritten, and an existing one is re-hashed and reused.
- Preconditions: artifact has a SHA-256, size, valid `RepoId`/`Revision`, and is not a
  shard. A verified local match is shown as information ("already on disk at …"); it
  never blocks a download, because M1 cannot use Ollama blobs directly.
- URL is `resolve/{revision}/{filename}` with the filename percent-encoded as a path
  segment from the catalog value; the filename never reaches the filesystem.
- Single active download per application. One stable lock file `models/.lock`, never unlinked, is held with
  `File::try_lock` for every mutation of a part or final file and its database row (download, discard, rename, recovery), so a second instance can
  neither interleave writes nor discard or rename the part file. A digest is validated as
  exactly 64 lowercase hex characters before it is used in any path.
- Resume: hash the existing part file from the start (one pass), then request
  `Range: bytes=<offset>-`. There is no `If-Range`: the redirect target's ETag differs
  from the origin's, so it would force a restart every time, and the pinned revision plus
  the final SHA-256 already guarantee integrity. Accept `206` only if `Content-Range`
  starts at the offset and the total equals the expected size. A `200` restarts from
  zero. Any other mismatch fails the download.
- A part file whose length equals the expected size skips the network entirely and goes straight to verification; one longer than expected is rejected before hashing. This avoids a `416` loop after a crash that received every byte.
- Never write past the expected size; a longer stream fails the download.
- Completion: flush, fsync, final digest compared to the expected
  value, then rename into `blobs/` and fsync the directory. A mismatch deletes the part
  file and records `integrity`.
- Free space: before and during writing, `statvfs` on the models directory must show at
  least a 2 GiB reserve above the bytes still needed; below it the download pauses with
  a clear message instead of filling the filesystem the history database shares. Total
  retained partial files are capped at 64 GiB; starting another needs the user to
  discard one.
- Recovery after a crash, reconciled locally at startup with no network activity and no
  automatic resume: `running`/`verifying` rows become `paused`; a final `blobs/` file with
  no `completed` row is re-hashed and then recorded `completed` (or deleted if wrong); a
  `paused` row whose part file is missing becomes `failed(missing_partial)` unless a
  verified final file exists. The row is updated only after the filesystem step is
  durable.
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
  with the reason shown. There is no file or environment-variable fallback. Every call
  runs off the UI thread with a 5 s timeout, because D-Bus activation on a desktop
  without a keyring can hang and a locked collection can wait on a prompt nobody answers.
  Availability is checked lazily when preferences open, never at startup. `oo7`'s
  Flatpak portal file backend is not enabled in M1; sandboxed packaging decides this in
  M4. The fake store tests the timeout path. Keyring work runs on its own task so a hung call cannot block download control; dismissed prompts and service disconnects become typed errors with an explicit retry, and "stored" or "cleared" is reported only after the store confirms.
- Token validation (`GET /api/whoami-v2`) runs only on an explicit "Validate" action, never as a side effect of storing.
- Gated artifacts: `gated` is shown from the API. A `401`/`403` from search detail,
  inspect, or download becomes `AccessRequired`; the message explains accepting the
  license on the website and adding a token. The app never opens the license flow
  or accepts terms for the user.

## Bridge

Superseded work is cancelled, not only hidden: a new search cancels the previous in-flight request, a new scan cancels the old one, and at most one inspection runs at a time. Jobs have a bounded queue; excess commands are rejected with a busy message.

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
| Hard link or symlink planted at a part or final path | Refused by `O_NOFOLLOW` and the link-count and owner check; nothing external is truncated |
| Crash after rename before the row update | Startup reconciliation re-hashes the final file and records it |
| Part file already complete, `416` from the server | Verified locally, no request |
| Signed CDN URL in an error | Stripped; typed error only |
| Redirect to an internal host or IP | Refused |
| Disk full | Write error preserved as retryable failure; part kept |
| Cancel | State is `cancelled` but the part file stays until the user discards it; discard needs the lock |
| Gated model, no token | `AccessRequired` with guidance |
| Database v2 opened by an older binary | Refused with the existing message |

## Test matrix

| Area | Test | Level |
| --- | --- | --- |
| Normalization | valid, invalid repo/revision, shard, non-GGUF, missing sha, family fallback | core unit |
| Quantization parser | table of filenames | core unit |
| Migration | blank → v2 (all tables present); damaged v2 refused; concurrent migration is serialized by the immediate transaction; DDL failure rolls back; blank → v2; v1 → v2 keeps runs; v2 no-op; v3 refused; unrelated refused; failure mid-step rolls back | core integration |
| DB queries | cache upsert, immutability by revision, size caps | core integration |
| HF client | mock server: pagination, 429, 401/403, oversize, malformed, timeout, redirect to CDN drops token, HTTP base refused | runtimes integration |
| Inspect | `inspect_prefix` on a prefix of a real-shaped file; truncated header gives Unknown; supported arch gives conditional fit; unsupported arch gives Unknown | core + runtimes |
| Dedup | size prefilter; digest cache hit/invalidate (including restored mtime); symlink or FIFO swap; symlink escape; unreadable root; cancel; filename-claims-digest ignored | core integration |
| Downloader | fresh, resume 206, server 200, bad Content-Range, over-long stream, digest mismatch deletes part, kill-and-resume, lock contention, ENOSPC, subfolder filename, hostile filename | runtimes integration |
| Secrets | fake store round trip; redaction in Debug; unavailable path; hang hits the 5 s timeout | runtimes unit |
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
| 0 | `chore/m1-plan` | these documents, README index, decision records | Opus (decisions) | Codex, Opus |
| 1a | `feat/m1-catalog-model` | core types, normalization, migration steps, cache queries, tests | Sonnet; Haiku (fixtures) | Codex |
| 1b | `feat/m1-hf-client` | rustls dependency, HF client, mock server, limits, redirect/token tests | Sonnet; Haiku (mock server scaffolding) | Codex |
| 1c | `feat/m1-catalog-explorer` | bridge commands, three-tier view, states, smoke test | Sonnet; Haiku (smoke scaffolding, docs) | Codex |
| 2 | `feat/m1-artifact-details` | `inspect_prefix`, inspect command, fit adapter, details pane | Sonnet | Codex |
| 3 | `feat/m1-local-dedup` | scanner, digest cache, UI badge | Sonnet (scanner); OpenCode (tests, docs) | Codex |
| 4 | `feat/m1-resumable-downloads` | downloader, lock, state, UI | Sonnet (security-sensitive) | Codex + Opus final read |
| 5 | `feat/m1-gated-access` | secrets, token UI, access states | Sonnet | Codex + Opus final read |
| 6 | `chore/m1-acceptance` | evidence, docs, milestone status | Haiku (draft), Opus (sign-off wording) | Opus |

Model roles: Opus makes architecture decisions and final security reads, and writes
acceptance wording; Sonnet writes security- and correctness-critical code; Haiku does
mechanical work with a bounded brief (fixtures, boilerplate, doc tables, formatting
fixes). Delegate output is a proposal until the diff and tests are read. Each phase
ends with fmt, clippy with warnings denied, the workspace tests, the repository
validator, and a green CI run before merge.
