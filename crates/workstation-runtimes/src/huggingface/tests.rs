// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::*;

const SEARCH_LINE: &str =
    "GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30 HTTP/1.1";
const SEARCH_QWEN_LINE: &str =
    "GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30&search=qwen+coder HTTP/1.1";
const DETAIL_LINE: &str = "GET /api/models/owner/name?blobs=true HTTP/1.1";
const REV: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_REV: &str = "fedcba9876543210fedcba9876543210fedcba98";
const SHA256: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

#[derive(Clone)]
enum Reply {
    Json {
        body: String,
        headers: Vec<(String, String)>,
    },
    Status {
        code: u16,
        headers: Vec<(String, String)>,
    },
    Redirect(String),
    /// Declares 5 MiB, sends a few bytes, then closes.
    DeclaredOversize,
    /// No length header; streams more than the 4 MiB cap, then closes.
    StreamedOversize,
}

fn json_reply(value: Value) -> Reply {
    Reply::Json {
        body: value.to_string(),
        headers: Vec::new(),
    }
}

fn status(code: u16) -> Reply {
    Reply::Status {
        code,
        headers: Vec::new(),
    }
}

fn with_header(code: u16, name: &str, value: &str) -> Reply {
    Reply::Status {
        code,
        headers: vec![(name.to_owned(), value.to_owned())],
    }
}

fn route(line: &str, reply: Reply) -> (String, Reply) {
    (line.to_owned(), reply)
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn head(code: u16, headers: &[(String, String)], length: Option<usize>) -> String {
    let mut text = format!("HTTP/1.1 {code} {}\r\n", reason(code));
    if let Some(length) = length {
        text += &format!("Content-Length: {length}\r\n");
    }
    for (name, value) in headers {
        text += &format!("{name}: {value}\r\n");
    }
    text += "Connection: close\r\n\r\n";
    text
}

impl Reply {
    fn write(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        match self {
            Self::Json { body, headers } => {
                let mut all = vec![("Content-Type".to_owned(), "application/json".to_owned())];
                all.extend(headers.iter().cloned());
                stream.write_all(head(200, &all, Some(body.len())).as_bytes())?;
                stream.write_all(body.as_bytes())
            }
            Self::Status { code, headers } => {
                stream.write_all(head(*code, headers, Some(0)).as_bytes())
            }
            Self::Redirect(location) => {
                let headers = [("Location".to_owned(), location.clone())];
                stream.write_all(head(302, &headers, Some(0)).as_bytes())
            }
            Self::DeclaredOversize => {
                let headers = [("Content-Type".to_owned(), "application/json".to_owned())];
                stream.write_all(head(200, &headers, Some(5 * 1024 * 1024)).as_bytes())?;
                stream.write_all(b"[]")
            }
            Self::StreamedOversize => {
                let headers = [("Content-Type".to_owned(), "application/json".to_owned())];
                stream.write_all(head(200, &headers, None).as_bytes())?;
                let chunk = vec![b' '; 64 * 1024];
                // 66 chunks is 4.125 MiB, past the 4 MiB cap. Stop early if the client hangs up.
                for _ in 0..66 {
                    if stream.write_all(&chunk).is_err() {
                        break;
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    line: String,
    /// Header lines as sent, one per line, lowercased names.
    headers: Vec<String>,
}

struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Routes are built with the bound port, so a `Link` header can point back at this server.
    fn start(build: impl FnOnce(u16) -> Vec<(String, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let routes = build(port);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorded = Arc::clone(&requests);
        let stopping = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(4);
            while !stopping.load(Ordering::Relaxed) && Instant::now() < deadline {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("mock listener failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut byte = [0];
                while raw.len() < 8192 && !raw.ends_with(b"\r\n\r\n") && Instant::now() < deadline {
                    match std::io::Read::read(&mut stream, &mut byte) {
                        Ok(1) => raw.push(byte[0]),
                        _ => break,
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let line = text.lines().next().unwrap_or("").to_owned();
                let headers = text
                    .lines()
                    .skip(1)
                    .map(|header| header.to_ascii_lowercase())
                    .collect();
                recorded.lock().unwrap().push(RecordedRequest {
                    line: line.clone(),
                    headers,
                });
                let reply = routes
                    .iter()
                    .find(|(expected, _)| *expected == line)
                    .map_or_else(|| status(404), |(_, reply)| reply.clone());
                // Clients may hang up early in the oversize tests; write errors are expected.
                let _ = reply.write(&mut stream);
            }
        });
        Self {
            port,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn client(&self) -> HuggingFaceClient {
        HuggingFaceClient::for_test(self.port)
    }

    fn lines(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.line.clone())
            .collect()
    }

    fn last(&self) -> RecordedRequest {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request was recorded")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn search_row(id: &str) -> Value {
    json!({ "id": id, "downloads": 5, "likes": 2, "gated": false })
}

fn detail_with(id: &str, sha: &str, card: Value, tags: Value, siblings: Value) -> Value {
    json!({
        "id": id,
        "sha": sha,
        "gated": "auto",
        "cardData": card,
        "tags": tags,
        "siblings": siblings,
    })
}

fn qwen_detail(id: &str, sha: &str) -> Value {
    detail_with(
        id,
        sha,
        json!({ "license": "apache-2.0", "base_model": "Qwen/Qwen2.5-Coder-7B-Instruct" }),
        json!([]),
        json!([
            { "rfilename": "m-Q4_K_M.gguf", "size": 4683073536u64,
              "lfs": { "sha256": SHA256, "size": 4683073536u64 } },
            { "rfilename": "README.md", "size": 3 },
            { "rfilename": "model.safetensors", "size": 9 }
        ]),
    )
}

async fn search_with(reply: Reply) -> Result<CatalogPage, HfError> {
    let server = Server::start(|_| vec![route(SEARCH_LINE, reply)]);
    server.client().search("", None).await
}

#[tokio::test]
async fn search_builds_the_expected_query_and_lists_valid_repos_only() {
    let server = Server::start(|_| {
        vec![route(
            SEARCH_QWEN_LINE,
            json_reply(json!([
                search_row("a/b"),
                { "modelId": "c/d" },
                { "id": "../bad" },
                { "id": "x" },
            ])),
        )]
    });
    let page = server.client().search("qwen coder", None).await.unwrap();

    let ids: Vec<_> = page
        .repos
        .iter()
        .map(|repo| repo.repo_id.as_str())
        .collect();
    assert_eq!(ids, ["a/b", "c/d"]);
    let first = &page.repos[0];
    assert_eq!(first.downloads, Some(5));
    assert_eq!(first.likes, Some(2));
    assert_eq!(first.gated, GatedAccess::Open);
    assert_eq!(page.repos[1].gated, GatedAccess::Unknown);

    let line = server.last().line;
    assert!(line.starts_with("GET /api/models?filter=gguf&sort=downloads&direction=-1&limit=30"));
    assert!(line.contains("search=qwen+coder"), "got {line}");

    let headers = server.last().headers;
    assert!(
        headers.iter().any(|h| h == "accept: application/json"),
        "{headers:?}"
    );
    assert!(
        headers
            .iter()
            .any(|h| h.starts_with("user-agent: local-ai-workstation/"))
    );
    for forbidden in ["authorization:", "cookie:", "referer:"] {
        assert!(
            !headers.iter().any(|h| h.starts_with(forbidden)),
            "{forbidden} sent: {headers:?}"
        );
    }
}

#[test]
fn next_cursor_accepts_only_same_origin_next_links_with_valid_cursors() {
    let client = HuggingFaceClient::for_test(4242);
    let same = "http://127.0.0.1:4242";
    let next = |target: &str| format!("<{target}>; rel=\"next\"");

    assert_eq!(
        client.next_cursor(&next(&format!(
            "{same}/api/models?filter=gguf&cursor=abc123"
        ))),
        Some("abc123".to_owned())
    );
    assert_eq!(
        client.next_cursor(&next("http://evil.example/api/models?cursor=x")),
        None
    );
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor=a%20b"))),
        None
    );
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor=a%3Cb"))),
        None
    );
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor=a%3Eb"))),
        None
    );
    let too_long = "a".repeat(1025);
    assert_eq!(
        client.next_cursor(&next(&format!("{same}/api/models?cursor={too_long}"))),
        None
    );
    assert_eq!(
        client.next_cursor(&format!("<{same}/api/models?cursor=abc>; rel=\"prev\"")),
        None
    );
}

#[tokio::test]
async fn search_follows_link_cursor_only_for_same_origin() {
    let server = Server::start(|port| {
        let link =
            format!("<http://127.0.0.1:{port}/api/models?filter=gguf&cursor=abc123>; rel=\"next\"");
        vec![route(
            SEARCH_LINE,
            Reply::Json {
                body: json!([search_row("a/b")]).to_string(),
                headers: vec![("Link".to_owned(), link)],
            },
        )]
    });
    let page = server.client().search("", None).await.unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn search_rejects_bad_input_without_sending() {
    let server = Server::start(|_| Vec::new());
    let client = server.client();

    assert_eq!(
        client
            .search("bad\nquery", None)
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    let long = "q".repeat(129);
    assert_eq!(
        client.search(&long, None).await.map(|_| ()).unwrap_err(),
        HfError::InvalidRequest
    );
    assert_eq!(
        client
            .search("", Some("a b"))
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    assert!(
        server.lines().is_empty(),
        "requests sent: {:?}",
        server.lines()
    );
}

#[tokio::test]
async fn error_statuses_map_to_typed_errors() {
    assert_eq!(
        search_with(status(401)).await.unwrap_err(),
        HfError::AccessRequired
    );
    assert_eq!(
        search_with(status(403)).await.unwrap_err(),
        HfError::AccessRequired
    );
    assert_eq!(
        search_with(status(404)).await.unwrap_err(),
        HfError::NotFound
    );
    assert_eq!(
        search_with(with_header(429, "Retry-After", "120"))
            .await
            .unwrap_err(),
        HfError::RateLimited {
            retry_after_secs: Some(120)
        }
    );
    assert_eq!(
        search_with(with_header(429, "Retry-After", "99999"))
            .await
            .unwrap_err(),
        HfError::RateLimited {
            retry_after_secs: Some(3600)
        }
    );
    assert_eq!(
        search_with(with_header(
            429,
            "Retry-After",
            "Wed, 21 Oct 2015 07:28:00 GMT"
        ))
        .await
        .unwrap_err(),
        HfError::RateLimited {
            retry_after_secs: None
        }
    );
    assert_eq!(
        search_with(status(500)).await.unwrap_err(),
        HfError::BadResponse
    );
    let not_json = Reply::Json {
        body: "not json".to_owned(),
        headers: Vec::new(),
    };
    assert_eq!(
        search_with(not_json).await.unwrap_err(),
        HfError::BadResponse
    );
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let server = Server::start(|_| {
        vec![
            route(
                SEARCH_LINE,
                Reply::Redirect("/api/models?filter=gguf&cursor=second".to_owned()),
            ),
            route(
                "GET /api/models?filter=gguf&cursor=second HTTP/1.1",
                json_reply(json!([search_row("a/b")])),
            ),
        ]
    });
    assert_eq!(
        server.client().search("", None).await.unwrap_err(),
        HfError::BadResponse
    );
    assert_eq!(server.lines(), [SEARCH_LINE.to_owned()]);
}

#[tokio::test]
async fn oversized_bodies_are_refused() {
    assert_eq!(
        search_with(Reply::DeclaredOversize).await.unwrap_err(),
        HfError::TooLarge
    );
    assert_eq!(
        search_with(Reply::StreamedOversize).await.unwrap_err(),
        HfError::TooLarge
    );
}

#[tokio::test]
async fn release_parses_detail_and_normalizes() {
    let server = Server::start(|_| {
        vec![route(
            DETAIL_LINE,
            json_reply(qwen_detail("owner/name", REV)),
        )]
    });
    let release = server.client().release("owner/name", None).await.unwrap();

    assert_eq!(release.repo_id, "owner/name");
    assert_eq!(release.revision, REV);
    assert_eq!(release.gated, GatedAccess::Automatic);
    assert_eq!(release.license.as_deref(), Some("apache-2.0"));
    assert_eq!(release.family.name, "Qwen2.5-Coder");
    assert_eq!(release.artifacts.len(), 1);
    let artifact = &release.artifacts[0];
    assert_eq!(artifact.filename, "m-Q4_K_M.gguf");
    assert_eq!(artifact.sha256.as_deref(), Some(SHA256));
    assert_eq!(artifact.size_bytes, Some(4683073536));
    assert_eq!(artifact.quantization_hint.as_deref(), Some("Q4_K_M"));
    assert_eq!(release.other_weight_files, 1);
    assert_eq!(release.dropped_invalid, 0);
}

#[tokio::test]
async fn release_falls_back_to_tags_for_license_and_base_model() {
    let detail = detail_with(
        "owner/name",
        REV,
        json!(null),
        json!([
            "license:mit",
            "base_model:finetune:Qwen/Qwen2.5-Coder-7B-Instruct"
        ]),
        json!([]),
    );
    let server = Server::start(|_| vec![route(DETAIL_LINE, json_reply(detail))]);
    let release = server.client().release("owner/name", None).await.unwrap();

    assert_eq!(release.license.as_deref(), Some("mit"));
    assert_eq!(release.family.name, "Qwen2.5-Coder");
    assert_eq!(
        release.family.source,
        workstation_core::models::family::FamilySource::BaseModel
    );
}

#[tokio::test]
async fn release_with_pin_uses_revision_endpoint_and_requires_matching_sha() {
    let pinned_line = format!("GET /api/models/owner/name/revision/{REV}?blobs=true HTTP/1.1");

    let ok = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("owner/name", REV)),
        )]
    });
    let release = ok.client().release("owner/name", Some(REV)).await.unwrap();
    assert_eq!(release.revision, REV);
    assert_eq!(ok.lines(), vec![pinned_line.clone()]);

    let wrong_sha = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("owner/name", OTHER_REV)),
        )]
    });
    assert_eq!(
        wrong_sha
            .client()
            .release("owner/name", Some(REV))
            .await
            .unwrap_err(),
        HfError::BadResponse
    );

    let other_repo = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("other/repo", REV)),
        )]
    });
    assert_eq!(
        other_repo
            .client()
            .release("owner/name", Some(REV))
            .await
            .unwrap_err(),
        HfError::BadResponse
    );

    let case_only = Server::start(|_| {
        vec![route(
            &pinned_line,
            json_reply(qwen_detail("Owner/Name", REV)),
        )]
    });
    assert!(
        case_only
            .client()
            .release("owner/name", Some(REV))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn release_rejects_invalid_requests_without_sending() {
    let server = Server::start(|_| Vec::new());
    let client = server.client();
    let upper = REV.to_ascii_uppercase();

    for repo in ["no-owner", "a/b/c", "../x"] {
        assert_eq!(
            client.release(repo, None).await.map(|_| ()).unwrap_err(),
            HfError::InvalidRequest,
            "repo {repo}"
        );
    }
    assert_eq!(
        client
            .release("owner/name", Some("main"))
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    assert_eq!(
        client
            .release("owner/name", Some(&upper))
            .await
            .map(|_| ())
            .unwrap_err(),
        HfError::InvalidRequest
    );
    assert!(
        server.lines().is_empty(),
        "requests sent: {:?}",
        server.lines()
    );
}

#[tokio::test]
async fn release_with_hostile_filenames_drops_them() {
    let detail = detail_with(
        "owner/name",
        REV,
        json!({}),
        json!([]),
        json!([
            { "rfilename": "../evil.gguf", "size": 1 },
            { "rfilename": "ok.gguf", "size": 2 },
        ]),
    );
    let server = Server::start(|_| vec![route(DETAIL_LINE, json_reply(detail))]);
    let release = server.client().release("owner/name", None).await.unwrap();
    let names: Vec<_> = release
        .artifacts
        .iter()
        .map(|a| a.filename.as_str())
        .collect();
    assert_eq!(names, ["ok.gguf"]);
    assert_eq!(release.dropped_invalid, 1);

    let siblings: Vec<Value> = (0..5001)
        .map(|i| json!({ "rfilename": format!("f{i}.txt") }))
        .collect();
    let huge = detail_with(
        "owner/name",
        REV,
        json!({}),
        json!([]),
        Value::Array(siblings),
    );
    let server = Server::start(|_| vec![route(DETAIL_LINE, json_reply(huge))]);
    assert_eq!(
        server
            .client()
            .release("owner/name", None)
            .await
            .unwrap_err(),
        HfError::TooLarge
    );
}

#[test]
fn production_client_builds_with_tls_roots() {
    assert!(HuggingFaceClient::new().is_ok());
}
