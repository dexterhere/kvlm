// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::*;

const VERSION: Reply = Reply::Json(r#"{"version":"0.12.0","future_field":true}"#);
const EMPTY: Reply = Reply::Json(r#"{"models":[]}"#);

#[derive(Clone, Copy)]
enum Reply {
    Json(&'static str),
    Error(u16, &'static str),
    Redirect,
    DeclaredOversize,
    StreamedOversize,
    SlowHeaders,
    SlowBody,
}

struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    line: String,
    headers: String,
    body: Vec<u8>,
}

impl Server {
    fn new(version: Reply, tags: Reply, ps: Reply) -> Self {
        Self::with_show(version, tags, Reply::Error(404, "unexpected request"), ps)
    }

    fn inspection(show: Reply, ps: Reply) -> Self {
        Self::with_show(VERSION, EMPTY, show, ps)
    }

    fn with_show(version: Reply, tags: Reply, show: Reply, ps: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorded = Arc::clone(&requests);
        let stopping = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(4);
            while !stopping.load(Ordering::Relaxed) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
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
                    .set_write_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while request.len() < 8192
                    && !request.ends_with(b"\r\n\r\n")
                    && Instant::now() < deadline
                    && !stopping.load(Ordering::Relaxed)
                {
                    match stream.read(&mut byte) {
                        Ok(1) => request.push(byte[0]),
                        _ => break,
                    }
                }
                let headers = String::from_utf8(request).unwrap();
                let line = headers.lines().next().unwrap_or("").to_string();
                let content_length = headers
                    .lines()
                    .filter_map(|header| header.split_once(':'))
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .find_map(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; content_length];
                let _ = stream.read_exact(&mut body);
                recorded.lock().unwrap().push(RecordedRequest {
                    line: line.clone(),
                    headers,
                    body,
                });
                let reply = match line.as_str() {
                    "GET /api/version HTTP/1.1" => version,
                    "GET /api/tags HTTP/1.1" => tags,
                    "GET /api/ps HTTP/1.1" => ps,
                    "POST /api/show HTTP/1.1" => show,
                    _ => Reply::Error(404, "unexpected request"),
                };
                // Clients intentionally close early in limit and timeout tests.
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

    fn adapter(&self) -> OllamaAdapter {
        OllamaAdapter::with_port(self.port, Duration::from_secs(1)).unwrap()
    }

    fn assert_requests(&self, paths: &[&str]) {
        let mut expected: Vec<_> = paths
            .iter()
            .map(|path| format!("GET {path} HTTP/1.1"))
            .collect();
        let mut actual: Vec<_> = self
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.line.clone())
            .collect();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
    }

    fn assert_inspection_requests(&self) {
        let mut actual: Vec<_> = self
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.line.clone())
            .collect();
        actual.sort();
        assert_eq!(
            actual,
            [
                "GET /api/ps HTTP/1.1".to_string(),
                "POST /api/show HTTP/1.1".to_string(),
            ]
        );
    }

    fn show_request(&self) -> RecordedRequest {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.line == "POST /api/show HTTP/1.1")
            .cloned()
            .expect("expected /api/show request")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

impl Reply {
    fn write(self, stream: &mut TcpStream) -> std::io::Result<()> {
        match self {
            Self::Json(body) => {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            }
            Self::Error(code, body) => {
                write!(stream, "HTTP/1.1 {code} Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            }
            Self::Redirect => stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
            Self::DeclaredOversize => write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_RESPONSE_BYTES + 1),
            Self::StreamedOversize => {
                stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")?;
                stream.write_all(&vec![b'x'; MAX_RESPONSE_BYTES + 1])
            }
            Self::SlowHeaders => {
                thread::sleep(Duration::from_millis(200));
                EMPTY.write(stream)
            }
            Self::SlowBody => {
                let body = br#"{"version":"test"}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
                thread::sleep(Duration::from_millis(200));
                stream.write_all(body)
            }
        }
    }
}

#[tokio::test]
async fn discovers_models_cloud_metadata_and_runtime_allocations() {
    let server = Server::new(
        VERSION,
        Reply::Json(
            r#"{"models":[
        {"name":"downloaded:latest","digest":"abc","size":1024,"remote_model":"","remote_host":"","details":{"family":"llama","parameter_size":"1B","quantization_level":"Q4"}},
        {"name":"remote:latest","remote_model":"remote","remote_host":"https://example.invalid"},
        {"name":"unknown:latest"}
    ]}"#,
        ),
        Reply::Json(
            r#"{"models":[{"name":"downloaded:latest","size":1000,"size_vram":750,"context_length":4096}]}"#,
        ),
    );
    let snapshot = server.adapter().discover().await.unwrap();
    assert_eq!(snapshot.version, "0.12.0");
    let models = snapshot.installed.unwrap();
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].digest.as_deref(), Some("abc"));
    assert_eq!(models[0].size, Some(1024));
    let details = models[0].details.as_ref().unwrap();
    assert_eq!(details.family.as_deref(), Some("llama"));
    assert_eq!(details.parameter_size.as_deref(), Some("1B"));
    assert_eq!(details.quantization_level.as_deref(), Some("Q4"));
    assert!(!models[0].cloud_routed());
    assert!(models[1].cloud_routed());
    assert!(!models[2].cloud_routed());
    assert!(models[2].size.is_none());
    let running = snapshot.running.unwrap();
    assert_eq!(running[0].cpu_bytes(), Some(250));
    assert_eq!(running[0].size_vram, Some(750));
    assert_eq!(running[0].context_length, Some(4096));
    server.assert_requests(&["/api/version", "/api/tags", "/api/ps"]);
}

#[tokio::test]
async fn partial_failures_preserve_version_and_the_other_inventory() {
    for (tags, ps, tags_ok) in [
        (Reply::Error(503, "private server details"), EMPTY, false),
        (EMPTY, Reply::Error(500, "private server details"), true),
    ] {
        let server = Server::new(VERSION, tags, ps);
        let snapshot = server.adapter().discover().await.unwrap();
        assert_eq!(snapshot.version, "0.12.0");
        let (success, failure) = if tags_ok {
            (
                snapshot.installed.map(|m| m.len()),
                snapshot.running.map(|m| m.len()),
            )
        } else {
            (
                snapshot.running.map(|m| m.len()),
                snapshot.installed.map(|m| m.len()),
            )
        };
        assert_eq!(success.unwrap(), 0);
        let error = failure.unwrap_err();
        assert!(error.contains("HTTP 50"));
        assert!(!error.contains("private"));
        server.assert_requests(&["/api/version", "/api/tags", "/api/ps"]);
    }
}

#[tokio::test]
async fn empty_inventories_are_valid_but_missing_arrays_are_errors() {
    let server = Server::new(VERSION, EMPTY, EMPTY);
    let snapshot = server.adapter().discover().await.unwrap();
    assert!(snapshot.installed.unwrap().is_empty());
    assert!(snapshot.running.unwrap().is_empty());
    for body in ["{}", r#"{"models":null}"#, r#"{"models":{}}"#] {
        let server = Server::new(VERSION, Reply::Json(body), Reply::Json(body));
        let snapshot = server.adapter().discover().await.unwrap();
        assert!(snapshot.installed.unwrap_err().contains("invalid JSON"));
        assert!(snapshot.running.unwrap_err().contains("invalid JSON"));
    }
}

#[tokio::test]
async fn rejects_invalid_version_without_listing_models() {
    for body in [
        "private malformed body",
        "{}",
        r#"{"version":null}"#,
        r#"{"version":42}"#,
        r#"{"version":"  "}"#,
    ] {
        let server = Server::new(Reply::Json(body), EMPTY, EMPTY);
        let error = server.adapter().discover().await.unwrap_err();
        assert!(error.starts_with("/api/version:"));
        assert!(!error.contains("private"));
        server.assert_requests(&["/api/version"]);
    }
}

#[tokio::test]
async fn rejects_invalid_inventory_fields_without_fabricating_readings() {
    for body in [
        r#"{"models":[{"name":" "}]}"#,
        r#"{"models":[{}]}"#,
        r#"{"models":[{"name":"bad","size":-1}]}"#,
        r#"{"models":[{"name":"bad","size":1.5}]}"#,
        r#"{"models":[{"name":"bad","size":"100"}]}"#,
    ] {
        let server = Server::new(VERSION, Reply::Json(body), Reply::Json(body));
        let snapshot = server.adapter().discover().await.unwrap();
        assert!(snapshot.installed.is_err());
        assert!(snapshot.running.is_err());
    }
    let server = Server::new(
        VERSION,
        EMPTY,
        Reply::Json(r#"{"models":[{"name":"bad","size_vram":-1}]}"#),
    );
    assert!(server.adapter().discover().await.unwrap().running.is_err());
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let server = Server::new(Reply::Redirect, EMPTY, EMPTY);
    assert_eq!(
        server.adapter().discover().await.unwrap_err(),
        "/api/version: HTTP 302"
    );
    server.assert_requests(&["/api/version"]);
}

#[tokio::test]
async fn bounds_declared_and_streamed_response_bodies() {
    for reply in [Reply::DeclaredOversize, Reply::StreamedOversize] {
        let server = Server::new(reply, EMPTY, EMPTY);
        assert_eq!(
            server.adapter().discover().await.unwrap_err(),
            "/api/version: response exceeds 2 MiB limit"
        );
        server.assert_requests(&["/api/version"]);
    }
}

#[tokio::test]
async fn request_timeout_includes_slow_headers_and_body() {
    for reply in [Reply::SlowHeaders, Reply::SlowBody] {
        let server = Server::new(reply, EMPTY, EMPTY);
        let adapter = OllamaAdapter::with_port(server.port, Duration::from_millis(50)).unwrap();
        let error = adapter.discover().await.unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        server.assert_requests(&["/api/version"]);
    }
}

#[tokio::test]
async fn reports_an_unavailable_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let adapter = OllamaAdapter::with_port(port, Duration::from_millis(200)).unwrap();
    let error = adapter.discover().await.unwrap_err();
    assert!(error.contains("endpoint unavailable"), "{error}");
}

#[test]
fn allocation_remainder_is_checked_and_missing_metadata_stays_unknown() {
    for (total, gpu, expected) in [
        (Some(100), Some(101), None),
        (None, Some(0), None),
        (Some(100), None, None),
        (Some(0), Some(0), Some(0)),
        (Some(100), Some(0), Some(100)),
        (Some(100), Some(100), Some(0)),
    ] {
        let model = RunningModel {
            name: "example".into(),
            digest: None,
            size: total,
            size_vram: gpu,
            context_length: None,
        };
        assert_eq!(model.cpu_bytes(), expected);
    }
    for (remote_model, remote_host, expected) in [
        (Some("model"), None, true),
        (None, Some("https://example.invalid"), true),
        (Some(" "), Some(""), false),
        (None, None, false),
    ] {
        let model = InstalledModel {
            name: "example".into(),
            digest: None,
            size: None,
            details: None,
            remote_model: remote_model.map(str::to_owned),
            remote_host: remote_host.map(str::to_owned),
        };
        assert_eq!(model.cloud_routed(), expected);
    }
}

#[tokio::test]
async fn inspection_posts_model_declarations_and_loaded_snapshot() {
    let server = Server::inspection(
        Reply::Json(
            r#"{
                "details":{"family":"llama","parameter_size":"8B","quantization_level":"Q4"},
                "capabilities":["completion","tools"],
                "parameters":"num_ctx 8192",
                "model_info":{"general.architecture":"llama","llama.context_length":8192},
                "remote_model":"cloud-copy",
                "remote_host":"https://example.invalid"
            }"#,
        ),
        Reply::Json(
            r#"{"models":[{"name":"example:latest","digest":"loaded","context_length":4096}]}"#,
        ),
    );

    let inspection = server
        .adapter()
        .inspect_model("example:latest")
        .await
        .unwrap();
    let metadata = inspection.metadata.unwrap();
    assert_eq!(
        metadata.details.as_ref().unwrap().family.as_deref(),
        Some("llama")
    );
    assert_eq!(
        metadata.capabilities.as_ref().unwrap(),
        &vec!["completion".to_owned(), "tools".to_owned()]
    );
    assert_eq!(metadata.parameters.as_deref(), Some("num_ctx 8192"));
    assert_eq!(metadata.architecture(), Some("llama"));
    assert_eq!(metadata.declared_context_length(), Some(8192));
    assert!(metadata.cloud_routed());
    let running = inspection.running.unwrap();
    assert_eq!(running[0].digest.as_deref(), Some("loaded"));
    assert_eq!(running[0].context_length, Some(4096));

    server.assert_inspection_requests();
    let request = server.show_request();
    assert!(
        request
            .headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({"model":"example:latest","verbose":false})
    );
}

#[tokio::test]
async fn inspection_preserves_valid_model_name_json_escapes() {
    let server = Server::inspection(Reply::Json("{}"), EMPTY);
    let model = "family/\"quoted\"\\path:β";
    let inspection = server.adapter().inspect_model(model).await.unwrap();
    assert!(inspection.metadata.is_ok());
    assert!(inspection.running.unwrap().is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&server.show_request().body).unwrap(),
        serde_json::json!({"model":model,"verbose":false})
    );
}

#[tokio::test]
async fn inspection_rejects_invalid_model_names_without_requests() {
    for model in ["", " \t ", "line\nbreak", "nul\0byte"] {
        let server = Server::inspection(Reply::Json("{}"), EMPTY);
        assert_eq!(
            server.adapter().inspect_model(model).await.unwrap_err(),
            "/api/show: invalid model name"
        );
        server.assert_requests(&[]);
    }
    let server = Server::inspection(Reply::Json("{}"), EMPTY);
    let too_long = "a".repeat(1025);
    assert!(server.adapter().inspect_model(&too_long).await.is_err());
    server.assert_requests(&[]);
}

#[tokio::test]
async fn inspection_keeps_unknown_fields_unknown_and_empty_objects_valid() {
    let server = Server::inspection(Reply::Json("{}"), EMPTY);
    let inspection = server.adapter().inspect_model("example").await.unwrap();
    let metadata = inspection.metadata.unwrap();
    assert!(metadata.details.is_none());
    assert!(metadata.capabilities.is_none());
    assert!(metadata.parameters.is_none());
    assert!(metadata.model_info.is_none());
    assert_eq!(metadata.architecture(), None);
    assert_eq!(metadata.declared_context_length(), None);
    assert!(!metadata.cloud_routed());
    assert!(inspection.running.unwrap().is_empty());
}

#[tokio::test]
async fn inspection_rejects_malformed_metadata_without_losing_running_snapshot() {
    for show in [
        Reply::Json(r#"{"capabilities":"completion"}"#),
        Reply::Json(r#"{"model_info":[]}"#),
        Reply::Json(r#"{"details":{"family":42}}"#),
    ] {
        let server = Server::inspection(show, Reply::Json(r#"{"models":[{"name":"loaded"}]}"#));
        let inspection = server.adapter().inspect_model("example").await.unwrap();
        assert!(inspection.metadata.unwrap_err().contains("invalid JSON"));
        assert_eq!(inspection.running.unwrap()[0].name, "loaded");
        server.assert_inspection_requests();
    }
}

#[tokio::test]
async fn inspection_keeps_metadata_when_running_snapshot_fails_or_model_disappears() {
    for show in [Reply::Json("{}"), Reply::Error(404, "private model name")] {
        let server = Server::inspection(show, Reply::Error(503, "private server details"));
        let inspection = server.adapter().inspect_model("example").await.unwrap();
        assert!(inspection.running.unwrap_err().contains("HTTP 503"));
        if matches!(show, Reply::Error(_, _)) {
            assert_eq!(inspection.metadata.unwrap_err(), "/api/show: HTTP 404");
        } else {
            assert!(inspection.metadata.is_ok());
        }
        server.assert_inspection_requests();
    }
}

#[tokio::test]
async fn inspection_context_declaration_requires_exact_positive_integer_key() {
    for (show, expected) in [
        (
            Reply::Json(
                r#"{"model_info":{"general.architecture":"llama","llama.context_length":4096}}"#,
            ),
            Some(4096),
        ),
        (
            Reply::Json(
                r#"{"model_info":{"general.architecture":"llama","llama.context_length":0}}"#,
            ),
            None,
        ),
        (
            Reply::Json(
                r#"{"model_info":{"general.architecture":"llama","llama.context_length":1.5}}"#,
            ),
            None,
        ),
        (
            Reply::Json(
                r#"{"model_info":{"general.architecture":"llama","llama.context_length":-1}}"#,
            ),
            None,
        ),
        (
            Reply::Json(
                r#"{"model_info":{"general.architecture":"llama","llama.context_length":"4096"}}"#,
            ),
            None,
        ),
        (
            Reply::Json(
                r#"{"model_info":{"general.architecture":"llama","other.context_length":4096}}"#,
            ),
            None,
        ),
    ] {
        let server = Server::inspection(show, EMPTY);
        let inspection = server.adapter().inspect_model("example").await.unwrap();
        assert_eq!(
            inspection.metadata.unwrap().declared_context_length(),
            expected
        );
    }
}

#[tokio::test]
async fn inspection_show_response_limits_timeouts_and_redirects_are_isolated() {
    for show in [Reply::Redirect, Reply::DeclaredOversize, Reply::SlowHeaders] {
        let server = Server::inspection(show, EMPTY);
        let adapter = OllamaAdapter::with_port(server.port, Duration::from_millis(50)).unwrap();
        let inspection = adapter.inspect_model("example").await.unwrap();
        let error = inspection.metadata.unwrap_err();
        assert!(
            error.contains("HTTP 302")
                || error.contains("exceeds 2 MiB")
                || error.contains("timed out"),
            "{error}"
        );
        if matches!(show, Reply::SlowHeaders) {
            assert!(
                inspection.running.is_ok() || inspection.running.unwrap_err().contains("timed out")
            );
        } else {
            assert!(inspection.running.unwrap().is_empty());
        }
        if matches!(show, Reply::SlowHeaders) {
            let _ = server.show_request();
        } else {
            server.assert_inspection_requests();
        }
    }
}
