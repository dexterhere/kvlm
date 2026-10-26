// SPDX-License-Identifier: GPL-3.0-or-later
//! Catalog service: explicit-action network fetches backed by the local offline cache.
//!
//! A live result is cached. A transient failure falls back to the saved copy with a
//! notice; "no access" and "not found" never do, because stale data could hide that a
//! repository was removed or made private. Nothing here runs at startup.

use crate::huggingface::{HfError, HuggingFaceClient};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use workstation_core::{
    db::{
        self, HistoryStore,
        catalog_cache::{REVISION_CONFLICT, page_key},
    },
    models::{
        artifact::ModelArtifact,
        catalog::CatalogPage,
        gguf::{GgufError, GgufMetadata, inspect_prefix},
        release::ModelRelease,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadSource {
    Live,
    /// Saved earlier; the interface shows how old it is.
    Cached {
        fetched_at_unix_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogLoad {
    pub page: CatalogPage,
    pub source: LoadSource,
    /// Why live data is missing, or that the cache could not be updated.
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseLoad {
    pub release: ModelRelease,
    pub source: LoadSource,
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectLoad {
    pub metadata: GgufMetadata,
    pub source: LoadSource,
    pub notice: Option<String>,
}

pub struct CatalogService {
    client: HuggingFaceClient,
    database: PathBuf,
}

const CACHE_WRITE_FAILED: &str = "Could not save this result for offline use.";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Only transient failures may be answered from the cache.
fn may_use_cache(error: &HfError) -> bool {
    !matches!(
        error,
        HfError::AccessRequired | HfError::NotFound | HfError::InvalidRequest
    )
}

fn describe(error: &HfError) -> String {
    match error {
        HfError::RateLimited {
            retry_after_secs: Some(seconds),
        } => {
            format!("{error} Try again in about {seconds} seconds.")
        }
        _ => error.to_string(),
    }
}

impl CatalogService {
    pub fn new() -> Result<Self, String> {
        Ok(Self::with_parts(
            HuggingFaceClient::new().map_err(|error| error.to_string())?,
            db::default_database_path()?,
        ))
    }

    pub(crate) fn with_parts(client: HuggingFaceClient, database: PathBuf) -> Self {
        Self { client, database }
    }

    /// Run SQLite work on a blocking thread with a fresh connection.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&HistoryStore) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let path = self.database.clone();
        tokio::task::spawn_blocking(move || work(&HistoryStore::open(&path)?))
            .await
            .map_err(|_| "Catalog cache worker failed. Retry.".to_string())?
    }

    /// Search over the network (explicit user action), caching the page.
    pub async fn search(&self, query: &str, cursor: Option<&str>) -> Result<CatalogLoad, String> {
        match self.client.search(query, cursor).await {
            Ok(page) => {
                // Stamped when the fetch finished, so a late write from an abandoned
                // request can never replace a newer page.
                let (key, stored, fetched_at) = (page_key(query, cursor), page.clone(), now_ms());
                let saved = self
                    .blocking(move |store| store.cache_catalog_page(&key, &stored, fetched_at))
                    .await;
                Ok(CatalogLoad {
                    page,
                    source: LoadSource::Live,
                    notice: saved.is_err().then(|| CACHE_WRITE_FAILED.to_owned()),
                })
            }
            Err(error) if may_use_cache(&error) => match self.cached_search(query, cursor).await {
                Ok(Some(mut cached)) => {
                    cached.notice = Some(format!(
                        "{} Showing results saved earlier.",
                        describe(&error)
                    ));
                    Ok(cached)
                }
                _ => Err(describe(&error)),
            },
            Err(error) => Err(describe(&error)),
        }
    }

    /// Cached page only; never touches the network.
    pub async fn cached_search(
        &self,
        query: &str,
        cursor: Option<&str>,
    ) -> Result<Option<CatalogLoad>, String> {
        let key = page_key(query, cursor);
        let cached = self
            .blocking(move |store| store.cached_catalog_page(&key))
            .await?;
        Ok(cached.map(|(page, fetched_at_unix_ms)| CatalogLoad {
            page,
            source: LoadSource::Cached { fetched_at_unix_ms },
            notice: None,
        }))
    }

    /// Fetch a repository's files (explicit user action), caching the release.
    /// With `pinned` the response must be for exactly that commit.
    pub async fn release(
        &self,
        repo_id: &str,
        pinned: Option<&str>,
    ) -> Result<ReleaseLoad, String> {
        match self.client.release(repo_id, pinned).await {
            Ok(release) => {
                let (stored, fetched_at) = (release.clone(), now_ms());
                let saved = self
                    .blocking(move |store| store.cache_release(&stored, fetched_at))
                    .await;
                // A changed size or checksum for a saved revision is an integrity
                // problem with the response, not a storage problem: do not show it.
                if matches!(&saved, Err(e) if e == REVISION_CONFLICT) {
                    return Err("Hugging Face reported different data for a revision saved earlier. The saved copy was kept and the new data was not used.".into());
                }
                Ok(ReleaseLoad {
                    release,
                    source: LoadSource::Live,
                    notice: saved.is_err().then(|| CACHE_WRITE_FAILED.to_owned()),
                })
            }
            Err(error) if may_use_cache(&error) => match self.cached_release(repo_id, pinned).await
            {
                Ok(Some(mut cached)) => {
                    cached.notice =
                        Some(format!("{} Showing files saved earlier.", describe(&error)));
                    Ok(cached)
                }
                _ => Err(describe(&error)),
            },
            Err(error) => Err(describe(&error)),
        }
    }

    /// Read what a file's own header declares (explicit user action). A saved copy for the
    /// pinned revision is used without the network, since a revision never changes.
    /// Header failures are never cached, so a fixed bug or restored file can be retried.
    pub async fn inspect(&self, artifact: &ModelArtifact) -> Result<InspectLoad, String> {
        let Some(size) = artifact.size_bytes else {
            return Err("The file size is unknown, so its header cannot be checked.".into());
        };
        if artifact.shards.is_some() {
            return Err("This model is split into several files and is not inspected.".into());
        }
        let ModelArtifact {
            repo_id,
            revision,
            filename,
            ..
        } = artifact.clone();
        let key = (repo_id.clone(), revision.clone(), filename.clone());
        // An unreadable cache only costs a refetch.
        let saved = self
            .blocking(move |store| store.cached_artifact_metadata(&key.0, &key.1, &key.2))
            .await
            .ok()
            .flatten();
        if let Some((metadata, fetched_at_unix_ms)) = saved
            && metadata.file_bytes == size
        {
            return Ok(InspectLoad {
                metadata,
                source: LoadSource::Cached { fetched_at_unix_ms },
                notice: None,
            });
        }
        let prefix = self
            .client
            .fetch_prefix(&repo_id, &revision, &filename, size)
            .await
            .map_err(|error| describe(&error))?;
        let fetched_at = now_ms();
        let metadata = tokio::task::spawn_blocking(move || inspect_prefix(&prefix, size))
            .await
            .map_err(|_| "File inspection failed. Retry.".to_string())?
            .map_err(describe_header)?;
        let stored = metadata.clone();
        let saved = self
            .blocking(move |store| {
                store.cache_artifact_metadata(&repo_id, &revision, &filename, &stored, fetched_at)
            })
            .await;
        if matches!(&saved, Err(e) if e == REVISION_CONFLICT) {
            return Err("Hugging Face reported a different header for a file saved earlier. The saved copy was kept and the new data was not used.".into());
        }
        Ok(InspectLoad {
            metadata,
            source: LoadSource::Live,
            notice: saved.is_err().then(|| CACHE_WRITE_FAILED.to_owned()),
        })
    }

    /// Cached release only; the newest one when no commit is given.
    pub async fn cached_release(
        &self,
        repo_id: &str,
        pinned: Option<&str>,
    ) -> Result<Option<ReleaseLoad>, String> {
        let (repo, pinned) = (repo_id.to_owned(), pinned.map(str::to_owned));
        let cached = self
            .blocking(move |store| match pinned {
                Some(revision) => store.cached_release(&repo, &revision),
                None => store.cached_latest_release(&repo),
            })
            .await?;
        Ok(cached.map(|(release, fetched_at_unix_ms)| ReleaseLoad {
            release,
            source: LoadSource::Cached { fetched_at_unix_ms },
            notice: None,
        }))
    }
}

/// Header problems in terms a person can act on; never echoes file contents.
fn describe_header(error: GgufError) -> String {
    match error {
        GgufError::Unsupported("split files") => {
            "This file is one part of a split model and is not supported.".into()
        }
        GgufError::Unsupported(_) => {
            "This file uses a GGUF feature the application does not support.".into()
        }
        GgufError::Limit(_) => "The file header is larger than the application reads.".into(),
        GgufError::Invalid(_) | GgufError::Io(_) => {
            "The file header is incomplete or not valid GGUF, so its details are unavailable."
                .into()
        }
    }
}

#[cfg(test)]
mod tests;
