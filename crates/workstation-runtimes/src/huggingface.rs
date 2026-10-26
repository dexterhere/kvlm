// SPDX-License-Identifier: GPL-3.0-or-later
//! Public Hugging Face Hub catalog client: GGUF repository search and per-repository
//! release detail. Every request follows an explicit user action; nothing runs at startup.
//!
//! API calls never follow redirects, cap every body while streaming, and fail with typed
//! errors that carry no URLs. Downloads and tokens are separate, later phases.

use reqwest::{Client, StatusCode, Url, header, redirect::Policy};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::Semaphore;
use workstation_core::models::{
    catalog::{
        CatalogPage, RawRepoDetail, RawSibling, RepoSummary, normalize_release, valid_repo_id,
        valid_revision,
    },
    release::{GatedAccess, ModelRelease},
};

const BASE: &str = "https://huggingface.co";
const PAGE_LIMIT: usize = 30;
const MAX_REPOS_PER_PAGE: usize = 100;
const MAX_SIBLINGS: usize = 5_000;
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_QUERY: usize = 128;
const MAX_CURSOR: usize = 1024;
const MAX_RETRY_AFTER_SECS: u64 = 3600;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const OPERATION_DEADLINE: Duration = Duration::from_secs(30);

/// Why a catalog request failed. Messages are safe to show: no URLs, no response text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HfError {
    /// Could not connect; the machine may be offline.
    Offline,
    Timeout,
    RateLimited {
        retry_after_secs: Option<u64>,
    },
    /// `401`/`403`: private, gated, or requires a token.
    AccessRequired,
    NotFound,
    /// A body exceeded its cap.
    TooLarge,
    /// Unexpected status, malformed JSON, or data that failed validation.
    BadResponse,
    /// The request itself was invalid and was not sent.
    InvalidRequest,
}

impl std::fmt::Display for HfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Offline => "Cannot reach Hugging Face. Check the network connection and retry.",
            Self::Timeout => "Hugging Face did not answer in time. Retry shortly.",
            Self::RateLimited { .. } => "Hugging Face is rate limiting requests. Wait and retry.",
            Self::AccessRequired => "This repository needs an access token or accepted terms.",
            Self::NotFound => "Hugging Face has no such repository or revision.",
            Self::TooLarge => "Hugging Face sent more data than the application accepts.",
            Self::BadResponse => "Hugging Face sent a response the application cannot use.",
            Self::InvalidRequest => "The search or repository name is not valid.",
        })
    }
}
impl std::error::Error for HfError {}

pub struct HuggingFaceClient {
    client: Client,
    base: Url,
    /// One request in flight: details are fetched on demand and must not burst.
    gate: Semaphore,
}

impl HuggingFaceClient {
    pub fn new() -> Result<Self, HfError> {
        Self::build(Url::parse(BASE).map_err(|_| HfError::BadResponse)?, true)
    }

    #[cfg(test)]
    fn for_test(port: u16) -> Self {
        Self::build(
            Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
            false,
        )
        .unwrap()
    }

    fn build(base: Url, https_only: bool) -> Result<Self, HfError> {
        let mut builder = Client::builder();
        if !https_only {
            // Loopback test fixtures must never be routed through an environment proxy.
            builder = builder.no_proxy();
        }
        let client = builder
            .use_rustls_tls()
            .https_only(https_only)
            .redirect(Policy::none())
            .referer(false)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .user_agent(concat!("local-ai-workstation/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| HfError::BadResponse)?;
        Ok(Self {
            client,
            base,
            gate: Semaphore::new(1),
        })
    }

    /// Search public GGUF repositories, most downloaded first. Returns repositories only;
    /// files, sizes and checksums come from [`Self::release`].
    pub async fn search(&self, query: &str, cursor: Option<&str>) -> Result<CatalogPage, HfError> {
        let query = query.trim();
        if query.chars().count() > MAX_QUERY || query.chars().any(char::is_control) {
            return Err(HfError::InvalidRequest);
        }
        if cursor.is_some_and(|c| !valid_cursor(c)) {
            return Err(HfError::InvalidRequest);
        }
        let mut pairs = vec![
            ("filter", "gguf".to_owned()),
            ("sort", "downloads".to_owned()),
            ("direction", "-1".to_owned()),
            ("limit", PAGE_LIMIT.to_string()),
        ];
        if !query.is_empty() {
            pairs.push(("search", query.to_owned()));
        }
        if let Some(cursor) = cursor {
            pairs.push(("cursor", cursor.to_owned()));
        }
        let (headers, body) = self.get(&["api", "models"], &pairs).await?;
        let rows: Vec<Value> = serde_json::from_slice(&body).map_err(|_| HfError::BadResponse)?;
        let repos = rows
            .iter()
            .take(MAX_REPOS_PER_PAGE)
            .filter_map(|row| {
                let repo_id = row.get("id").or_else(|| row.get("modelId"))?.as_str()?;
                valid_repo_id(repo_id).then(|| RepoSummary {
                    repo_id: repo_id.to_owned(),
                    downloads: row.get("downloads").and_then(Value::as_u64),
                    likes: row.get("likes").and_then(Value::as_u64),
                    gated: row
                        .get("gated")
                        .map_or(GatedAccess::Unknown, GatedAccess::from_api),
                })
            })
            .collect();
        let next_cursor = headers
            .get(header::LINK)
            .and_then(|value| value.to_str().ok())
            .and_then(|link| self.next_cursor(link));
        Ok(CatalogPage { repos, next_cursor })
    }

    /// Fetch one repository's files. Without `pinned` the current commit is learned;
    /// with it, the response must be for exactly that commit.
    pub async fn release(
        &self,
        repo_id: &str,
        pinned: Option<&str>,
    ) -> Result<ModelRelease, HfError> {
        if !valid_repo_id(repo_id) || pinned.is_some_and(|rev| !valid_revision(rev)) {
            return Err(HfError::InvalidRequest);
        }
        let (owner, name) = repo_id.split_once('/').ok_or(HfError::InvalidRequest)?;
        let mut path = vec!["api", "models", owner, name];
        if let Some(revision) = pinned {
            path.extend(["revision", revision]);
        }
        let (_, body) = self.get(&path, &[("blobs", "true".to_owned())]).await?;
        let detail: Value = serde_json::from_slice(&body).map_err(|_| HfError::BadResponse)?;
        let raw = parse_detail(&detail, repo_id, pinned)?;
        normalize_release(raw).map_err(|_| HfError::BadResponse)
    }

    /// The `cursor` of a `rel="next"` link, only if the link points at this API's origin.
    /// The next request is rebuilt from the cursor, never from the link.
    fn next_cursor(&self, link: &str) -> Option<String> {
        link.split(',').find_map(|part| {
            let (target, params) = part.split_once('>')?;
            if !params.replace(' ', "").contains("rel=\"next\"") {
                return None;
            }
            let url = Url::parse(target.trim().strip_prefix('<')?).ok()?;
            let same_origin = url.scheme() == self.base.scheme()
                && url.host_str() == self.base.host_str()
                && url.port_or_known_default() == self.base.port_or_known_default();
            let cursor = url
                .query_pairs()
                .find(|(key, _)| key == "cursor")?
                .1
                .into_owned();
            (same_origin && valid_cursor(&cursor)).then_some(cursor)
        })
    }

    /// GET under the single-flight gate and the overall deadline.
    async fn get(
        &self,
        segments: &[&str],
        pairs: &[(&str, String)],
    ) -> Result<(header::HeaderMap, Vec<u8>), HfError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| HfError::InvalidRequest)?
            .pop_if_empty()
            .extend(segments);
        url.query_pairs_mut().extend_pairs(pairs);
        // The deadline covers waiting for the single-flight permit as well as the request.
        tokio::time::timeout(OPERATION_DEADLINE, async {
            let _permit = self
                .gate
                .acquire()
                .await
                .map_err(|_| HfError::BadResponse)?;
            let response = self
                .client
                .get(url)
                .header(header::ACCEPT, "application/json")
                .send()
                .await
                .map_err(map_transport)?;
            match response.status() {
                StatusCode::OK => {}
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    return Err(HfError::AccessRequired);
                }
                StatusCode::NOT_FOUND => return Err(HfError::NotFound),
                StatusCode::TOO_MANY_REQUESTS => {
                    return Err(HfError::RateLimited {
                        retry_after_secs: retry_after(response.headers()),
                    });
                }
                _ => return Err(HfError::BadResponse),
            }
            let headers = response.headers().clone();
            Ok((headers, read_capped(response).await?))
        })
        .await
        .map_err(|_| HfError::Timeout)?
    }
}

fn map_transport(error: reqwest::Error) -> HfError {
    if error.is_timeout() {
        HfError::Timeout
    } else if error.is_connect() {
        HfError::Offline
    } else {
        HfError::BadResponse
    }
}

/// Seconds from a numeric `Retry-After`, capped. Date forms are treated as unknown.
fn retry_after(headers: &header::HeaderMap) -> Option<u64> {
    let seconds = headers
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(seconds.min(MAX_RETRY_AFTER_SECS))
}

/// Stream a body, refusing more than `MAX_BODY` bytes whatever the headers claim.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, HfError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY as u64)
    {
        return Err(HfError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_transport)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(HfError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Opaque pagination cursors are URL-safe tokens; anything else is not forwarded.
fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty()
        && cursor.len() <= MAX_CURSOR
        && cursor.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'=' | b'+' | b'/')
        })
}

/// Displayed text from a remote source: bounded, single line, no control characters.
fn short_text(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn first_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(text) => Some(text),
        Value::Array(items) => items.iter().find_map(Value::as_str),
        _ => None,
    }
}

fn parse_detail(
    detail: &Value,
    requested: &str,
    pinned: Option<&str>,
) -> Result<RawRepoDetail, HfError> {
    let id = detail
        .get("id")
        .and_then(Value::as_str)
        .ok_or(HfError::BadResponse)?;
    if !id.eq_ignore_ascii_case(requested) {
        return Err(HfError::BadResponse);
    }
    let revision = detail
        .get("sha")
        .and_then(Value::as_str)
        .ok_or(HfError::BadResponse)?;
    if pinned.is_some_and(|pin| pin != revision) {
        return Err(HfError::BadResponse);
    }
    let card = detail.get("cardData");
    let tags: Vec<&str> = detail
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| tags.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let license = card
        .and_then(|c| c.get("license"))
        .and_then(first_string)
        .or_else(|| tags.iter().find_map(|tag| tag.strip_prefix("license:")))
        .and_then(|text| short_text(text, 64));
    let base_model = card
        .and_then(|c| c.get("base_model"))
        .and_then(first_string)
        .or_else(|| {
            tags.iter()
                .find(|tag| tag.starts_with("base_model:"))
                .and_then(|tag| tag.rsplit(':').next())
        })
        .and_then(|text| short_text(text, 200));
    let siblings = detail
        .get("siblings")
        .and_then(Value::as_array)
        .ok_or(HfError::BadResponse)?;
    if siblings.len() > MAX_SIBLINGS {
        return Err(HfError::TooLarge);
    }
    let siblings = siblings
        .iter()
        .filter_map(|file| {
            let lfs = file.get("lfs");
            Some(RawSibling {
                filename: file.get("rfilename")?.as_str()?.to_owned(),
                size: file
                    .get("size")
                    .and_then(Value::as_u64)
                    .or_else(|| lfs?.get("size")?.as_u64()),
                lfs_sha256: lfs
                    .and_then(|l| l.get("sha256").or_else(|| l.get("oid")))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect();
    Ok(RawRepoDetail {
        repo_id: id.to_owned(),
        revision: revision.to_owned(),
        license,
        gated: detail
            .get("gated")
            .map_or(GatedAccess::Unknown, GatedAccess::from_api),
        base_model,
        siblings,
    })
}

#[cfg(test)]
mod tests;
