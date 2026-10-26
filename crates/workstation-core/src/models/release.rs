// SPDX-License-Identifier: GPL-3.0-or-later
//! A repository at one immutable revision: the middle tier of the browser.

use super::{artifact::ModelArtifact, family::ModelFamily};
use serde::{Deserialize, Serialize};

/// Whether the repository requires accepting terms before files can be fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatedAccess {
    Open,
    /// Access is granted after accepting terms.
    Automatic,
    /// A maintainer approves each request.
    Manual,
    Unknown,
}

impl GatedAccess {
    /// Hugging Face reports `false`, `"auto"`, or `"manual"`.
    pub fn from_api(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Bool(false) => Self::Open,
            serde_json::Value::String(text) if text == "auto" => Self::Automatic,
            serde_json::Value::String(text) if text == "manual" => Self::Manual,
            _ => Self::Unknown,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Automatic => "automatic",
            Self::Manual => "manual",
            Self::Unknown => "unknown",
        }
    }
    pub fn from_stored(text: &str) -> Self {
        match text {
            "open" => Self::Open,
            "automatic" => Self::Automatic,
            "manual" => Self::Manual,
            _ => Self::Unknown,
        }
    }
}

/// One repository at a pinned commit, with the artifacts it offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRelease {
    pub repo_id: String,
    pub revision: String,
    pub family: ModelFamily,
    pub license: Option<String>,
    pub gated: GatedAccess,
    pub artifacts: Vec<ModelArtifact>,
    /// Entries dropped for invalid names or identifiers.
    pub dropped_invalid: u32,
    /// Safetensors, PyTorch, ONNX and similar files: counted, not offered.
    pub other_weight_files: u32,
}
