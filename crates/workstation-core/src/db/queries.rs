// SPDX-License-Identifier: GPL-3.0-or-later

use super::{HistoryEntry, RunCompletion, RunOutcome, RunSnapshot, schema};
use crate::{hardware::HardwareProfile, models::artifact::canonical_manifest_digest};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

const MAX_JSON: usize = 262_144;
pub struct HistoryStore {
    connection: Connection,
}

impl HistoryStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("History database path must be absolute.".into());
        }
        let parent = path
            .parent()
            .ok_or("History database needs a parent directory.")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(
                |_| "Cannot create private history directory. Check data directory permissions.",
            )?;
        let directory =
            fs::symlink_metadata(parent).map_err(|_| "Cannot inspect history directory.")?;
        if !directory.is_dir() || directory.mode() & 0o022 != 0 {
            return Err("History directory must be a real directory, not a symlink, and not writable by other users.".into());
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => {
                return Err(
                    "Cannot create history database. Check permissions and free disk space.".into(),
                );
            }
        }
        if !fs::symlink_metadata(path)
            .map_err(|_| "Cannot inspect history file.")?
            .is_file()
        {
            return Err("History database must be a regular file, not a symlink.".into());
        }
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            match fs::symlink_metadata(Path::new(&name)) {
                Ok(metadata) if !metadata.is_file() || metadata.mode() & 0o077 != 0 => {
                    return Err("History files must be regular files accessible only to their owner. Restrict restored database and companion file permissions to 0600 before retrying.".into());
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err("Cannot inspect history file permissions.".into()),
            }
        }
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| "Cannot open history database. Check permissions and file integrity.")?;
        schema::initialize(&mut connection)?;
        Ok(Self { connection })
    }
    pub fn start(&self, snapshot: &RunSnapshot) -> Result<i64, String> {
        let started = i64::try_from(snapshot.started_at_unix_ms)
            .map_err(|_| "History timestamp exceeds the supported range.")?;
        let json = encode(snapshot)?;
        self.connection.execute("INSERT INTO benchmark_runs(started_at_unix_ms,snapshot_json) VALUES (?1,?2)", params![started, json])
            .map_err(|_| "Cannot start history record. Check free disk space and retry after other writers finish.")?;
        Ok(self.connection.last_insert_rowid())
    }
    pub fn finish(&self, id: i64, completion: &RunCompletion) -> Result<(), String> {
        let json = encode(completion)?;
        let changed = self.connection.execute("UPDATE benchmark_runs SET completion_json=?1 WHERE id=?2 AND completion_json IS NULL", params![json, id])
            .map_err(|_| "Cannot save benchmark outcome. Check free disk space and retry.")?;
        if changed == 1 {
            return Ok(());
        }
        let existing: Option<Option<String>> = self
            .connection
            .query_row(
                "SELECT completion_json FROM benchmark_runs WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Cannot confirm saved benchmark outcome.")?;
        if existing.as_ref().and_then(|v| v.as_ref()) == Some(&json) {
            Ok(())
        } else {
            Err("History record is missing or already has a different outcome.".into())
        }
    }
    pub fn recent(&self) -> Result<Vec<HistoryEntry>, String> {
        let mut statement = self.connection.prepare("SELECT id, length(CAST(snapshot_json AS BLOB)), length(CAST(completion_json AS BLOB)), snapshot_json, completion_json FROM benchmark_runs ORDER BY id DESC LIMIT 100")
            .map_err(|_| "Cannot query benchmark history.")?;
        let mut rows = statement
            .query([])
            .map_err(|_| "Cannot read benchmark history.")?;
        let mut entries = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|_| "Cannot read benchmark history row.")?
        {
            let length: i64 = row.get(1).map_err(|_| "Invalid history snapshot length.")?;
            let completion_length: Option<i64> =
                row.get(2).map_err(|_| "Invalid history outcome length.")?;
            if length < 0
                || length > MAX_JSON as i64
                || completion_length.is_some_and(|n| n < 0 || n > MAX_JSON as i64)
            {
                return Err(
                    "History record exceeds the supported size. Keep the file for recovery.".into(),
                );
            }
            let snapshot: String = row.get(3).map_err(|_| "Invalid history snapshot.")?;
            let completion: Option<String> = row.get(4).map_err(|_| "Invalid history outcome.")?;
            entries.push(HistoryEntry {
                id: row.get(0).map_err(|_| "Invalid history identifier.")?,
                snapshot: decode(&snapshot)?,
                completion: completion.as_deref().map(decode).transpose()?,
            });
        }
        Ok(entries)
    }

    /// Search exact identities across history, independent of the recent-view limit.
    /// Call from a blocking worker with a freshly detected hardware profile.
    pub fn verified_run(
        &self,
        digest: &str,
        hardware: &HardwareProfile,
    ) -> Result<Option<i64>, String> {
        let digest = canonical_manifest_digest(digest).ok_or("Invalid manifest digest.")?;
        let Some(hash) = hardware
            .identity_hash()
            .filter(|hash| hardware.profile_hash.as_deref() == Some(hash.as_str()))
        else {
            return Ok(None);
        };
        // CASE guards malformed JSON; every selected record is revalidated below.
        let mut statement = self.connection.prepare("SELECT id, length(CAST(snapshot_json AS BLOB)), length(CAST(completion_json AS BLOB)), snapshot_json, completion_json
            FROM benchmark_runs WHERE completion_json IS NOT NULL
            AND CASE WHEN json_valid(snapshot_json) THEN lower(json_extract(snapshot_json, '$.model_digest')) END IN (?1, ?2)
            AND CASE WHEN json_valid(snapshot_json) THEN json_extract(snapshot_json, '$.hardware.profile_hash') END = ?3
            ORDER BY id DESC")
            .map_err(|_| "Cannot query benchmark verification.")?;
        let mut rows = statement
            .query(params![digest, &digest[7..], hash])
            .map_err(|_| "Cannot read benchmark verification.")?;
        while let Some(row) = rows
            .next()
            .map_err(|_| "Cannot read benchmark verification row.")?
        {
            let snapshot_length: i64 = row
                .get(1)
                .map_err(|_| "Invalid verification snapshot length.")?;
            let completion_length: i64 = row
                .get(2)
                .map_err(|_| "Invalid verification outcome length.")?;
            if !(0..=MAX_JSON as i64).contains(&snapshot_length)
                || !(0..=MAX_JSON as i64).contains(&completion_length)
            {
                return Err(
                    "Verification record exceeds the supported size. Keep the file for recovery."
                        .into(),
                );
            }
            let snapshot_json: String = row.get(3).map_err(|_| "Invalid verification snapshot.")?;
            let completion_json: String =
                row.get(4).map_err(|_| "Invalid verification outcome.")?;
            let snapshot: RunSnapshot = decode(&snapshot_json)?;
            let completion: RunCompletion = decode(&completion_json)?;
            let matches = snapshot
                .model_digest
                .as_deref()
                .and_then(canonical_manifest_digest)
                .as_ref()
                == Some(&digest)
                && completion
                    .model_digest
                    .as_deref()
                    .and_then(canonical_manifest_digest)
                    .as_ref()
                    == Some(&digest)
                && snapshot
                    .hardware
                    .as_ref()
                    .and_then(|profile| profile.profile_hash.as_deref())
                    == Some(hash.as_str())
                && snapshot
                    .hardware
                    .as_ref()
                    .and_then(HardwareProfile::identity_hash)
                    .as_deref()
                    == Some(hash.as_str())
                && snapshot.runtime_endpoint == "http://127.0.0.1:11434"
                && snapshot.started_at_unix_ms > 0
                && completion.finished_at_unix_ms >= snapshot.started_at_unix_ms
                && completion
                    .runtime_version
                    .as_ref()
                    .is_some_and(|version| !version.trim().is_empty());
            if matches
                && matches!(&completion.outcome, RunOutcome::Completed(metrics)
                if metrics.response_chunks > 0 && metrics.elapsed_ns > 0
                && metrics.first_response_ns.is_some_and(|first| first <= metrics.elapsed_ns)
                && metrics.eval_count.is_some_and(|count| count > 0)
                && metrics.eval_duration_ns.is_some_and(|duration| duration > 0)
                && matches!(metrics.done_reason.as_str(), "stop" | "length"))
            {
                return row
                    .get(0)
                    .map(Some)
                    .map_err(|_| "Invalid verification identifier.".into());
            }
        }
        Ok(None)
    }
}

fn encode(value: &impl Serialize) -> Result<String, String> {
    let json = serde_json::to_string(value).map_err(|_| "Cannot encode benchmark history.")?;
    if json.len() > MAX_JSON {
        return Err("Benchmark history record exceeds the supported size.".into());
    }
    Ok(json)
}
fn decode<T: DeserializeOwned>(json: &str) -> Result<T, String> {
    serde_json::from_str(json)
        .map_err(|_| "History contains an unreadable record. Keep the file for recovery.".into())
}
