// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

enum Reply {
    Full(u16, &'static str, String),
    /// (delay before sending, bytes); then optionally hold the connection open silently.
    Chunks(Vec<(u64, Vec<u8>)>, u64),
}

fn json(code: u16, body: &str) -> Reply {
    Reply::Full(code, "application/json", body.to_owned())
}

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<(String, String)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Fake {
    fn start(handler: impl Fn(&str, &str) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let (s, st) = (Arc::clone(&seen), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !st.load(Ordering::Relaxed) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (h, s) = (Arc::clone(&handler), Arc::clone(&s));
                        thread::spawn(move || serve(stream, &*h, &s));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        Self {
            port,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    fn client(&self) -> LlamaClient {
        LlamaClient::new(self.port).unwrap()
    }

    fn bodies(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, b)| b.clone())
            .collect()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    handler: &(dyn Fn(&str, &str) -> Reply + Send + Sync),
    seen: &Mutex<Vec<(String, String)>>,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") && raw.len() < 16384 {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        raw.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0usize);
    let mut body = vec![0u8; length];
    let _ = stream.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).into_owned();
    let line = head.lines().next().unwrap_or("").to_owned();
    seen.lock().unwrap().push((line.clone(), body.clone()));
    match handler(&line, &body) {
        Reply::Full(code, ctype, text) => {
            let _ = write!(
                stream,
                "HTTP/1.1 {code} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
        Reply::Chunks(chunks, hold_ms) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            );
            for (delay, bytes) in chunks {
                thread::sleep(Duration::from_millis(delay));
                if write!(stream, "{:x}\r\n", bytes.len()).is_err()
                    || stream.write_all(&bytes).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    return;
                }
                let _ = stream.flush();
            }
            if hold_ms > 0 {
                thread::sleep(Duration::from_millis(hold_ms));
            } else {
                let _ = stream.write_all(b"0\r\n\r\n");
            }
        }
    }
}

fn event(json: &str) -> Vec<u8> {
    format!("data: {json}\n\n").into_bytes()
}

fn request<'a>(alias: &'a str, prompt: &'a str) -> ChatRequest<'a> {
    ChatRequest {
        model: alias,
        prompt,
        max_tokens: 64,
        seed: 7,
        temperature: 0.0,
        cache_prompt: false,
    }
}

static NO: AtomicBool = AtomicBool::new(false);

#[tokio::test]
async fn identity_needs_health_and_the_launch_alias() {
    let models = r#"{"data":[{"id":"abc123"}]}"#;
    let fake = Fake::start(move |line, _| match line {
        l if l.starts_with("GET /health") => json(200, "{}"),
        l if l.starts_with("GET /v1/models") => json(200, models),
        _ => json(404, "{}"),
    });
    assert_eq!(fake.client().identity("abc123").await, Health::Ready);
    assert_eq!(fake.client().identity("someone-else").await, Health::Down);

    let loading = Fake::start(|_, _| json(503, "{}"));
    assert_eq!(loading.client().identity("abc123").await, Health::Loading);
    let silent = LlamaClient::new(1).unwrap();
    assert_eq!(silent.identity("abc123").await, Health::Down);
}

#[tokio::test]
async fn props_slots_and_busy_are_read_and_unknown_stays_unknown() {
    let fake = Fake::start(|line, _| match line {
        l if l.starts_with("GET /props") => json(
            200,
            r#"{"default_generation_settings":{"n_ctx":4096},"total_slots":2,"build_info":"b1","model_path":"/m/a.gguf"}"#,
        ),
        l if l.starts_with("GET /slots") => json(
            200,
            r#"[{"id":0,"n_ctx":2048,"is_processing":false},{"id":1,"n_ctx":2048,"is_processing":true},{"bogus":1}]"#,
        ),
        _ => json(404, "{}"),
    });
    let c = fake.client();
    let props = c.props().await.unwrap();
    assert_eq!((props.n_ctx, props.total_slots), (Some(4096), Some(2)));
    assert_eq!(props.model_path.as_deref(), Some("/m/a.gguf"));
    assert_eq!(c.slots().await.unwrap().len(), 2);
    assert_eq!(c.busy().await, Some(true));

    // Slots disabled: fall back to the request counter.
    let metrics = Fake::start(|line, _| match line {
        l if l.starts_with("GET /slots") => json(501, "{}"),
        l if l.starts_with("GET /metrics") => Reply::Full(
            200,
            "text/plain",
            "# HELP x\nllamacpp:requests_processing 0\nllamacpp:prompt_tokens_total 12\n".into(),
        ),
        _ => json(404, "{}"),
    });
    let c = metrics.client();
    assert_eq!(c.slots().await, Err(LlamaError::Unsupported));
    assert_eq!(c.busy().await, Some(false));
    assert_eq!(
        c.metrics().await.unwrap().get("prompt_tokens_total"),
        Some(12.0)
    );

    // Neither source: the answer is unknown, never "idle".
    let none = Fake::start(|_, _| json(404, "{}"));
    assert_eq!(none.client().busy().await, None);
}

#[test]
fn metrics_parsing_reads_only_known_finite_values() {
    let m = parse_metrics(
        "# TYPE x counter\nllamacpp:tokens_predicted_total 50\nllamacpp:unknown 1\n\
         llamacpp:kv_cache_usage_ratio NaN\nllamacpp:requests_deferred inf\n\
         llamacpp:prompt_seconds_total 1.5e1\nother:prompt_tokens_total 9\nllamacpp:n_decode_total\n",
    );
    assert_eq!(m.get("tokens_predicted_total"), Some(50.0));
    assert_eq!(m.get("prompt_seconds_total"), Some(15.0));
    for missing in [
        "unknown",
        "kv_cache_usage_ratio",
        "requests_deferred",
        "prompt_tokens_total",
        "n_decode_total",
    ] {
        assert_eq!(m.get(missing), None, "{missing}");
    }
}

fn chat_stream() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(event(
        r#"{"choices":[{"delta":{"role":"assistant","content":null}}]}"#,
    ));
    body.extend(b": keep-alive\n\n".to_vec());
    body.extend(event(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#));
    body.extend(event(r#"{"choices":[{"delta":{"content":"lo wor"}}]}"#));
    body.extend(event(r#"{"choices":[{"delta":{"content":""}}]}"#));
    body.extend(event(r#"{"choices":[{"delta":{"content":"ld"}}]}"#));
    body.extend(event(
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ));
    body.extend(event(
        r#"{"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":9},"timings":{"prompt_n":5,"prompt_ms":12.5,"predicted_n":9,"predicted_ms":90.0}}"#,
    ));
    body.extend(b"data: [DONE]\n\n".to_vec());
    body
}

#[tokio::test]
async fn a_chat_stream_counts_content_events_and_keeps_server_counters_separate() {
    let fake = Fake::start(|_, _| Reply::Chunks(vec![(0, chat_stream())], 0));
    let mut seen = Vec::new();
    let end = fake
        .client()
        .stream_chat(
            &request("abc123", "hi \"there\"\n"),
            DEFAULT_DEADLINE,
            &NO,
            |e| {
                if let StreamEvent::Content { text, .. } = e {
                    seen.push(text.to_owned());
                }
            },
        )
        .await
        .unwrap();
    assert_eq!(seen, ["Hel", "lo wor", "ld"]);
    // Three content events, nine tokens: events are not tokens.
    assert_eq!(end.content_events, 3);
    assert_eq!(end.usage.unwrap().completion_tokens, 9);
    assert_eq!(end.timings.unwrap().predicted_n, Some(9));
    assert_eq!(end.finish_reason.as_deref(), Some("stop"));
    assert!(end.first_content_after.is_some());

    // The request is plain JSON built from typed fields.
    let body: Value = serde_json::from_str(&fake.bodies()[0]).unwrap();
    assert_eq!(body["model"], "abc123");
    assert_eq!(body["messages"][0]["content"], "hi \"there\"\n");
    assert_eq!(body["stream"], true);
    assert_eq!(body["cache_prompt"], false);
    assert_eq!(
        (body["max_tokens"].as_u64(), body["seed"].as_u64()),
        (Some(64), Some(7))
    );
}

#[tokio::test]
async fn events_split_across_network_chunks_and_crlf_are_reassembled() {
    let whole = chat_stream();
    let text = String::from_utf8(whole).unwrap().replace('\n', "\r\n");
    let bytes = text.into_bytes();
    let pieces: Vec<(u64, Vec<u8>)> = bytes.chunks(7).map(|c| (0, c.to_vec())).collect();
    let fake = Fake::start(move |_, _| Reply::Chunks(pieces.clone(), 0));
    let end = fake
        .client()
        .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &NO, |_| {})
        .await
        .unwrap();
    assert_eq!(end.content_events, 3);
}

#[tokio::test]
async fn reasoning_deltas_are_counted_apart_from_content() {
    let mut body = event(r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#);
    body.extend(event(r#"{"choices":[{"delta":{"content":"answer"}}]}"#));
    body.extend(b"data: [DONE]\n\n".to_vec());
    let fake = Fake::start(move |_, _| Reply::Chunks(vec![(0, body.clone())], 0));
    let end = fake
        .client()
        .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &NO, |_| {})
        .await
        .unwrap();
    assert_eq!((end.content_events, end.reasoning_events), (1, 1));
}

#[tokio::test]
async fn bad_streams_fail_with_typed_errors_and_no_response_text() {
    let cases: Vec<(Reply, LlamaError)> = vec![
        (
            json(500, "secret server internals"),
            LlamaError::Status(500),
        ),
        (json(302, "{}"), LlamaError::Status(302)),
        (
            Reply::Chunks(vec![(0, event(r#"{"error":{"message":"oom"}}"#))], 0),
            LlamaError::BadResponse,
        ),
        (
            Reply::Chunks(vec![(0, b"data: {not json}\n\n".to_vec())], 0),
            LlamaError::BadResponse,
        ),
        (
            Reply::Chunks(
                vec![(0, [b"data: ".to_vec(), vec![b'x'; MAX_LINE + 10]].concat())],
                0,
            ),
            LlamaError::TooLarge,
        ),
        (
            Reply::Chunks(
                (0..150)
                    .map(|_| (0, [b":".as_slice(), &vec![b'c'; 60_000], b"\n"].concat()))
                    .collect(),
                0,
            ),
            LlamaError::TooLarge,
        ),
    ];
    for (reply, expected) in cases {
        let cell = Arc::new(Mutex::new(Some(reply)));
        let fake = Fake::start(move |_, _| {
            cell.lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| json(500, "{}"))
        });
        let result = fake
            .client()
            .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &NO, |_| {})
            .await;
        assert_eq!(result.err(), Some(expected));
    }
}

#[tokio::test]
async fn too_many_events_are_refused() {
    let one = event(r#"{"choices":[{"delta":{"content":"a"}}]}"#);
    let chunk: Vec<u8> = one.iter().cycle().take(one.len() * 1000).copied().collect();
    let fake =
        Fake::start(move |_, _| Reply::Chunks((0..101).map(|_| (0, chunk.clone())).collect(), 0));
    let result = fake
        .client()
        .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &NO, |_| {})
        .await;
    assert_eq!(result.err(), Some(LlamaError::TooLarge));
}

#[tokio::test]
async fn cancel_returns_promptly_even_when_the_server_goes_silent() {
    let first = event(r#"{"choices":[{"delta":{"content":"a"}}]}"#);
    let fake = Fake::start(move |_, _| Reply::Chunks(vec![(0, first.clone())], 10_000));
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let result = fake
        .client()
        .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &cancel, |_| {
            cancel.store(true, Ordering::Relaxed)
        })
        .await;
    assert_eq!(result.err(), Some(LlamaError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn the_overall_deadline_ends_a_silent_stream() {
    let fake = Fake::start(|_, _| Reply::Chunks(vec![], 10_000));
    let result = fake
        .client()
        .stream_chat(&request("a", "p"), Duration::from_millis(400), &NO, |_| {})
        .await;
    assert_eq!(result.err(), Some(LlamaError::Timeout));
}

#[tokio::test]
async fn a_stream_that_ends_without_completion_is_incomplete() {
    let first = event(r#"{"choices":[{"delta":{"content":"a"}}]}"#);
    for body in [first, Vec::new()] {
        let fake = Fake::start(move |_, _| Reply::Chunks(vec![(0, body.clone())], 0));
        let result = fake
            .client()
            .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &NO, |_| {})
            .await;
        assert_eq!(result.err(), Some(LlamaError::Incomplete));
    }
}

#[tokio::test]
async fn cancelling_inside_a_buffered_chunk_stops_before_the_next_event() {
    let mut body = event(r#"{"choices":[{"delta":{"content":"a"}}]}"#);
    body.extend(event(r#"{"choices":[{"delta":{"content":"b"}}]}"#));
    body.extend(b"data: [DONE]\n\n".to_vec());
    let fake = Fake::start(move |_, _| Reply::Chunks(vec![(0, body.clone())], 0));
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    let result = fake
        .client()
        .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &cancel, |_| {
            calls += 1;
            cancel.store(true, Ordering::Relaxed)
        })
        .await;
    assert_eq!(result.err(), Some(LlamaError::Cancelled));
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn a_request_cancelled_before_sending_never_reaches_the_server() {
    let fake = Fake::start(|_, _| Reply::Chunks(vec![], 0));
    let cancel = AtomicBool::new(true);
    let result = fake
        .client()
        .stream_chat(&request("a", "p"), DEFAULT_DEADLINE, &cancel, |_| {})
        .await;
    assert_eq!(result.err(), Some(LlamaError::Cancelled));
    assert!(fake.bodies().is_empty());
}
