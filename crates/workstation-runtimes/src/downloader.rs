// SPDX-License-Identifier: GPL-3.0-or-later
//! Resumable, verified downloads of one catalog artifact into the application's model store.
//!
//! Only an explicit user action starts a transfer. Bytes are written to a digest-named
//! partial file, hashed as they arrive, and published to `blobs/` only when the SHA-256
//! matches. The pinned commit plus that digest are the integrity guarantee; the server's
//! word is not. Tokens, gated access and the keyring arrive in a later phase.

use crate::{
    huggingface::{HfError, HuggingFaceClient},
    secrets::TokenSlot,
};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{runtime::Handle, sync::watch};
use workstation_core::{
    db::{
        self, HistoryStore,
        downloads::{DownloadRow, DownloadState, NewDownload},
    },
    models::{
        artifact::ModelArtifact,
        catalog::canonical_sha256,
        download::{Published, Storage, StorageError},
    },
};

const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const SAVE_EVERY: Duration = Duration::from_secs(2);
const SPACE_CHECK_EVERY: u64 = 64 * 1024 * 1024;

/// What the user asked a running transfer to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Keep the partial file and let the user resume.
    Pause,
    /// Keep the partial file; the user may discard it later.
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub download_id: i64,
    pub bytes_done: u64,
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadEnd {
    /// Verified and published (or already there).
    Completed,
    Paused {
        /// `Some("space_reserve")` when free space ran short.
        code: Option<&'static str>,
        message: Option<String>,
    },
    Cancelled,
    Failed {
        code: &'static str,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadOutcome {
    pub download_id: Option<i64>,
    pub bytes_done: u64,
    pub end: DownloadEnd,
}

pub struct Downloader {
    client: Arc<HuggingFaceClient>,
    database: PathBuf,
    models_dir: PathBuf,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Downloader {
    pub fn new(token: TokenSlot) -> Result<Self, String> {
        Ok(Self::with_parts(
            Arc::new(HuggingFaceClient::new(token).map_err(|e| e.to_string())?),
            db::default_database_path()?,
            db::default_models_dir()?,
        ))
    }

    pub fn with_parts(
        client: Arc<HuggingFaceClient>,
        database: PathBuf,
        models_dir: PathBuf,
    ) -> Self {
        Self {
            client,
            database,
            models_dir,
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&HistoryStore, &PathBuf) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (database, models) = (self.database.clone(), self.models_dir.clone());
        tokio::task::spawn_blocking(move || work(&HistoryStore::open(&database)?, &models))
            .await
            .map_err(|_| "Download bookkeeping failed. Retry.".to_string())?
    }

    /// Saved downloads. Reads local state only; first reconciles rows with the files (after
    /// a crash) unless another instance holds the store.
    pub async fn list(&self) -> Result<Vec<DownloadRow>, String> {
        self.blocking(|store, models| {
            if let Ok(storage) = Storage::open(models) {
                store.reconcile_downloads(&storage, now_ms())?;
            }
            store.list_downloads()
        })
        .await
    }

    /// Delete an unfinished download's partial file and its row.
    pub async fn discard(&self, download_id: i64) -> Result<(), String> {
        self.blocking(move |store, models| {
            let row = store
                .get_download(download_id)?
                .ok_or("That download no longer exists.")?;
            if row.state == DownloadState::Completed {
                return store.delete_download(download_id);
            }
            // The store lock is held until the row is gone, so a resume cannot slip in
            // between and have its fresh row deleted.
            let storage = Storage::open(models).map_err(|e| e.to_string())?;
            let row = store
                .get_download(download_id)?
                .ok_or("That download no longer exists.")?;
            if row.state.is_active() {
                return Err("Pause or cancel the download before discarding it.".into());
            }
            // Identical files from different repositories share one partial file.
            let shared = store.list_downloads()?.iter().any(|other| {
                other.id != row.id
                    && other.expected_sha256 == row.expected_sha256
                    && other.state != DownloadState::Completed
            });
            if !shared {
                storage
                    .discard_part(&row.expected_sha256)
                    .map_err(|e| e.to_string())?;
            }
            store.delete_download(download_id)
        })
        .await
    }

    /// Download `artifact` (or resume it). Returns when it finishes, pauses, or fails.
    /// `progress` is called at most four times a second from a blocking thread.
    pub async fn run(
        &self,
        artifact: ModelArtifact,
        stop: watch::Receiver<Option<Stop>>,
        progress: impl Fn(Progress) + Send + 'static,
    ) -> DownloadOutcome {
        let (client, database, models) = (
            Arc::clone(&self.client),
            self.database.clone(),
            self.models_dir.clone(),
        );
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || {
            Transfer::start(
                &client, &database, &models, &handle, &artifact, stop, &progress,
            )
        })
        .await
        .unwrap_or_else(|_| DownloadOutcome {
            download_id: None,
            bytes_done: 0,
            end: DownloadEnd::Failed {
                code: "internal",
                message: "The download worker stopped unexpectedly. Resume to retry.".into(),
            },
        })
    }
}

struct Transfer<'a> {
    store: HistoryStore,
    storage: Storage,
    id: i64,
    sha: String,
    size: u64,
    progress: &'a dyn Fn(Progress),
}

impl Transfer<'_> {
    fn start(
        client: &HuggingFaceClient,
        database: &Path,
        models: &Path,
        handle: &Handle,
        artifact: &ModelArtifact,
        stop: watch::Receiver<Option<Stop>>,
        progress: &dyn Fn(Progress),
    ) -> DownloadOutcome {
        let early = |code, message: String| DownloadOutcome {
            download_id: None,
            bytes_done: 0,
            end: DownloadEnd::Failed { code, message },
        };
        let (Some(sha), Some(size)) = (
            artifact.sha256.as_deref().and_then(canonical_sha256),
            artifact.size_bytes.filter(|size| *size > 0),
        ) else {
            return early(
                "not_downloadable",
                "This file has no published checksum or size, so it cannot be downloaded safely."
                    .into(),
            );
        };
        if artifact.shards.is_some() {
            return early(
                "not_downloadable",
                "Split models are not downloadable yet.".into(),
            );
        }
        let store = match HistoryStore::open(database) {
            Ok(store) => store,
            Err(message) => return early("database", message),
        };
        let storage = match Storage::open(models) {
            Ok(storage) => storage,
            Err(StorageError::InUse) => {
                return early("in_use", StorageError::InUse.to_string());
            }
            Err(error) => return early("storage", error.to_string()),
        };
        let new = NewDownload {
            artifact_id: artifact.artifact_id.clone(),
            repo_id: artifact.repo_id.clone(),
            revision: artifact.revision.clone(),
            filename: artifact.filename.clone(),
            sha256: sha.clone(),
            size,
        };
        let id = match store.begin_download(&new, now_ms()) {
            Ok(id) => id,
            Err(message) => return early("busy", message),
        };
        let mut transfer = Transfer {
            store,
            storage,
            id,
            sha,
            size,
            progress,
        };
        transfer.run(client, handle, artifact, stop)
    }

    fn outcome(&self, bytes_done: u64, end: DownloadEnd) -> DownloadOutcome {
        let (state, code) = match &end {
            DownloadEnd::Completed => (DownloadState::Completed, None),
            DownloadEnd::Paused { code, .. } => (DownloadState::Paused, *code),
            DownloadEnd::Cancelled => (DownloadState::Cancelled, None),
            DownloadEnd::Failed { code, .. } => (DownloadState::Failed, Some(*code)),
        };
        let _ = self
            .store
            .update_download(self.id, state, bytes_done, code, now_ms());
        DownloadOutcome {
            download_id: Some(self.id),
            bytes_done,
            end,
        }
    }

    fn failed(&self, bytes_done: u64, code: &'static str, message: String) -> DownloadOutcome {
        self.outcome(bytes_done, DownloadEnd::Failed { code, message })
    }

    fn storage_failed(&self, bytes_done: u64, error: StorageError) -> DownloadOutcome {
        let code = match error {
            StorageError::DiskFull => "disk_full",
            StorageError::Integrity => "integrity",
            StorageError::PartialBudget => "partial_budget",
            StorageError::PartTooLong => "part_too_long",
            StorageError::InUse => "in_use",
            _ => "storage",
        };
        self.failed(bytes_done, code, error.to_string())
    }

    fn report(&self, bytes_done: u64) {
        (self.progress)(Progress {
            download_id: self.id,
            bytes_done,
            total: self.size,
        });
    }

    fn run(
        &mut self,
        client: &HuggingFaceClient,
        handle: &Handle,
        artifact: &ModelArtifact,
        mut stop: watch::Receiver<Option<Stop>>,
    ) -> DownloadOutcome {
        // Already published and verified: nothing to fetch.
        match self.storage.verified_blob(&self.sha, self.size) {
            Ok(true) => {
                let _ = self.storage.discard_part(&self.sha);
                self.report(self.size);
                return self.outcome(self.size, DownloadEnd::Completed);
            }
            Ok(false) => {}
            Err(error) => return self.storage_failed(0, error),
        }
        if let Err(error) = self.storage.check_partial_budget(&self.sha, self.size) {
            return self.storage_failed(0, error);
        }
        let (mut file, mut done) = match self.storage.open_part(&self.sha, self.size) {
            Ok(opened) => opened,
            Err(error) => return self.storage_failed(0, error),
        };
        if let Some(request) = *stop.borrow() {
            return self.stopped(request, done);
        }
        if let Err(error) = self.storage.check_space(self.size - done) {
            return self.paused_for_space(done, error);
        }
        let mut hasher = match self.storage.hash_prefix(&mut file, done) {
            Ok(hasher) => hasher,
            Err(error) => return self.storage_failed(done, error),
        };
        self.report(done);
        // Hashing a long partial file and connecting cannot be interrupted; check around them.
        if let Some(request) = *stop.borrow() {
            return self.stopped(request, done);
        }

        // A part file that already holds every byte needs no network at all.
        if done < self.size {
            let opened = handle.block_on(client.open_download(
                &artifact.repo_id,
                &artifact.revision,
                &artifact.filename,
                self.size,
                done,
            ));
            let mut stream = match opened {
                Ok(stream) => stream,
                Err(error) => return self.network_failed(done, &error),
            };
            if stream.start == 0 && done > 0 {
                // The server ignored the range: start over rather than splice.
                if let Err(error) = self.storage.restart_part(&mut file) {
                    return self.storage_failed(done, error);
                }
                hasher = Sha256::new();
                done = 0;
            }
            let (mut last_report, mut last_save, mut checked) =
                (Instant::now(), Instant::now(), done);
            loop {
                if let Some(request) = *stop.borrow() {
                    return self.stopped(request, done);
                }
                let next = handle.block_on(async {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => None,
                        chunk = stream.chunk() => Some(chunk),
                    }
                });
                let chunk = match next {
                    None => {
                        // A dropped sender (the app is closing) pauses instead of spinning.
                        if stop.has_changed().is_err() {
                            return self.stopped(Stop::Pause, done);
                        }
                        continue; // a stop request arrived; handled at the loop top
                    }
                    Some(Err(error)) => return self.network_failed(done, &error),
                    Some(Ok(None)) => break,
                    Some(Ok(Some(chunk))) => chunk,
                };
                if done + chunk.len() as u64 > self.size {
                    // More than the published size: the response cannot be trusted.
                    drop(file);
                    let _ = self.storage.discard_part(&self.sha);
                    return self.failed(
                        0,
                        "too_large",
                        "The server sent more data than the file's published size. The partial file was discarded.".into(),
                    );
                }
                if let Err(error) = file.write_all(&chunk) {
                    return self.storage_failed(done, error.into());
                }
                hasher.update(&chunk);
                done += chunk.len() as u64;
                if done - checked >= SPACE_CHECK_EVERY {
                    checked = done;
                    if let Err(error) = self.storage.check_space(self.size - done) {
                        let _ = file.sync_all();
                        return self.paused_for_space(done, error);
                    }
                }
                if last_report.elapsed() >= PROGRESS_EVERY {
                    last_report = Instant::now();
                    self.report(done);
                }
                if last_save.elapsed() >= SAVE_EVERY {
                    last_save = Instant::now();
                    let _ = self.store.update_download(
                        self.id,
                        DownloadState::Running,
                        done,
                        None,
                        now_ms(),
                    );
                }
            }
            if done != self.size {
                let _ = file.sync_all();
                return self.failed(
                    done,
                    "incomplete",
                    "The connection ended before the whole file arrived. Resume to continue."
                        .into(),
                );
            }
        }
        if let Some(request) = *stop.borrow() {
            let _ = file.sync_all();
            return self.stopped(request, self.size);
        }
        self.report(self.size);
        let _ = self.store.update_download(
            self.id,
            DownloadState::Verifying,
            self.size,
            None,
            now_ms(),
        );
        match self.storage.publish(&self.sha, file, hasher) {
            Ok(Published::New | Published::Reused) => {
                self.outcome(self.size, DownloadEnd::Completed)
            }
            Err(error) => self.storage_failed(0, error),
        }
    }

    fn stopped(&self, request: Stop, done: u64) -> DownloadOutcome {
        let end = match request {
            Stop::Pause => DownloadEnd::Paused {
                code: None,
                message: None,
            },
            Stop::Cancel => DownloadEnd::Cancelled,
        };
        self.outcome(done, end)
    }

    fn paused_for_space(&self, done: u64, error: StorageError) -> DownloadOutcome {
        match error {
            StorageError::NotEnoughSpace { .. } => self.outcome(
                done,
                DownloadEnd::Paused {
                    code: Some("space_reserve"),
                    message: Some(error.to_string()),
                },
            ),
            other => self.storage_failed(done, other),
        }
    }

    fn network_failed(&self, done: u64, error: &HfError) -> DownloadOutcome {
        let code = match error {
            HfError::AccessRequired => "access_required",
            HfError::NotFound => "not_found",
            HfError::RateLimited { .. } => "rate_limited",
            HfError::Offline | HfError::Timeout => "network",
            _ => "bad_response",
        };
        self.failed(done, code, error.to_string())
    }
}

#[cfg(test)]
mod tests;
