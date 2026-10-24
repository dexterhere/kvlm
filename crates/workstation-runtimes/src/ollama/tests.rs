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
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new(version: Reply, tags: Reply, ps: Reply) -> Self {
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
                let request = String::from_utf8(request).unwrap();
                let line = request.lines().next().unwrap_or("").to_string();
                recorded.lock().unwrap().push(line.clone());
                let reply = match line.as_str() {
                    "GET /api/version HTTP/1.1" => version,
                    "GET /api/tags HTTP/1.1" => tags,
                    "GET /api/ps HTTP/1.1" => ps,
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
        let mut actual = self.requests.lock().unwrap().clone();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
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
