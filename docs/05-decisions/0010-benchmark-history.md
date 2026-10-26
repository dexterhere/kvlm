# 0010: Local benchmark history

Date: 2026-10-25. Status: implemented on a dependent feature branch; integration pending.

## Decision

Keep storage in `workstation-core::db` and orchestration in
`workstation-bench::history`. The GTK bridge invokes all database and hardware
work through `spawn_blocking`. A required initial snapshot is committed before
inference; a failed initial write prevents inference. Completion, failure,
cancellation, and deadline outcomes finalize that record once. An identical retry
is safe; a different replacement is rejected. A failed final write preserves the
in-memory outcome and produces separate save feedback.

Each run stores the selected model name, reported manifest digest and size, a fresh
hardware snapshot with its probe issues, the attached loopback endpoint, and fixed
requested settings plus a SHA-256 prompt identifier. Successful completion adds the
observed runtime version, guarded manifest digest, client durations, response chunk
count, and optional server counters. Missing observations stay absent; values are
not invented for failed preflight. No prompt, generated response, credential, or
preflight response body is stored. Counters and nanosecond durations remain JSON
integers, including the full unsigned range, rather than floating-point rates.
Hardware and model snapshots remain local and can contain device names/identifiers.

Use `$XDG_DATA_HOME/local-ai-workstation/workstation.db`, falling back to
`$HOME/.local/share/local-ai-workstation/workstation.db`. Relative XDG values are
ignored; no working-directory fallback exists. Newly created directories use 0700
and database files 0600. Existing directory permissions are preserved. The final
directory and database must be real directories/files, not symlinks. The directory
must not be writable by group/others; existing database and companion files with
group/other access are rejected, preserving their bytes and permissions for recovery. SQLite opens
with NOFOLLOW, uses WAL, enables foreign keys, and waits at most one second for
locks. WAL requires a local writable filesystem.

Schema 1 uses application ID `0x4b564c4d` and one STRICT `benchmark_runs` table.
An immediate transaction creates the initial schema only in an empty, unversioned
database. Unknown identity/version, unrelated tables, incompatible columns, and
corruption are errors, never permission to recreate or downgrade a file. Ownership
and version checks precede changing journal mode. Snapshot and completion JSON
are individually bounded to 256 KiB; all SQL values use bound parameters.
Future schema changes require an explicit, data-preserving migration. Keep copies
of the database and any `-wal`/`-shm` companions together when recovering data;
do not copy only an active WAL database file.

Load at most the newest 100 rows in insertion order; clocks can move backwards.
The UI automatically loads history on startup, coalesces reloads after successful
saves, rejects stale replies, and offers manual Reload. Failed reloads leave the
previous snapshot visibly stale. Runs left without a terminal outcome display
“Unfinished (running or interrupted)”: another application instance may still be
running them. Startup never rewrites those rows.

## Limits

Persistence proves that the run record survives reopening storage. It does not
prove artifact-file identity, hardware identity, effective runtime settings,
resource peaks, or calibrated fit. No Benchmark Verified badge is granted.
Retention, deletion, exports, search, repeated-run aggregation, resource sampling,
and the specification's other tables are deferred until their owning features.

## Sources

- [SQLite WAL](https://www.sqlite.org/wal.html)
- [SQLite pragmas](https://www.sqlite.org/pragma.html)
- [rusqlite 0.41](https://docs.rs/rusqlite/0.41.0/rusqlite/)
- [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/latest/)
