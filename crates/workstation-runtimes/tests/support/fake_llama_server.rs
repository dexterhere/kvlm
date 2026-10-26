// SPDX-License-Identifier: GPL-3.0-or-later
//! Test double for `llama-server`. Reads `--model`, `--port`, `--alias`; the first line of the
//! model file selects a behaviour: serve, loading, wrong_alias, exit_early, crash_after,
//! flood, noise, hang, echo_env.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--version") {
        println!("fake llama-server");
        return;
    }
    let mode = std::fs::read_to_string(get("--model").unwrap_or_default())
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or("serve")
        .trim()
        .to_owned();
    let port: u16 = get("--port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let alias = get("--alias").unwrap_or_default();
    eprintln!("fake: mode={mode} host={:?}", get("--host"));

    match mode.as_str() {
        "exit_early" => {
            eprintln!("boom: cannot load model");
            std::process::exit(3);
        }
        "hang" => loop {
            thread::sleep(Duration::from_secs(60));
        },
        "noise" => {
            println!("{}", "x".repeat(100_000));
            println!("\u{1b}[31mred\u{1b}[0m \u{0} bell\u{7}");
        }
        "flood" => {
            thread::spawn(|| {
                let out = std::io::stdout();
                loop {
                    let mut lock = out.lock();
                    if writeln!(lock, "flood line").is_err() {
                        break;
                    }
                }
            });
        }
        "echo_env" => {
            for (k, v) in std::env::vars() {
                println!("env {k}={v}");
            }
        }
        m if m.starts_with("crash_after") => {
            let ms: u64 = m
                .trim_start_matches("crash_after")
                .trim()
                .parse()
                .unwrap_or(500);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                std::process::exit(9);
            });
        }
        _ => {}
    }

    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    for stream in listener.incoming().flatten() {
        let (mode, alias) = (mode.clone(), alias.clone());
        thread::spawn(move || answer(stream, &mode, &alias));
    }
}

fn answer(mut stream: TcpStream, mode: &str, alias: &str) {
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let line = request.lines().next().unwrap_or("");
    let (code, body) = if line.starts_with("GET /health") {
        if mode == "loading" {
            (503, r#"{"error":"loading model"}"#.to_owned())
        } else {
            (200, r#"{"status":"ok"}"#.to_owned())
        }
    } else if line.starts_with("GET /v1/models") {
        let id = if mode == "wrong_alias" {
            "someone-else"
        } else {
            alias
        };
        (200, format!(r#"{{"data":[{{"id":"{id}"}}]}}"#))
    } else {
        (404, "{}".to_owned())
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
