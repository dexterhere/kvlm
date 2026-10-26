// SPDX-License-Identifier: GPL-3.0-or-later
//! Remembered SHA-256 digests of local files, as hints that skip re-hashing.
//! A digest is trusted only while the file's device, inode, size, modification time
//! and change time are all unchanged. Call from a blocking worker.

use super::queries::HistoryStore;
use rusqlite::{OptionalExtension, params};

const MAX_ROWS: i64 = 20_000;

/// What identifies one state of a file without reading its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub dev: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i128,
    pub ctime_ns: i128,
}

fn signed(value: u64) -> i64 {
    // SQLite integers are signed; the bit pattern round-trips through `unsigned`.
    value as i64
}

fn unsigned(value: i64) -> u64 {
    value as u64
}

fn nanos(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl HistoryStore {
    /// The remembered digest of `path`, only if `stamp` matches what was recorded.
    pub fn cached_file_digest(&self, path: &str, stamp: &FileStamp) -> Option<String> {
        let row: Option<(i64, i64, i64, i64, i64, String)> = self
            .connection
            .query_row(
                "SELECT dev,inode,size,mtime_ns,ctime_ns,sha256 FROM local_file_digests WHERE path=?1",
                [path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (dev, inode, size, mtime, ctime, sha) = row?;
        (unsigned(dev) == stamp.dev
            && unsigned(inode) == stamp.inode
            && unsigned(size) == stamp.size
            && mtime == nanos(stamp.mtime_ns)
            && ctime == nanos(stamp.ctime_ns))
        .then_some(sha)
    }

    /// Remember a digest computed for the file state `stamp`.
    pub fn cache_file_digest(
        &self,
        path: &str,
        stamp: &FileStamp,
        sha256: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        if path.is_empty() || path.len() > 4096 || sha256.len() != 64 {
            return Err("Digest cache entry is invalid.".into());
        }
        let now = i64::try_from(now_ms).map_err(|_| "Digest timestamp is out of range.")?;
        self.write(|c| {
            c.execute(
                "INSERT INTO local_file_digests(path,dev,inode,size,mtime_ns,ctime_ns,sha256,verified_at_unix_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(path) DO UPDATE SET dev=excluded.dev, inode=excluded.inode, size=excluded.size,
                   mtime_ns=excluded.mtime_ns, ctime_ns=excluded.ctime_ns, sha256=excluded.sha256,
                   verified_at_unix_ms=excluded.verified_at_unix_ms",
                params![
                    path,
                    signed(stamp.dev),
                    signed(stamp.inode),
                    signed(stamp.size),
                    nanos(stamp.mtime_ns),
                    nanos(stamp.ctime_ns),
                    sha256,
                    now
                ],
            )
            .map_err(|_| "Cannot save the file digest.")?;
            c.execute(
                "DELETE FROM local_file_digests WHERE rowid IN (
                   SELECT rowid FROM local_file_digests ORDER BY verified_at_unix_ms ASC, rowid ASC
                   LIMIT max(0, (SELECT count(*) FROM local_file_digests) - ?1))",
                [MAX_ROWS],
            )
            .map_err(|_| "Cannot trim the digest cache.")?;
            Ok(())
        })
    }
}
