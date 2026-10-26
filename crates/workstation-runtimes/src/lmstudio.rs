// SPDX-License-Identifier: GPL-3.0-or-later
//! LM Studio as an attached, externally owned runtime: read-only discovery over its local
//! server. Nothing here starts, stops, loads or unloads anything, and no key is stored.
//!
//! Local execution cannot be proven for this runtime (it may route a request elsewhere), so
//! no benchmark may treat it as provably local without the user confirming it.

use serde::Deserialize;

use crate::local_http::{DEFAULT_TIMEOUT, LocalError, LoopbackClient, clean};

pub const DEFAULT_PORT: u16 = 1234;
/// Models read from one listing; more are reported as truncated, not silently dropped.
pub const MAX_MODELS: usize = 1000;
const MAX_LABEL: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Loaded,
    NotLoaded,
    /// The listing did not say (the OpenAI-compatible fallback does not).
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    /// `llm`, `vlm`, `embeddings` and so on, as LM Studio names it.
    pub kind: Option<String>,
    pub architecture: Option<String>,
    pub quantization: Option<String>,
    pub format: Option<String>,
    pub state: ModelState,
    pub max_context_length: Option<u64>,
    pub loaded_context_length: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `/api/v0/models`, which reports load state and context lengths.
    Native,
    /// `/v1/models`, the OpenAI-compatible list: names only.
    Compatible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub source: Source,
    pub models: Vec<Model>,
    /// More models existed than `MAX_MODELS`.
    pub truncated: bool,
}

#[derive(Deserialize)]
struct List {
    data: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    arch: Option<String>,
    quantization: Option<String>,
    compatibility_type: Option<String>,
    state: Option<String>,
    max_context_length: Option<u64>,
    loaded_context_length: Option<u64>,
}

fn label(value: Option<String>) -> Option<String> {
    value
        .map(|v| clean(&v, MAX_LABEL))
        .filter(|v| !v.is_empty())
}

fn model(raw: Raw, native: bool) -> Option<Model> {
    let id = clean(&raw.id, MAX_LABEL);
    if id.is_empty() {
        return None;
    }
    let state = match (native, raw.state.as_deref()) {
        (true, Some("loaded")) => ModelState::Loaded,
        (true, Some("not-loaded")) => ModelState::NotLoaded,
        _ => ModelState::Unknown,
    };
    Some(Model {
        id,
        kind: label(raw.kind),
        architecture: label(raw.arch),
        quantization: label(raw.quantization),
        format: label(raw.compatibility_type),
        state,
        max_context_length: raw.max_context_length.filter(|n| *n > 0),
        loaded_context_length: raw.loaded_context_length.filter(|n| *n > 0),
    })
}

pub struct LmStudioAdapter {
    client: LoopbackClient,
}

impl LmStudioAdapter {
    pub fn new(port: u16) -> Result<Self, String> {
        Ok(Self {
            client: LoopbackClient::new(port, DEFAULT_TIMEOUT)?,
        })
    }

    pub async fn discover(&self) -> Result<Discovery, LocalError> {
        match self.client.get_json::<List>("/api/v0/models").await {
            Ok(list) => Ok(build(list, Source::Native)),
            // An older LM Studio has no native listing; the compatible one still names models.
            Err(LocalError::Status(404)) => {
                let list = self.client.get_json::<List>("/v1/models").await?;
                Ok(build(list, Source::Compatible))
            }
            Err(other) => Err(other),
        }
    }
}

fn build(list: List, source: Source) -> Discovery {
    let truncated = list.data.len() > MAX_MODELS;
    let models = list
        .data
        .into_iter()
        .take(MAX_MODELS)
        .filter_map(|raw| model(raw, source == Source::Native))
        .collect();
    Discovery {
        source,
        models,
        truncated,
    }
}

#[cfg(test)]
mod tests;
