// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only discovery of an externally owned Ollama instance on IPv4 loopback.

use std::time::Duration;

use reqwest::{Client, RequestBuilder, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};

pub const ENDPOINT: &str = "http://127.0.0.1:11434";
mod generation;
pub use generation::{GenerationProgress, GenerationRequest, GenerationSummary};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Discovery {
    pub version: String,
    pub installed: Result<Vec<InstalledModel>, String>,
    pub running: Result<Vec<RunningModel>, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct InstalledModel {
    pub name: String,
    pub digest: Option<String>,
    pub size: Option<u64>,
    pub details: Option<ModelDetails>,
    pub remote_model: Option<String>,
    pub remote_host: Option<String>,
}

impl InstalledModel {
    /// Missing remote metadata is not proof of local execution.
    pub fn cloud_routed(&self) -> bool {
        [&self.remote_model, &self.remote_host]
            .into_iter()
            .flatten()
            .any(|value| !value.trim().is_empty())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelDetails {
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RunningModel {
    pub name: String,
    pub digest: Option<String>,
    pub size: Option<u64>,
    pub size_vram: Option<u64>,
    pub context_length: Option<u64>,
}

impl RunningModel {
    /// Ollama's allocation remainder, not an independent measurement of process RSS.
    pub fn cpu_bytes(&self) -> Option<u64> {
        self.size?.checked_sub(self.size_vram?)
    }
}

/// Declarations returned by `/api/show`, never proof of loaded runtime settings.
#[derive(Clone, Debug, Deserialize)]
pub struct ModelMetadata {
    pub details: Option<ModelDetails>,
    pub capabilities: Option<Vec<String>>,
    pub parameters: Option<String>,
    pub model_info: Option<serde_json::Map<String, serde_json::Value>>,
    pub remote_model: Option<String>,
    pub remote_host: Option<String>,
}

impl ModelMetadata {
    pub fn architecture(&self) -> Option<&str> {
        self.model_info
            .as_ref()?
            .get("general.architecture")?
            .as_str()
            .filter(|value| !value.trim().is_empty())
    }

    /// The model's declared training context, not the context allocated by Ollama.
    pub fn declared_context_length(&self) -> Option<u64> {
        self.model_info
            .as_ref()?
            .get(&format!("{}.context_length", self.architecture()?))?
            .as_u64()
            .filter(|value| *value > 0)
    }

    pub fn cloud_routed(&self) -> bool {
        [&self.remote_model, &self.remote_host]
            .into_iter()
            .flatten()
            .any(|value| !value.trim().is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct ModelInspection {
    pub metadata: Result<ModelMetadata, String>,
    pub running: Result<Vec<RunningModel>, String>,
}

#[derive(Deserialize)]
struct Version {
    version: String,
}

#[derive(Deserialize)]
struct Models<T> {
    models: Vec<T>,
}

pub struct OllamaAdapter {
    client: Client,
    port: u16,
}

impl OllamaAdapter {
    pub fn new() -> Result<Self, String> {
        Self::with_port(11434, Duration::from_secs(5))
    }

    fn with_port(port: u16, timeout: Duration) -> Result<Self, String> {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .map_err(|_| "Could not initialize the Ollama HTTP client".to_string())?;
        Ok(Self { client, port })
    }

    pub async fn discover(&self) -> Result<Discovery, String> {
        let version = self.get::<Version>("/api/version").await?.version;
        if version.trim().is_empty() {
            return Err("/api/version: empty version".into());
        }
        let (installed, running) = tokio::join!(
            self.get::<Models<InstalledModel>>("/api/tags"),
            self.get::<Models<RunningModel>>("/api/ps"),
        );
        Ok(Discovery {
            version,
            installed: installed.and_then(|list| {
                if list.models.iter().any(|model| model.name.trim().is_empty()) {
                    Err("/api/tags: empty model name".into())
                } else {
                    Ok(list.models)
                }
            }),
            running: running.and_then(|list| {
                if list.models.iter().any(|model| model.name.trim().is_empty()) {
                    Err("/api/ps: empty model name".into())
                } else {
                    Ok(list.models)
                }
            }),
        })
    }

    /// Inspect declarations and take a fresh loaded-model snapshot without loading a model.
    pub async fn inspect_model(&self, model: &str) -> Result<ModelInspection, String> {
        if model.trim().is_empty() || model.len() > 1024 || model.chars().any(char::is_control) {
            return Err("/api/show: invalid model name".into());
        }
        let body = serde_json::to_vec(&serde_json::json!({"model": model, "verbose": false}))
            .map_err(|_| "/api/show: could not encode model name".to_string())?;
        let request = self
            .client
            .post(self.url("/api/show"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        let (metadata, running) = tokio::join!(
            self.request::<ModelMetadata>("/api/show", request),
            self.get::<Models<RunningModel>>("/api/ps"),
        );
        Ok(ModelInspection {
            metadata,
            running: running.and_then(|list| {
                if list.models.iter().any(|model| model.name.trim().is_empty()) {
                    Err("/api/ps: empty model name".into())
                } else {
                    Ok(list.models)
                }
            }),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.request(path, self.client.get(self.url(path))).await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        path: &str,
        request: RequestBuilder,
    ) -> Result<T, String> {
        let mut response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                format!("{path}: request timed out")
            } else if error.is_connect() {
                format!("{path}: endpoint unavailable; check your existing Ollama service")
            } else {
                format!("{path}: HTTP request failed")
            }
        })?;
        if !response.status().is_success() {
            return Err(format!("{path}: HTTP {}", response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(format!("{path}: response exceeds 2 MiB limit"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                format!("{path}: response timed out")
            } else {
                format!("{path}: response could not be read")
            }
        })? {
            if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                return Err(format!("{path}: response exceeds 2 MiB limit"));
            }
            body.extend_from_slice(&chunk);
        }
        // Do not expose server-controlled response bodies in errors or logs.
        serde_json::from_slice(&body)
            .map_err(|_| format!("{path}: invalid JSON or response fields"))
    }
}

#[cfg(test)]
mod tests;
