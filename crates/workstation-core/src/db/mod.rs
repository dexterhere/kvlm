// SPDX-License-Identifier: GPL-3.0-or-later
//! Versioned, local-only benchmark history. Call from a blocking worker.

use crate::hardware::HardwareProfile;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub mod catalog_cache;
pub mod digests;
pub mod downloads;
pub mod queries;
pub mod schema;
pub use queries::HistoryStore;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub snapshot: RunSnapshot,
    pub completion: Option<RunCompletion>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub started_at_unix_ms: u64,
    pub model_name: String,
    pub model_digest: Option<String>,
    pub model_size_bytes: Option<u64>,
    pub hardware: Option<HardwareProfile>,
    pub hardware_notice: Option<String>,
    pub runtime_endpoint: String,
    pub workload: WorkloadSnapshot,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkloadSnapshot {
    pub id: String,
    pub prompt_sha256: String,
    pub num_predict: u32,
    pub num_ctx: u32,
    pub seed: u32,
    pub temperature: u32,
    pub think: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunCompletion {
    pub finished_at_unix_ms: u64,
    pub runtime_version: Option<String>,
    pub model_digest: Option<String>,
    pub outcome: RunOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<crate::hardware::resources::ResourceSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RunOutcome {
    Completed(RunMetrics),
    Cancelled(PartialMetrics),
    TimedOut(PartialMetrics),
    Failed {
        message: String,
        partial: PartialMetrics,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartialMetrics {
    pub first_response_ns: Option<u64>,
    pub response_chunks: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunMetrics {
    pub first_response_ns: Option<u64>,
    pub elapsed_ns: u64,
    pub response_chunks: u64,
    pub eval_count: Option<u64>,
    pub eval_duration_ns: Option<u64>,
    pub load_duration_ns: Option<u64>,
    pub done_reason: String,
}

pub fn default_database_path() -> Result<PathBuf, String> {
    data_path(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}
/// Where verified model files live: beside the database, under the application data directory.
pub fn default_models_dir() -> Result<PathBuf, String> {
    let database = default_database_path()?;
    database
        .parent()
        .map(|dir| dir.join("models"))
        .ok_or_else(|| "The application data directory is unavailable.".to_string())
}

fn data_path(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    let root = xdg
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .map(Path::to_path_buf)
        .or_else(|| {
            home.map(Path::new)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".local/share"))
        })
        .ok_or("Set an absolute XDG_DATA_HOME or HOME to store benchmark history.")?;
    Ok(root.join("local-ai-workstation/workstation.db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xdg_paths_never_fall_back_to_the_working_directory() {
        assert_eq!(
            data_path(Some(OsStr::new("/data")), None).unwrap(),
            PathBuf::from("/data/local-ai-workstation/workstation.db")
        );
        for xdg in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert_eq!(
                data_path(xdg, Some(OsStr::new("/home/test"))).unwrap(),
                PathBuf::from("/home/test/.local/share/local-ai-workstation/workstation.db")
            );
            assert!(data_path(xdg, Some(OsStr::new("relative"))).is_err());
        }
    }
}
