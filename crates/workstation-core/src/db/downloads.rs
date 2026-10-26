// SPDX-License-Identifier: GPL-3.0-or-later
//! Download bookkeeping. Rows describe intent and progress; the files in the model store are
//! the truth, which `reconcile` checks after a crash. Call from a blocking worker.

use super::queries::HistoryStore;
use crate::models::{
    artifact::valid_remote_filename,
    catalog::{canonical_sha256, valid_repo_id, valid_revision},
    download::{Storage, StorageError},
};
use rusqlite::{OptionalExtension, Row, params};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "verifying" => Self::Verifying,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// Bytes are being moved or checked right now.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Verifying)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadRow {
    pub id: i64,
    pub artifact_id: String,
    pub repo_id: String,
    pub revision: String,
    pub filename: String,
    pub expected_sha256: String,
    pub expected_size: u64,
    pub state: DownloadState,
    pub bytes_done: u64,
    pub failure_code: Option<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

/// What to download: the pinned identity of one catalog artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDownload {
    pub artifact_id: String,
    pub repo_id: String,
    pub revision: String,
    pub filename: String,
    pub sha256: String,
    pub size: u64,
}

const COLUMNS: &str = "id,artifact_id,repo_id,revision,filename,expected_sha256,expected_size,state,bytes_done,failure_code,created_at_unix_ms,updated_at_unix_ms";
const MAX_ROWS: i64 = 500;

fn read(row: &Row<'_>) -> rusqlite::Result<Option<DownloadRow>> {
    let state: String = row.get(7)?;
    let (size, done, created, updated): (i64, i64, i64, i64) =
        (row.get(6)?, row.get(8)?, row.get(10)?, row.get(11)?);
    Ok(DownloadState::parse(&state).map(|state| DownloadRow {
        id: row.get(0).unwrap_or(0),
        artifact_id: row.get(1).unwrap_or_default(),
        repo_id: row.get(2).unwrap_or_default(),
        revision: row.get(3).unwrap_or_default(),
        filename: row.get(4).unwrap_or_default(),
        expected_sha256: row.get(5).unwrap_or_default(),
        expected_size: u64::try_from(size).unwrap_or(0),
        state,
        bytes_done: u64::try_from(done).unwrap_or(0),
        failure_code: row.get(9).unwrap_or(None),
        created_at_unix_ms: u64::try_from(created).unwrap_or(0),
        updated_at_unix_ms: u64::try_from(updated).unwrap_or(0),
    }))
}

fn ms(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Download timestamp is out of range.".to_string())
}

impl HistoryStore {
    /// Start, or resume, the download of one artifact and return its row id. An unfinished
    /// row for the same artifact and digest is reused. Only one download may be active.
    pub fn begin_download(&self, new: &NewDownload, now_ms: u64) -> Result<i64, String> {
        if !valid_repo_id(&new.repo_id)
            || !valid_revision(&new.revision)
            || !valid_remote_filename(&new.filename)
            || canonical_sha256(&new.sha256).as_deref() != Some(&new.sha256)
            || new.size == 0
            || new.artifact_id.is_empty()
            || new.artifact_id.len() > 1024
        {
            return Err("Download request is invalid.".into());
        }
        let (now, size) = (ms(now_ms)?, ms(new.size)?);
        self.write(|c| {
            let active: i64 = c
                .query_row(
                    "SELECT count(*) FROM downloads WHERE state IN ('queued','running','verifying')",
                    [],
                    |r| r.get(0),
                )
                .map_err(|_| "Cannot read downloads.")?;
            if active > 0 {
                return Err("Another download is already running. Pause or cancel it first.".into());
            }
            let existing: Option<i64> = c
                .query_row(
                    "SELECT id FROM downloads WHERE artifact_id=?1 AND expected_sha256=?2 AND state<>'completed' ORDER BY id DESC LIMIT 1",
                    params![new.artifact_id, new.sha256],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| "Cannot read downloads.")?;
            if let Some(id) = existing {
                c.execute(
                    "UPDATE downloads SET state='running', failure_code=NULL, updated_at_unix_ms=?2 WHERE id=?1",
                    params![id, now],
                )
                .map_err(|_| "Cannot update the download.")?;
                return Ok(id);
            }
            c.execute(
                "INSERT INTO downloads(artifact_id,repo_id,revision,filename,expected_sha256,expected_size,state,bytes_done,failure_code,created_at_unix_ms,updated_at_unix_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,'running',0,NULL,?7,?7)",
                params![new.artifact_id, new.repo_id, new.revision, new.filename, new.sha256, size, now],
            )
            .map_err(|_| "Cannot record the download.")?;
            let id = c.last_insert_rowid();
            c.execute(
                "DELETE FROM downloads WHERE id IN (SELECT id FROM downloads WHERE state = 'completed' ORDER BY updated_at_unix_ms ASC, id ASC LIMIT max(0,(SELECT count(*) FROM downloads)-?1))",
                [MAX_ROWS],
            )
            .map_err(|_| "Cannot trim the download list.")?;
            Ok(id)
        })
    }

    /// Record progress or a state change.
    pub fn update_download(
        &self,
        id: i64,
        state: DownloadState,
        bytes_done: u64,
        failure_code: Option<&str>,
        now_ms: u64,
    ) -> Result<(), String> {
        let (now, done) = (ms(now_ms)?, ms(bytes_done)?);
        self.write(|c| {
            c.execute(
                "UPDATE downloads SET state=?2, bytes_done=?3, failure_code=?4, updated_at_unix_ms=?5 WHERE id=?1",
                params![id, state.as_str(), done, failure_code, now],
            )
            .map_err(|_| "Cannot update the download.")?;
            Ok(())
        })
    }

    pub fn get_download(&self, id: i64) -> Result<Option<DownloadRow>, String> {
        let row = self
            .connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM downloads WHERE id=?1"),
                [id],
                read,
            )
            .optional()
            .map_err(|_| "Cannot read downloads.")?;
        Ok(row.flatten())
    }

    /// Newest first; rows with an unknown state are skipped.
    pub fn list_downloads(&self) -> Result<Vec<DownloadRow>, String> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM downloads ORDER BY id DESC LIMIT {MAX_ROWS}"
            ))
            .map_err(|_| "Cannot read downloads.")?;
        let rows = statement
            .query_map([], read)
            .map_err(|_| "Cannot read downloads.")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Cannot read downloads.")?;
        Ok(rows.into_iter().flatten().collect())
    }

    pub fn delete_download(&self, id: i64) -> Result<(), String> {
        self.write(|c| {
            c.execute("DELETE FROM downloads WHERE id=?1", [id])
                .map_err(|_| "Cannot remove the download.")?;
            Ok(())
        })
    }

    /// Bring rows and files back in line after a crash. Local only: no network, no resume.
    ///
    /// Active rows become paused; a row whose verified file is already published becomes
    /// completed; a paused row that claims bytes but has no part file and no published file
    /// becomes `failed(missing_partial)`. A row changes only after its file check is done.
    pub fn reconcile_downloads(&self, storage: &Storage, now_ms: u64) -> Result<(), String> {
        for row in self.list_downloads()? {
            if matches!(row.state, DownloadState::Completed) {
                continue;
            }
            let part = storage
                .part_len(&row.expected_sha256)
                .map_err(storage_message)?;
            if storage
                .verified_blob(&row.expected_sha256, row.expected_size)
                .map_err(storage_message)?
            {
                storage
                    .discard_part(&row.expected_sha256)
                    .map_err(storage_message)?;
                self.update_download(
                    row.id,
                    DownloadState::Completed,
                    row.expected_size,
                    None,
                    now_ms,
                )?;
                continue;
            }
            let done = part.unwrap_or(0).min(row.expected_size);
            let resumable = row.state.is_active() || row.state == DownloadState::Paused;
            let (state, code) = if resumable && part.is_none() && row.bytes_done > 0 {
                (DownloadState::Failed, Some("missing_partial"))
            } else if row.state.is_active() {
                (DownloadState::Paused, None)
            } else {
                continue;
            };
            self.update_download(row.id, state, done, code, now_ms)?;
        }
        Ok(())
    }
}

/// Text for a person; carries no paths.
pub fn storage_message(error: StorageError) -> String {
    error.to_string()
}
