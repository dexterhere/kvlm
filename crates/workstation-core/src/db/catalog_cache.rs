// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline cache of catalog pages and releases, stored beside benchmark history.
//! Bounded and evicted oldest first; call from a blocking worker.

use super::queries::HistoryStore;
use crate::models::{
    artifact::valid_remote_filename,
    catalog::{CatalogPage, valid_repo_id, valid_revision, validate_release},
    gguf::GgufMetadata,
    release::ModelRelease,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const MAX_QUERY_ROWS: i64 = 200;
const MAX_QUERY_BYTES: i64 = 16 * 1024 * 1024;
const MAX_RELEASE_ROWS: i64 = 2_000;
const MAX_RELEASE_BYTES: i64 = 64 * 1024 * 1024;
const MAX_METADATA_ROWS: i64 = 4000;
const MAX_METADATA_BYTES: i64 = 64 * 1024 * 1024;
const MAX_KEY: usize = 512;

/// Returned when a revision would be stored with a size or checksum that differs from
/// what is already saved. Callers must reject the new data, not the cache.
pub const REVISION_CONFLICT: &str =
    "Cached revision conflicts with newly fetched data. Keep the cache and retry later.";

/// Stable cache key for a search page: digests of the lowercased trimmed query and of
/// the cursor, so any permitted query and cursor length fits the key limit.
pub fn page_key(query: &str, cursor: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let hex = |text: &str| -> String {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    format!(
        "search:v2:{}:{}",
        hex(&query.trim().to_lowercase()),
        cursor.map_or(String::new(), hex)
    )
}

fn timestamp(ms: u64) -> Result<i64, String> {
    i64::try_from(ms).map_err(|_| "Catalog timestamp exceeds the supported range.".into())
}

impl HistoryStore {
    /// One short writer transaction, so lookup, conflict checks, upsert and eviction
    /// cannot interleave with another application instance.
    pub(super) fn write<T>(
        &self,
        work: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(|_| "Catalog cache is locked. Retry after other writers finish.")?;
        let value = work(&tx)?;
        tx.commit()
            .map_err(|_| "Cannot commit catalog cache. Check free disk space.")?;
        Ok(value)
    }

    /// Cache one search page under a normalized key. Replaces the same key.
    pub fn cache_catalog_page(
        &self,
        key: &str,
        page: &CatalogPage,
        now_ms: u64,
    ) -> Result<(), String> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err("Catalog cache key is invalid.".into());
        }
        let json = serde_json::to_string(page).map_err(|_| "Cannot encode catalog page.")?;
        let at = timestamp(now_ms)?;
        self.write(|c| {
            c.execute(
                "INSERT INTO catalog_queries(query_key,result_json,fetched_at_unix_ms) VALUES (?1,?2,?3)
                 ON CONFLICT(query_key) DO UPDATE SET result_json=excluded.result_json, fetched_at_unix_ms=excluded.fetched_at_unix_ms
                 WHERE excluded.fetched_at_unix_ms >= catalog_queries.fetched_at_unix_ms",
                params![key, json, at],
            )
            .map_err(|_| "Cannot cache catalog page. The page may be too large or the disk full.")?;
            evict(c, "catalog_queries", MAX_QUERY_ROWS, MAX_QUERY_BYTES, "result_json")
        })
    }

    /// A cached page and the time it was fetched, for an offline view labelled with its age.
    pub fn cached_catalog_page(&self, key: &str) -> Result<Option<(CatalogPage, u64)>, String> {
        let row: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT result_json, fetched_at_unix_ms FROM catalog_queries WHERE query_key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "Cannot read cached catalog page.")?;
        row.map(|(json, at)| {
            let page =
                serde_json::from_str(&json).map_err(|_| "Cached catalog page is unreadable.")?;
            Ok((page, u64::try_from(at).unwrap_or(0)))
        })
        .transpose()
    }

    /// Cache a release. A revision is immutable: once an artifact has a stored size or
    /// checksum it must reappear with the same values, and may not disappear.
    pub fn cache_release(&self, release: &ModelRelease, now_ms: u64) -> Result<(), String> {
        validate_release(release).map_err(|_| "Release failed validation and was not cached.")?;
        let json = serde_json::to_string(release).map_err(|_| "Cannot encode release.")?;
        let at = timestamp(now_ms)?;
        self.write(|c| {
            // An unreadable or invalid stored row is damaged cache, not a fact to protect.
            if let Some((existing, _)) = read_release(c, &release.repo_id, &release.revision)? {
                for old in &existing.artifacts {
                    let known = old.size_bytes.is_some() || old.sha256.is_some();
                    let new = release.artifacts.iter().find(|a| a.artifact_id == old.artifact_id);
                    let conflict = match new {
                        None => known,
                        Some(new) => {
                            old.size_bytes.is_some_and(|v| Some(v) != new.size_bytes)
                                || old.sha256.as_ref().is_some_and(|v| Some(v) != new.sha256.as_ref())
                        }
                    };
                    if conflict {
                        return Err(REVISION_CONFLICT.into());
                    }
                }
            }
            c.execute(
                "INSERT INTO catalog_releases(repo_id,revision,release_json,fetched_at_unix_ms) VALUES (?1,?2,?3,?4)
                 ON CONFLICT(repo_id,revision) DO UPDATE SET release_json=excluded.release_json, fetched_at_unix_ms=excluded.fetched_at_unix_ms
                 WHERE excluded.fetched_at_unix_ms >= catalog_releases.fetched_at_unix_ms",
                params![release.repo_id, release.revision, json, at],
            )
            .map_err(|_| "Cannot cache release. It may be too large or the disk full.")?;
            evict(c, "catalog_releases", MAX_RELEASE_ROWS, MAX_RELEASE_BYTES, "release_json")
        })
    }

    /// The most recently cached release of a repository, whatever its revision. Used when
    /// the network is unavailable and no commit has been learned yet.
    pub fn cached_latest_release(
        &self,
        repo_id: &str,
    ) -> Result<Option<(ModelRelease, u64)>, String> {
        let revision: Option<String> = self
            .connection
            .query_row(
                "SELECT revision FROM catalog_releases WHERE repo_id=?1 ORDER BY fetched_at_unix_ms DESC, rowid DESC LIMIT 1",
                [repo_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Cannot read cached release.")?;
        match revision {
            Some(revision) => read_release(&self.connection, repo_id, &revision),
            None => Ok(None),
        }
    }

    /// Cache what a file's header declared. A revision is immutable, so a stored row
    /// stays valid; a differing second parse of the same file is refused.
    pub fn cache_artifact_metadata(
        &self,
        repo_id: &str,
        revision: &str,
        filename: &str,
        metadata: &GgufMetadata,
        now_ms: u64,
    ) -> Result<(), String> {
        if !valid_repo_id(repo_id) || !valid_revision(revision) || !valid_remote_filename(filename)
        {
            return Err("Artifact identity is invalid.".into());
        }
        let json = serde_json::to_string(metadata).map_err(|_| "Cannot encode file metadata.")?;
        let at = timestamp(now_ms)?;
        self.write(|c| {
            // A revision's header never changes: differing facts are an integrity
            // problem with the response, never a replacement.
            let saved: Option<String> = c
                .query_row(
                    "SELECT gguf_json FROM artifact_metadata WHERE repo_id=?1 AND revision=?2 AND filename=?3",
                    params![repo_id, revision, filename],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| "Cannot read cached file metadata.")?;
            if let Some(saved) = saved
                && serde_json::from_str::<GgufMetadata>(&saved).is_ok_and(|old| old != *metadata)
            {
                return Err(REVISION_CONFLICT.into());
            }
            c.execute(
                "INSERT INTO artifact_metadata(repo_id,revision,filename,gguf_json,fetched_at_unix_ms) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(repo_id,revision,filename) DO UPDATE SET gguf_json=excluded.gguf_json, fetched_at_unix_ms=excluded.fetched_at_unix_ms
                 WHERE excluded.fetched_at_unix_ms >= artifact_metadata.fetched_at_unix_ms",
                params![repo_id, revision, filename, json, at],
            )
            .map_err(|_| "Cannot cache file metadata. It may be too large or the disk full.")?;
            evict(c, "artifact_metadata", MAX_METADATA_ROWS, MAX_METADATA_BYTES, "gguf_json")
        })
    }

    /// Saved header facts for a file at a pinned revision; unreadable rows read as absent.
    pub fn cached_artifact_metadata(
        &self,
        repo_id: &str,
        revision: &str,
        filename: &str,
    ) -> Result<Option<(GgufMetadata, u64)>, String> {
        let row: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT gguf_json, fetched_at_unix_ms FROM artifact_metadata WHERE repo_id=?1 AND revision=?2 AND filename=?3",
                params![repo_id, revision, filename],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "Cannot read cached file metadata.")?;
        Ok(row.and_then(|(json, at)| {
            let metadata: GgufMetadata = serde_json::from_str(&json).ok()?;
            metadata
                .plausible()
                .then(|| (metadata, u64::try_from(at).unwrap_or(0)))
        }))
    }

    pub fn cached_release(
        &self,
        repo_id: &str,
        revision: &str,
    ) -> Result<Option<(ModelRelease, u64)>, String> {
        read_release(&self.connection, repo_id, revision)
    }
}

/// A stored release that no longer decodes or validates reads as absent.
fn read_release(
    c: &Connection,
    repo_id: &str,
    revision: &str,
) -> Result<Option<(ModelRelease, u64)>, String> {
    let row: Option<(String, i64)> = c
        .query_row(
            "SELECT release_json, fetched_at_unix_ms FROM catalog_releases WHERE repo_id=?1 AND revision=?2",
            params![repo_id, revision],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|_| "Cannot read cached release.")?;
    Ok(row.and_then(|(json, at)| {
        let release: ModelRelease = serde_json::from_str(&json).ok()?;
        (validate_release(&release).is_ok()
            && release.repo_id == repo_id
            && release.revision == revision)
            .then(|| (release, u64::try_from(at).unwrap_or(0)))
    }))
}

/// Keep at most `max_rows` rows and `max_bytes` of JSON, deleting the oldest first and
/// never the newest. Runs inside the caller's writer transaction. Table and column
/// names are constants from this module, never user input.
fn evict(
    c: &Connection,
    table: &str,
    max_rows: i64,
    max_bytes: i64,
    column: &str,
) -> Result<(), String> {
    let error = "Cannot trim the catalog cache.";
    let (mut rows, mut bytes): (i64, i64) = c
        .query_row(
            &format!(
                "SELECT count(*), coalesce(sum(length(CAST({column} AS BLOB))),0) FROM {table}"
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| error)?;
    if rows <= max_rows && bytes <= max_bytes {
        return Ok(());
    }
    let mut statement = c
        .prepare(&format!(
            "SELECT rowid, length(CAST({column} AS BLOB)) FROM {table} ORDER BY fetched_at_unix_ms ASC, rowid ASC"
        ))
        .map_err(|_| error)?;
    let candidates: Vec<(i64, i64)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| error)?
        .collect::<Result<_, _>>()
        .map_err(|_| error)?;
    for (rowid, length) in candidates.iter().take(candidates.len().saturating_sub(1)) {
        if rows <= max_rows && bytes <= max_bytes {
            break;
        }
        c.execute(&format!("DELETE FROM {table} WHERE rowid=?1"), [rowid])
            .map_err(|_| error)?;
        rows -= 1;
        bytes -= length;
    }
    Ok(())
}
