// SPDX-License-Identifier: GPL-3.0-or-later
//! A minimal read-only HTTP client for an externally owned runtime on IPv4 loopback.
//!
//! It can only be built for `127.0.0.1:<port>`, never uses a proxy or follows a redirect,
//! bounds every body while it is read, and returns typed errors that carry no response text.
//! No credentials are sent: a server that asks for a key is reported as such.

use std::{fmt, time::Duration};

use reqwest::{Client, redirect::Policy};
use serde::de::DeserializeOwned;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_JSON_BODY: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalError {
    /// Nothing is listening, or the connection dropped.
    Unreachable,
    Timeout,
    /// The server wants an API key; this application stores none for these runtimes.
    AuthRequired,
    Status(u16),
    BadResponse,
    TooLarge,
}

impl fmt::Display for LocalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str("Nothing answered on that port."),
            Self::Timeout => f.write_str("The runtime did not answer in time."),
            Self::AuthRequired => f.write_str(
                "The runtime asks for an API key. This application does not store keys for it.",
            ),
            Self::Status(code) => write!(f, "The runtime answered with status {code}."),
            Self::BadResponse => f.write_str("The runtime sent a response that cannot be used."),
            Self::TooLarge => {
                f.write_str("The runtime sent more data than the application accepts.")
            }
        }
    }
}

impl std::error::Error for LocalError {}

pub(crate) struct LoopbackClient {
    client: Client,
    port: u16,
}

impl LoopbackClient {
    pub(crate) fn new(port: u16, timeout: Duration) -> Result<Self, String> {
        if port == 0 {
            return Err("The port must be between 1 and 65535.".into());
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .referer(false)
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .map_err(|_| "Could not initialize the HTTP client.".to_string())?;
        Ok(Self { client, port })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub(crate) async fn get_bytes(&self, path: &str, limit: usize) -> Result<Vec<u8>, LocalError> {
        let mut response = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    LocalError::Timeout
                } else if error.is_connect() {
                    LocalError::Unreachable
                } else {
                    LocalError::BadResponse
                }
            })?;
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(LocalError::AuthRequired),
            code => return Err(LocalError::Status(code)),
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(LocalError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                LocalError::Timeout
            } else {
                LocalError::BadResponse
            }
        })? {
            if chunk.len() > limit - body.len() {
                return Err(LocalError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, LocalError> {
        let body = self.get_bytes(path, MAX_JSON_BODY).await?;
        serde_json::from_slice(&body).map_err(|_| LocalError::BadResponse)
    }
}

/// Characters that must never reach the screen from a server-supplied label: controls plus
/// the Unicode line separators and bidirectional controls, which can start a new line or
/// reorder the text around them.
pub fn display_unsafe(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// Strip unsafe characters and cut a server-supplied label to a displayable length.
pub(crate) fn clean(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|c| !display_unsafe(*c))
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_separators_and_bidi_controls_are_removed_from_labels() {
        assert_eq!(clean("a\u{2028}b\u{202E}c\u{1b}d\u{2066}e", 50), "abcde");
        assert_eq!(clean("  spaced  ", 50), "spaced");
        assert_eq!(clean("abcdef", 3), "abc");
    }
}
