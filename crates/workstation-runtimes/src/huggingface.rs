// SPDX-License-Identifier: GPL-3.0-or-later
//! Public Hugging Face Hub catalog client: GGUF repository search and per-repository
//! release detail. Every request follows an explicit user action; nothing runs at startup.
//!
//! API calls never follow redirects, cap every body while streaming, and fail with typed
//! errors that carry no URLs. Downloads and tokens are separate, later phases.

use reqwest::{Client, StatusCode, Url, header, redirect::Policy};
use serde_json::Value;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use workstation_core::models::{
    artifact::valid_remote_filename,
    catalog::{
        CatalogPage, RawRepoDetail, RawSibling, RepoSummary, normalize_release, valid_repo_id,
        valid_revision,
    },
    gguf::HEADER_LIMIT,
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
const PREFIX_DEADLINE: Duration = Duration::from_secs(120);
const MAX_HOPS: usize = 4;

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
    /// Production only talks https to Hugging Face hosts; test fixtures use loopback.
    strict_hosts: bool,
}

impl HuggingFaceClient {
    pub fn new() -> Result<Self, HfError> {
        Self::build(Url::parse(BASE).map_err(|_| HfError::BadResponse)?, true)
    }

    #[cfg(test)]
    pub(crate) fn for_test(port: u16) -> Self {
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
        if https_only {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
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
            strict_hosts: https_only,
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

    /// The first bytes of one GGUF file at a pinned commit, for header inspection only.
    ///
    /// One ranged request (redirects are followed by hand, at most four hops, only to
    /// Hugging Face hosts) whose total length must equal `expected_size`. Returns
    /// `min(expected_size, HEADER_LIMIT)` bytes; anything else is a bad response.
    pub async fn fetch_prefix(
        &self,
        repo_id: &str,
        revision: &str,
        filename: &str,
        expected_size: u64,
    ) -> Result<Vec<u8>, HfError> {
        if !valid_repo_id(repo_id)
            || !valid_revision(revision)
            || !valid_remote_filename(filename)
            || expected_size == 0
        {
            return Err(HfError::InvalidRequest);
        }
        let want = expected_size.min(HEADER_LIMIT);
        let url = self.resolve_url(repo_id, revision, filename)?;
        tokio::time::timeout(PREFIX_DEADLINE, async {
            let _permit = self
                .gate
                .acquire()
                .await
                .map_err(|_| HfError::BadResponse)?;
            let response = self.follow(url, format!("bytes=0-{}", want - 1)).await?;
            match response.status() {
                StatusCode::PARTIAL_CONTENT => {
                    if content_range(&response) != Some((0, want - 1, expected_size)) {
                        return Err(HfError::BadResponse);
                    }
                }
                // A server that ignored the range: acceptable only for the right file.
                StatusCode::OK if response.content_length() == Some(expected_size) => {}
                status => return Err(status_error(status, &response)),
            }
            read_exactly(response, want).await
        })
        .await
        .map_err(|_| HfError::Timeout)?
    }

    /// Open a stream of one GGUF file at a pinned commit, from byte `offset`.
    ///
    /// `206` must start at `offset` and end at the last byte of a file whose total equals
    /// `expected_size`; a `200` restarts from zero (the stream says so in `start`). It does
    /// not use the single-flight gate: a long transfer must not block catalog browsing.
    pub async fn open_download(
        &self,
        repo_id: &str,
        revision: &str,
        filename: &str,
        expected_size: u64,
        offset: u64,
    ) -> Result<DownloadStream, HfError> {
        if !valid_repo_id(repo_id)
            || !valid_revision(revision)
            || !valid_remote_filename(filename)
            || expected_size == 0
            || offset >= expected_size
        {
            return Err(HfError::InvalidRequest);
        }
        let url = self.resolve_url(repo_id, revision, filename)?;
        let response = self.follow(url, format!("bytes={offset}-")).await?;
        let start = match response.status() {
            StatusCode::PARTIAL_CONTENT => {
                if content_range(&response) != Some((offset, expected_size - 1, expected_size)) {
                    return Err(HfError::BadResponse);
                }
                offset
            }
            StatusCode::OK if response.content_length() == Some(expected_size) => 0,
            status => return Err(status_error(status, &response)),
        };
        Ok(DownloadStream { response, start })
    }

    fn resolve_url(&self, repo_id: &str, revision: &str, filename: &str) -> Result<Url, HfError> {
        let (owner, name) = repo_id.split_once('/').ok_or(HfError::InvalidRequest)?;
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| HfError::InvalidRequest)?
            .pop_if_empty()
            .extend([owner, name, "resolve", revision])
            .extend(filename.split('/'));
        Ok(url)
    }

    /// GET with a `Range`, following redirects by hand (at most four hops, only to allowed
    /// hosts). Returns the first non-redirect response; the caller checks its status.
    async fn follow(&self, mut url: Url, range: String) -> Result<reqwest::Response, HfError> {
        for _ in 0..=MAX_HOPS {
            let response = self
                .client
                .get(url.clone())
                .header(header::RANGE, &range)
                .header(header::ACCEPT_ENCODING, "identity")
                .send()
                .await
                .map_err(map_transport)?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let next = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .ok_or(HfError::BadResponse)?;
            if !self.redirect_allowed(&next) {
                return Err(HfError::BadResponse);
            }
            url = next;
        }
        Err(HfError::BadResponse)
    }

    /// Redirect targets must be https Hugging Face hosts on the default port, with no
    /// credentials in the URL. Loopback fixtures may only stay on the test origin.
    fn redirect_allowed(&self, url: &Url) -> bool {
        if !self.strict_hosts {
            return url.host_str() == self.base.host_str()
                && url.port_or_known_default() == self.base.port_or_known_default();
        }
        let Some(host) = url.domain() else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.port(), None | Some(443))
            && ["huggingface.co", "hf.co"]
                .iter()
                .any(|root| host == *root || host.ends_with(&format!(".{root}")))
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

/// Resolver that refuses to hand out loopback, private, link-local or otherwise
/// non-public addresses, so an allowed host name cannot be pointed at the local network.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err("no public address".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => public_v4(mapped),
            None => public_v6(ip),
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
        || (a == 192 && b == 0) // IETF protocol assignments
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || a >= 240) // reserved
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local
        || (first & 0xffc0) == 0xfe80 // link local
        || (first == 0x2001 && ip.segments()[1] == 0x0db8)) // documentation
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

/// The body of an open download; `start` is the offset its first byte belongs at.
pub struct DownloadStream {
    response: reqwest::Response,
    pub start: u64,
}

impl DownloadStream {
    /// The next piece of the body, or `None` at the end. Errors carry no URLs.
    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, HfError> {
        Ok(self
            .response
            .chunk()
            .await
            .map_err(map_transport)?
            .map(|c| c.to_vec()))
    }
}

fn content_range(response: &reqwest::Response) -> Option<(u64, u64, u64)> {
    response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range)
}

fn status_error(status: StatusCode, response: &reqwest::Response) -> HfError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => HfError::AccessRequired,
        StatusCode::NOT_FOUND => HfError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => HfError::RateLimited {
            retry_after_secs: retry_after(response.headers()),
        },
        _ => HfError::BadResponse,
    }
}

/// `(first, last, total)` of a `Content-Range: bytes first-last/total` header.
fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?, total.parse().ok()?))
}

/// Read exactly `want` bytes and stop, so a server that sends the whole file costs no more.
async fn read_exactly(mut response: reqwest::Response, want: u64) -> Result<Vec<u8>, HfError> {
    let mut body = Vec::with_capacity(want as usize);
    while (body.len() as u64) < want {
        let Some(chunk) = response.chunk().await.map_err(map_transport)? else {
            return Err(HfError::BadResponse);
        };
        let room = (want - body.len() as u64) as usize;
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
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
        // The caller's spelling keeps cache keys and offline lookups consistent.
        repo_id: requested.to_owned(),
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
