// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in update check: one HTTPS request to the project's public release feed, made only
//! when the user presses the button. Nothing is downloaded or installed; the answer is a
//! version number and a link the user may open themselves. No identifier, version or other
//! data about this machine is sent beyond what any HTTPS request carries.

use reqwest::{Client, header, redirect::Policy};
use std::{cmp::Ordering, time::Duration};

const FEED: &str = "https://api.github.com/repos/dexterhere/kvlm/releases/latest";
const RELEASES: &str = "https://github.com/dexterhere/kvlm/releases/tag/";
const MAX_BODY: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// GitHub requires a user agent. It names the program only, never the installed version.
const USER_AGENT: &str = "local-ai-workstation";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    Offline,
    Timeout,
    Status(u16),
    BadResponse,
    TooLarge,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => f.write_str("Cannot reach GitHub. Check the network connection."),
            Self::Timeout => f.write_str("GitHub did not answer in time."),
            Self::Status(404) => f.write_str("No release has been published yet."),
            Self::Status(code) => write!(f, "GitHub answered with status {code}."),
            Self::BadResponse => f.write_str("GitHub sent a response that cannot be used."),
            Self::TooLarge => f.write_str("GitHub sent more data than the application accepts."),
        }
    }
}

impl std::error::Error for UpdateError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate { latest: String },
    Newer { latest: String, link: String },
}

/// `1.2.3` or `v1.2.3`, digits only. Pre-release or build suffixes are not versions we offer.
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

pub async fn check(current: &str) -> Result<Outcome, UpdateError> {
    check_at(FEED, current).await
}

async fn check_at(url: &str, current: &str) -> Result<Outcome, UpdateError> {
    let installed = parse_version(current).ok_or(UpdateError::BadResponse)?;
    let client = Client::builder()
        .redirect(Policy::none())
        .referer(false)
        .timeout(TIMEOUT)
        .connect_timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|_| UpdateError::BadResponse)?;
    let mut response = client
        .get(url)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                UpdateError::Timeout
            } else if error.is_connect() {
                UpdateError::Offline
            } else {
                UpdateError::BadResponse
            }
        })?;
    if !response.status().is_success() {
        return Err(UpdateError::Status(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(UpdateError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| UpdateError::BadResponse)?
    {
        if chunk.len() > MAX_BODY - body.len() {
            return Err(UpdateError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| UpdateError::BadResponse)?;
    let tag = value
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or(UpdateError::BadResponse)?;
    let latest = parse_version(tag).ok_or(UpdateError::BadResponse)?;
    let latest_text = tag.strip_prefix('v').unwrap_or(tag).to_string();
    Ok(match latest.cmp(&installed) {
        Ordering::Greater => Outcome::Newer {
            // The link is built from the validated tag, never taken from the response.
            link: format!("{RELEASES}{tag}"),
            latest: latest_text,
        },
        _ => Outcome::UpToDate {
            latest: latest_text,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{Server, json_reply, route, status};
    use serde_json::json;

    const FEED_LINE: &str = "GET /feed HTTP/1.1";

    async fn run(reply: crate::test_server::Reply, current: &str) -> Result<Outcome, UpdateError> {
        let server = Server::start(move |_| vec![route(FEED_LINE, reply.clone())]);
        check_at(&format!("http://127.0.0.1:{}/feed", server.port()), current).await
    }

    #[test]
    fn the_user_agent_does_not_reveal_the_installed_version() {
        assert!(!USER_AGENT.chars().any(|c| c.is_ascii_digit() || c == '/'));
    }

    #[test]
    fn only_plain_numeric_versions_parse() {
        assert_eq!(parse_version("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("0.10.0"), Some([0, 10, 0]));
        for bad in [
            "1.2",
            "1.2.3.4",
            "1.2.3-rc1",
            "v",
            "1..3",
            "1.2.x",
            "9999999999.0.0",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn a_newer_tag_gives_a_link_built_from_the_tag_not_the_response() {
        let reply = json_reply(json!({"tag_name":"v0.2.0","html_url":"https://evil.example/x"}));
        assert_eq!(
            run(reply, "0.1.0").await,
            Ok(Outcome::Newer {
                latest: "0.2.0".into(),
                link: "https://github.com/dexterhere/kvlm/releases/tag/v0.2.0".into()
            })
        );
    }

    #[tokio::test]
    async fn equal_older_and_odd_answers_never_offer_an_update() {
        let same = json_reply(json!({"tag_name":"v0.1.0"}));
        assert!(matches!(
            run(same, "0.1.0").await,
            Ok(Outcome::UpToDate { .. })
        ));
        let older = json_reply(json!({"tag_name":"0.0.9"}));
        assert!(matches!(
            run(older, "0.1.0").await,
            Ok(Outcome::UpToDate { .. })
        ));
        let pre = json_reply(json!({"tag_name":"v9.0.0-rc1"}));
        assert_eq!(run(pre, "0.1.0").await, Err(UpdateError::BadResponse));
        let missing = json_reply(json!({"name":"x"}));
        assert_eq!(run(missing, "0.1.0").await, Err(UpdateError::BadResponse));
        assert_eq!(
            run(status(404), "0.1.0").await,
            Err(UpdateError::Status(404))
        );
        assert_eq!(
            run(crate::test_server::Reply::DeclaredOversize, "0.1.0").await,
            Err(UpdateError::TooLarge)
        );
    }
}
