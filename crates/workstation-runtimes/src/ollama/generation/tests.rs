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

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone)]
enum Reply {
    Json(&'static str),
    Error(u16),
    Stream(Vec<Vec<u8>>),
    Slow,
    SlowBody,
}

#[derive(Clone, Debug)]
struct Request {
    line: String,
    body: Vec<u8>,
}

struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new(status: Reply, tags: Reply, show: Reply, generate: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorded = Arc::clone(&requests);
        let stopping = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !stopping.load(Ordering::Relaxed) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("mock listener failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut headers = Vec::new();
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    if !matches!(stream.read(&mut byte), Ok(1)) {
                        break;
                    }
                    headers.push(byte[0]);
                }
                let headers = String::from_utf8(headers).unwrap();
                let line = headers.lines().next().unwrap_or("").to_owned();
                let length = headers
                    .lines()
                    .filter_map(|header| header.split_once(':'))
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .find_map(|(_, value)| value.trim().parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                let _ = stream.read_exact(&mut body);
                recorded.lock().unwrap().push(Request {
                    line: line.clone(),
                    body,
                });
                let reply = match line.as_str() {
                    "GET /api/status HTTP/1.1" => status.clone(),
                    "GET /api/version HTTP/1.1" => Reply::Json(r#"{"version":"fixture-only"}"#),
                    "GET /api/tags HTTP/1.1" => tags.clone(),
                    "POST /api/show HTTP/1.1" => show.clone(),
                    "POST /api/generate HTTP/1.1" => generate.clone(),
                    _ => Reply::Error(404),
                };
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
        OllamaAdapter::with_port(self.port, Duration::from_millis(300)).unwrap()
    }

    fn lines(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.line.clone())
            .collect()
    }

    fn body(&self, line: &str) -> serde_json::Value {
        let body = self
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.line == line)
            .expect("expected request")
            .body
            .clone();
        serde_json::from_slice(&body).unwrap()
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
            Self::Json(body) => write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
            Self::Error(code) => write!(
                stream,
                "HTTP/1.1 {code} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
            Self::Stream(parts) => {
                stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n",
                )?;
                for part in parts {
                    stream.write_all(&part)?;
                }
                Ok(())
            }
            Self::SlowBody => {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n")?;
                stream.flush()?;
                thread::sleep(Duration::from_millis(100));
                Ok(())
            }
            Self::Slow => {
                thread::sleep(Duration::from_millis(100));
                Ok(())
            }
        }
    }
}

fn selected() -> InstalledModel {
    InstalledModel {
        name: "example:latest".into(),
        digest: Some(DIGEST.into()),
        size: None,
        details: None,
        remote_model: None,
        remote_host: None,
    }
}

fn tags() -> Reply {
    Reply::Json(
        r#"{"models":[{"name":"example:latest","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#,
    )
}

fn done_stream() -> Reply {
    Reply::Stream(vec![
        format!(r#"{{"model":"{DIGEST}:local","done":false,"response":"h"}}"#)
            .into_bytes()
            .into_iter()
            .chain(b"\r\n".iter().copied())
            .collect(),
        format!(r#"{{"model":"{DIGEST}:local","done":true,"response":"é","done_reason":"stop","eval_count":2,"eval_duration":3,"load_duration":4}}"#).into_bytes(),
    ])
}

fn record(value: serde_json::Value) -> Vec<u8> {
    value.to_string().into_bytes()
}

fn local_record(done: bool, response: &str) -> serde_json::Value {
    serde_json::json!({"model":format!("{DIGEST}:local"), "done":done, "response":response})
}

fn request() -> GenerationRequest<'static> {
    GenerationRequest {
        prompt: "say hi",
        num_predict: 4,
        num_ctx: 64,
        seed: 7,
    }
}

#[tokio::test]
async fn generates_only_after_local_guards_and_pins_wire_identity() {
    let server = Server::new(
        Reply::Json(r#"{"cloud":{"disabled":true}}"#),
        tags(),
        Reply::Json(r#"{"capabilities":["completion"]}"#),
        done_stream(),
    );
    let (progress, receiver) = tokio::sync::watch::channel(GenerationProgress::default());
    let summary = server
        .adapter()
        .generate_local(&selected(), request(), progress)
        .await
        .unwrap();
    assert_eq!(summary.model_digest, DIGEST);
    assert_eq!(summary.runtime_version, "fixture-only");
    assert_eq!(summary.response_chunks, 2);
    assert_eq!(summary.eval_count, Some(2));
    assert_eq!(summary.done_reason, "stop");
    assert_eq!(receiver.borrow().response_chunks, 2);
    assert_eq!(
        server.lines(),
        [
            "GET /api/status HTTP/1.1",
            "GET /api/version HTTP/1.1",
            "GET /api/tags HTTP/1.1",
            "POST /api/show HTTP/1.1",
            "POST /api/generate HTTP/1.1"
        ]
    );
    assert_eq!(
        server.body("POST /api/show HTTP/1.1"),
        serde_json::json!({"model":format!("{DIGEST}:local"),"verbose":false})
    );
    assert_eq!(
        server.body("POST /api/generate HTTP/1.1"),
        serde_json::json!({"model":format!("{DIGEST}:local"),"prompt":"say hi","stream":true,"think":false,"options":{"num_predict":4,"num_ctx":64,"seed":7,"temperature":0}})
    );
}

#[tokio::test]
async fn blocking_status_or_identity_never_reaches_generation() {
    for (status, tag_reply) in [
        (Reply::Json("{}"), tags()),
        (Reply::Error(404), tags()),
        (Reply::Json(r#"{"cloud":{"disabled":"true"}}"#), tags()),
        (Reply::Json(r#"{"cloud":{"disabled":false}}"#), tags()),
        (
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            Reply::Json(
                r#"{"models":[{"name":"example:latest","digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}"#,
            ),
        ),
        (
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            Reply::Json(
                r#"{"models":[{"name":"example:latest","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"name":"example:latest","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#,
            ),
        ),
        (
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            Reply::Json(
                r#"{"models":[{"name":"example:latest","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","remote_host":"https://example.invalid"}]}"#,
            ),
        ),
    ] {
        let server = Server::new(
            status,
            tag_reply,
            Reply::Json(r#"{"capabilities":["completion"]}"#),
            done_stream(),
        );
        let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
        assert!(
            server
                .adapter()
                .generate_local(&selected(), request(), progress)
                .await
                .is_err()
        );
        assert!(
            !server
                .lines()
                .iter()
                .any(|line| line == "POST /api/generate HTTP/1.1")
        );
    }
}

#[tokio::test]
async fn guard_rejections_for_remote_incomplete_or_unsupported_selection_are_local() {
    for model in [
        InstalledModel {
            digest: None,
            ..selected()
        },
        InstalledModel {
            remote_model: Some("remote".into()),
            ..selected()
        },
        InstalledModel {
            name: "example:cloud".into(),
            ..selected()
        },
    ] {
        let server = Server::new(
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            tags(),
            Reply::Json("{}"),
            done_stream(),
        );
        let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
        assert!(
            server
                .adapter()
                .generate_local(&model, request(), progress)
                .await
                .is_err()
        );
        assert!(server.lines().is_empty());
    }
}

#[tokio::test]
async fn rejects_missing_completion_and_invalid_or_terminal_streams() {
    let terminal = record(
        serde_json::json!({"model":format!("{DIGEST}:local"),"done":true,"response":"x","done_reason":"stop"}),
    );
    let trailing = record(local_record(false, "y"));
    let cases = [
        (Reply::Json("{}"), done_stream()),
        (
            Reply::Json(r#"{"capabilities":["completion"]}"#),
            Reply::Stream(vec![record(serde_json::json!({"done":false}))]),
        ),
        (
            Reply::Json(r#"{"capabilities":["completion"]}"#),
            Reply::Stream(vec![
                [terminal, b"\n".to_vec(), trailing, b"\n".to_vec()].concat(),
            ]),
        ),
    ];
    for (show, stream) in cases {
        let server = Server::new(
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            tags(),
            show,
            stream,
        );
        let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
        assert!(
            server
                .adapter()
                .generate_local(&selected(), request(), progress)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn show_failures_or_remote_metadata_cannot_fall_back_to_generation() {
    for show in [
        Reply::Error(404),
        Reply::Json(r#"{"capabilities":["completion"],"remote_model":"remote"}"#),
    ] {
        let server = Server::new(
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            tags(),
            show,
            done_stream(),
        );
        let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
        assert!(
            server
                .adapter()
                .generate_local(&selected(), request(), progress)
                .await
                .is_err()
        );
        assert!(
            !server
                .lines()
                .iter()
                .any(|line| line == "POST /api/generate HTTP/1.1")
        );
    }
}

#[tokio::test]
async fn stream_header_timeout_is_bounded() {
    let server = Server::new(
        Reply::Json(r#"{"cloud":{"disabled":true}}"#),
        tags(),
        Reply::Json(r#"{"capabilities":["completion"]}"#),
        Reply::Slow,
    );
    let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
    let error = server
        .adapter()
        .generate_local_with_timeouts(
            &selected(),
            request(),
            progress,
            Duration::from_millis(50),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
    assert!(error.contains("timed out"), "{error}");
}

#[test]
fn parser_handles_single_byte_utf8_crlf_and_final_record() {
    let reference = format!("{DIGEST}:local");
    let mut parser = StreamParser::new(&reference);
    let (progress, receiver) = tokio::sync::watch::channel(GenerationProgress::default());
    let mut bytes = record(local_record(false, "hé"));
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(&record(
        serde_json::json!({"model":reference,"done":true,"response":"!","done_reason":"length"}),
    ));
    for byte in bytes.chunks(1) {
        parser
            .feed(byte, Duration::from_millis(1), &progress)
            .unwrap();
    }
    let summary = parser
        .finish(
            Duration::from_millis(2),
            "example".into(),
            DIGEST.into(),
            "test".into(),
            &progress,
        )
        .unwrap();
    assert_eq!(summary.response_chunks, 2);
    assert_eq!(summary.done_reason, "length");
    assert_eq!(receiver.borrow().response_chunks, 2);
}

#[test]
fn parser_rejects_unsafe_records_and_enforces_bounds() {
    let reference = format!("{DIGEST}:local");
    for bytes in [
        b"{bad}\n".to_vec(),
        record(serde_json::json!({"model":reference,"done":false,"error":{"private":"body"}})),
        record(
            serde_json::json!({"model":reference,"done":false,"remote_host":"https://example.invalid"}),
        ),
        record(
            serde_json::json!({"model":reference,"done":true,"response":"x","done_reason":"stop","eval_count":-1}),
        ),
    ] {
        let mut parser = StreamParser::new(&reference);
        let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
        let mut bytes = bytes;
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        assert!(parser.feed(&bytes, Duration::ZERO, &progress).is_err());
    }
    let (progress, _) = tokio::sync::watch::channel(GenerationProgress::default());
    let mut parser = StreamParser::new(&reference);
    parser.bytes = MAX_STREAM_BYTES - 1;
    assert!(
        parser
            .feed(b"xx", Duration::ZERO, &progress)
            .unwrap_err()
            .contains("4 MiB")
    );
    let mut parser = StreamParser::new(&reference);
    parser.records = MAX_RECORDS;
    assert!(
        parser
            .feed(b"{}\n", Duration::ZERO, &progress)
            .unwrap_err()
            .contains("too many")
    );
    let mut parser = StreamParser::new(&reference);
    parser.line = vec![b'x'; MAX_LINE_BYTES - 1];
    assert!(
        parser
            .feed(b"xx", Duration::ZERO, &progress)
            .unwrap_err()
            .contains("64 KiB")
    );
}

#[tokio::test]
async fn wrong_content_type_and_idle_body_do_not_complete_a_benchmark() {
    for (reply, expected) in [
        (Reply::Json("{}"), "expected an NDJSON"),
        (Reply::SlowBody, "idle timeout"),
    ] {
        let server = Server::new(
            Reply::Json(r#"{"cloud":{"disabled":true}}"#),
            tags(),
            Reply::Json(r#"{"capabilities":["completion"]}"#),
            reply,
        );
        let (progress, _) = watch::channel(GenerationProgress::default());
        let error = server
            .adapter()
            .generate_local_with_timeouts(
                &selected(),
                request(),
                progress,
                Duration::from_secs(1),
                Duration::from_millis(30),
            )
            .await
            .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn thinking_and_invalid_metric_types_do_not_become_response_timings() {
    let reference = format!("{DIGEST}:local");
    let (progress, readings) = watch::channel(GenerationProgress::default());
    let mut parser = StreamParser::new(&reference);
    let thinking = serde_json::json!({"model":reference,"done":false,"thinking":"hidden reasoning"})
        .to_string() + "\n";
    parser
        .feed(thinking.as_bytes(), Duration::from_millis(1), &progress)
        .unwrap();
    assert!(readings.borrow().first_output_after.is_none());
    let text =
        serde_json::json!({"model":reference,"done":true,"response":"answer","done_reason":"stop"})
            .to_string()
            + "\n";
    parser
        .feed(text.as_bytes(), Duration::from_millis(2), &progress)
        .unwrap();
    assert_eq!(
        readings.borrow().first_output_after,
        Some(Duration::from_millis(2))
    );
    assert_eq!(readings.borrow().response_chunks, 1);
    let mut parser = StreamParser::new(&reference);
    let invalid = serde_json::json!({"model":reference,"done":true,"response":"answer","done_reason":"stop","eval_duration":"1000"}).to_string() + "\n";
    assert!(
        parser
            .feed(invalid.as_bytes(), Duration::ZERO, &progress)
            .unwrap_err()
            .contains("invalid stream record")
    );
}
