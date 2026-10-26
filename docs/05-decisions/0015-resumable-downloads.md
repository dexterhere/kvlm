# 0015: Resumable, verified model downloads

Date: 2026-10-26. Status: Phase 4 of the [M1 plan](../04-roadmap/m1/Implementation.md)
implemented. Tokens and gated access follow in Phase 5.

## Decision

The application can download one catalog file at a time into its own model store. A
transfer starts only from an explicit click on a file that has a published size and a
SHA-256, and is not part of a split model. Nothing resumes by itself, including after a
crash or restart: saved downloads are listed from local state and wait for the user.

The pinned commit and the published SHA-256 are the integrity guarantee. Bytes are
streamed to a partial file named after the expected digest, hashed as they arrive, and
renamed into `blobs/sha256-<hex>` only when the digest matches. A mismatch deletes the
partial file and publishes nothing.

## Store layout and safety

`$XDG_DATA_HOME/local-ai-workstation/models/` is created with mode 0700, as are
`partial/` and `blobs/`. Partial files and blobs are 0600. Every directory is checked to
be a real directory owned by the user, not a symbolic link. A `.lock` file held with
`flock` allows one application instance to write; a second instance reports that the
store is in use and changes nothing.

Publication fsyncs the file, renames it without replacing an existing name, and fsyncs
the directory. A good blob that already exists is reused; a bad one is replaced.

`rustix` (Apache-2.0 OR MIT) is added to `workstation-core` for `renameat_with` with
`NOREPLACE`, because the standard library cannot rename without replacing. The
workspace still forbids `unsafe_code`.

## Resume and limits

- A resume sends `Range: bytes=<n>-` with no `If-Range`. The commit is already pinned, so
  the content cannot change. A `206` must report exactly `(n, size-1, size)`. A `200` is
  accepted only with a `Content-Length` equal to the published size and restarts from
  zero. Anything else fails and keeps the partial file.
- The partial file is never written past the published size. A longer stream fails with
  `too_large` and the partial file is discarded, because the response cannot be trusted.
- A partial file longer than the published size is refused, not truncated.
- Free space is checked before the transfer and every 64 MiB, keeping a 2 GiB reserve. A
  shortage pauses the download with an explanation; it never fills the disk.
- Partial files may not exceed 64 GiB in total.
- Downloads follow the same redirect rules as the catalog client: https only, Hugging
  Face hosts, port 443, no credentials in the URL, public addresses only. No credentials
  are sent in this phase.

## Bookkeeping

The `downloads` table stores the artifact, the expected digest and size, the state
(`queued`, `running`, `paused`, `verifying`, `completed`, `failed`, `cancelled`), the
bytes written and a failure code. Only one row may be active. At startup, and whenever
the list is loaded while the store is free, rows are reconciled with the files: an
interrupted row becomes `paused`, a row whose verified blob already exists becomes
`completed`, a started download whose partial file vanished becomes `failed`
(`missing_partial`), and nothing touches the network.

## User controls

Pause and Cancel both keep the partial file. Resume continues it. Discard deletes the
partial file and the row and needs a second click. Closing the application pauses a
running transfer when it can; after a crash the next start reconciles the row as above.

## Consequences

- The application's own blobs are a new local-copy root for deduplication, so a finished
  download appears as "already on disk, SHA-256 verified".
- Ollama and Hugging Face cache files are never modified. Importing a blob into a
  runtime is out of scope for M1.
