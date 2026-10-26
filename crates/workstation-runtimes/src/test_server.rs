// SPDX-License-Identifier: GPL-3.0-or-later
//! Std-only loopback HTTP mock shared by the Hugging Face and catalog service tests.

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

use serde_json::Value;

use crate::huggingface::HuggingFaceClient;

/// How long a mock answers requests after it starts; tests finish well inside this.
const SERVER_LIFETIME: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) enum Reply {
    Json {
        body: String,
        headers: Vec<(String, String)>,
    },
    Status {
        code: u16,
        headers: Vec<(String, String)>,
    },
    Redirect(String),
    /// Raw bytes with explicit status and headers (ranged file replies).
    Bytes {
        code: u16,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    },
    /// Declares 5 MiB, sends a few bytes, then closes.
    DeclaredOversize,
    /// No length header; streams more than the 4 MiB cap, then closes.
    StreamedOversize,
}

pub(crate) fn json_reply(value: Value) -> Reply {
    Reply::Json {
        body: value.to_string(),
        headers: Vec::new(),
    }
}

pub(crate) fn status(code: u16) -> Reply {
    Reply::Status {
        code,
        headers: Vec::new(),
    }
}

pub(crate) fn with_header(code: u16, name: &str, value: &str) -> Reply {
    Reply::Status {
        code,
        headers: vec![(name.to_owned(), value.to_owned())],
    }
}

pub(crate) fn ranged(first: u64, body: &[u8], total: u64) -> Reply {
    Reply::Bytes {
        code: 206,
        body: body.to_vec(),
        headers: vec![(
            "Content-Range".to_owned(),
            format!("bytes {first}-{}/{total}", first + body.len() as u64 - 1),
        )],
    }
}

pub(crate) fn route(line: &str, reply: Reply) -> (String, Reply) {
    (line.to_owned(), reply)
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        206 => "Partial Content",
        302 => "Found",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

pub(crate) fn head(code: u16, headers: &[(String, String)], length: Option<usize>) -> String {
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
            Self::Bytes {
                code,
                body,
                headers,
            } => {
                stream.write_all(head(*code, headers, Some(body.len())).as_bytes())?;
                stream.write_all(body)
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
pub(crate) struct RecordedRequest {
    pub(crate) line: String,
    /// Header lines as sent, one per line, lowercased names.
    pub(crate) headers: Vec<String>,
}

pub(crate) struct Server {
    port: u16,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Routes are built with the bound port, so a `Link` header can point back at this server.
    pub(crate) fn start(build: impl FnOnce(u16) -> Vec<(String, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let routes = build(port);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorded = Arc::clone(&requests);
        let stopping = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + SERVER_LIFETIME;
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

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn client(&self) -> HuggingFaceClient {
        HuggingFaceClient::for_test(self.port)
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.line.clone())
            .collect()
    }

    pub(crate) fn last(&self) -> RecordedRequest {
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
