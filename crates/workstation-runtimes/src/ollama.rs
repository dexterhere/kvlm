// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only discovery of an externally owned Ollama instance on IPv4 loopback.

use std::time::Duration;

use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};

pub const ENDPOINT: &str = "http://127.0.0.1:11434";
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

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let mut response = self.client.get(url).send().await.map_err(|error| {
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
