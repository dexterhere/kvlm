# 0014: Hugging Face catalog access

Date: 2026-10-26. Status: Phases 1a and 1b of the [M1 plan](../04-roadmap/m1/Implementation.md)
implemented; the browser view, details, deduplication, downloads, and tokens follow.

## Decision

The application reads the public Hugging Face Hub API to list GGUF repositories and to
fetch one repository's files. It makes a request only after an explicit user action and
never at startup. Results are cached in the local SQLite database so previously fetched
pages stay browsable offline with their age shown.

Search lists repositories only, because search rows carry no file sizes or checksums.
A repository's files load when the user expands it. The first detail request learns the
commit; every later request, including inspection and download, is pinned to that
commit and must return the same commit id. Size, checksum, and metadata therefore refer
to one immutable tuple.

All remote identifiers are untrusted. Repository ids, commit ids, file names, and
checksums are validated before they reach a URL or a file path; hostile or duplicate
entries are dropped and counted. Missing facts stay unknown: a file without a published
checksum is listed but not downloadable, and a shard group has no single checksum.

## Network client

- `workstation-runtimes` owns HTTP; `workstation-core` stays free of network and async
  code.
- TLS is rustls with the bundled Mozilla root set, so no system OpenSSL is required.
  New dependencies and licenses: rustls, rustls-webpki, rustls-pki-types, tokio-rustls,
  hyper-rustls, ring (Apache-2.0 AND ISC), untrusted (ISC), subtle (BSD-3-Clause),
  zeroize, and webpki-roots (CDLA-Permissive-2.0 data). All are permissive and
  compatible with GPL-3.0-or-later.
- API calls follow no redirects, send no referrer, cookies, or credentials, cap every
  body while streaming at 4 MiB, and share an overall 30 second deadline. One request is
  in flight at a time. A `429` reports `Retry-After` and is never retried automatically.
- Errors are typed and carry no URLs or response text.
- Pagination rebuilds the next request from a validated cursor against the fixed API
  origin; it never follows a URL taken from a response.
- Tokens, signed download URLs, and CDN redirects arrive with the download and gated
  access phases and are covered by their own tests.

## Consequences

- The database schema moves to version 2. The migration is ordered, atomic, and checks
  exact table shapes; older application versions refuse the new file and keep it intact.
- Cached releases are validated on write and read, and a revision's stored size or
  checksum can never change or disappear.
- Family names are derived from a declared base model or the repository name and are
  labelled as derived. Quantization read from a file name is a hint until the GGUF
  header confirms it.
