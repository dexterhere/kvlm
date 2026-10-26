// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline cache of catalog pages and releases, stored beside benchmark history.
//! Bounded and evicted oldest first; call from a blocking worker.

use super::queries::HistoryStore;
use crate::models::{
    catalog::{CatalogPage, valid_repo_id, valid_revision},
    release::ModelRelease,
};
use rusqlite::{OptionalExtension, params};

const MAX_QUERY_ROWS: i64 = 200;
const MAX_QUERY_BYTES: i64 = 16 * 1024 * 1024;
const MAX_RELEASE_ROWS: i64 = 2_000;
const MAX_RELEASE_BYTES: i64 = 64 * 1024 * 1024;
const MAX_KEY: usize = 512;

fn timestamp(ms: u64) -> Result<i64, String> {
    i64::try_from(ms).map_err(|_| "Catalog timestamp exceeds the supported range.".into())
}

impl HistoryStore {
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
        self.connection
            .execute(
                "INSERT INTO catalog_queries(query_key,result_json,fetched_at_unix_ms) VALUES (?1,?2,?3)
                 ON CONFLICT(query_key) DO UPDATE SET result_json=excluded.result_json, fetched_at_unix_ms=excluded.fetched_at_unix_ms",
                params![key, json, timestamp(now_ms)?],
            )
            .map_err(|_| "Cannot cache catalog page. The page may be too large or the disk full.")?;
        self.evict(
            "catalog_queries",
            MAX_QUERY_ROWS,
            MAX_QUERY_BYTES,
            "result_json",
        )
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

    /// Cache a release. A revision is immutable: a stored size or checksum is never
    /// replaced by a different one.
    pub fn cache_release(&self, release: &ModelRelease, now_ms: u64) -> Result<(), String> {
        if !valid_repo_id(&release.repo_id) || !valid_revision(&release.revision) {
            return Err("Release identity is invalid.".into());
        }
        if let Some((existing, _)) = self.cached_release(&release.repo_id, &release.revision)? {
            for old in &existing.artifacts {
                let Some(new) = release
                    .artifacts
                    .iter()
                    .find(|a| a.artifact_id == old.artifact_id)
                else {
                    continue;
                };
                if old.size_bytes.is_some_and(|v| Some(v) != new.size_bytes)
                    || old
                        .sha256
                        .as_ref()
                        .is_some_and(|v| Some(v) != new.sha256.as_ref())
                {
                    return Err("Cached revision conflicts with newly fetched data. Keep the cache and retry later.".into());
                }
            }
        }
        let json = serde_json::to_string(release).map_err(|_| "Cannot encode release.")?;
        self.connection
            .execute(
                "INSERT INTO catalog_releases(repo_id,revision,release_json,fetched_at_unix_ms) VALUES (?1,?2,?3,?4)
                 ON CONFLICT(repo_id,revision) DO UPDATE SET release_json=excluded.release_json, fetched_at_unix_ms=excluded.fetched_at_unix_ms",
                params![release.repo_id, release.revision, json, timestamp(now_ms)?],
            )
            .map_err(|_| "Cannot cache release. It may be too large or the disk full.")?;
        self.evict(
            "catalog_releases",
            MAX_RELEASE_ROWS,
            MAX_RELEASE_BYTES,
            "release_json",
        )
    }

    pub fn cached_release(
        &self,
        repo_id: &str,
        revision: &str,
    ) -> Result<Option<(ModelRelease, u64)>, String> {
        let row: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT release_json, fetched_at_unix_ms FROM catalog_releases WHERE repo_id=?1 AND revision=?2",
                params![repo_id, revision],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "Cannot read cached release.")?;
        row.map(|(json, at)| {
            let release =
                serde_json::from_str(&json).map_err(|_| "Cached release is unreadable.")?;
            Ok((release, u64::try_from(at).unwrap_or(0)))
        })
        .transpose()
    }

    /// Keep at most `max_rows` rows and `max_bytes` of JSON, deleting the oldest first.
    /// Table and column names are constants from this module, never user input.
    fn evict(
        &self,
        table: &str,
        max_rows: i64,
        max_bytes: i64,
        column: &str,
    ) -> Result<(), String> {
        let order = "fetched_at_unix_ms ASC, rowid ASC";
        let (rows, bytes): (i64, i64) = self
            .connection
            .query_row(
                &format!(
                    "SELECT count(*), coalesce(sum(length(CAST({column} AS BLOB))),0) FROM {table}"
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| "Cannot measure the catalog cache.")?;
        if rows <= max_rows && bytes <= max_bytes {
            return Ok(());
        }
        // Delete oldest rows until both bounds hold; the newest row always stays.
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT rowid, length(CAST({column} AS BLOB)) FROM {table} ORDER BY {order}"
            ))
            .map_err(|_| "Cannot order the catalog cache.")?;
        let candidates: Vec<(i64, i64)> = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|_| "Cannot order the catalog cache.")?
            .collect::<Result<_, _>>()
            .map_err(|_| "Cannot order the catalog cache.")?;
        let (mut rows, mut bytes) = (rows, bytes);
        for (rowid, length) in candidates.iter().take(candidates.len().saturating_sub(1)) {
            if rows <= max_rows && bytes <= max_bytes {
                break;
            }
            self.connection
                .execute(&format!("DELETE FROM {table} WHERE rowid=?1"), [rowid])
                .map_err(|_| "Cannot evict old catalog cache rows.")?;
            rows -= 1;
            bytes -= length;
        }
        Ok(())
    }
}
