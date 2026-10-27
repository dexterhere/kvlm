// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded capture of a child's stdout and stderr. A child that floods output, writes
//! control characters, or never ends a line cannot grow memory past `MAX_LINES * MAX_LINE`.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_LINES: usize = 2_000;
pub const MAX_LINE: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub stderr: bool,
    pub text: String,
}

#[derive(Default)]
struct Inner {
    lines: VecDeque<LogLine>,
    /// Bumped on every push so a viewer can tell whether anything changed.
    version: u64,
}

#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Inner>>);

impl LogBuffer {
    fn with<T>(&self, work: impl FnOnce(&mut Inner) -> T) -> T {
        work(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn push(&self, stderr: bool, text: String) {
        self.with(|inner| {
            if inner.lines.len() == MAX_LINES {
                inner.lines.pop_front();
            }
            inner.lines.push_back(LogLine { stderr, text });
            inner.version += 1;
        });
    }

    /// The newest `count` lines, oldest first.
    pub fn tail(&self, count: usize) -> Vec<LogLine> {
        self.with(|inner| {
            let skip = inner.lines.len().saturating_sub(count);
            inner.lines.iter().skip(skip).cloned().collect()
        })
    }

    pub fn version(&self) -> u64 {
        self.with(|inner| inner.version)
    }

    pub fn clear(&self) {
        self.with(|inner| {
            inner.lines.clear();
            inner.version += 1;
        });
    }
}

/// Lossy UTF-8 with every control character other than tab replaced, so a log line can
/// never carry terminal escapes into the interface.
pub fn sanitize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| {
            if c != '\t' && c.is_control() {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// Read `reader` to the end, splitting lines with a hard length cap and keeping draining
/// after the cap so the child never blocks on a full pipe.
pub async fn pump(mut reader: impl AsyncRead + Unpin, log: LogBuffer, stderr: bool) {
    let mut chunk = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut cut = false;
    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                emit(&log, stderr, &mut line, &mut cut);
            } else if line.len() < MAX_LINE {
                line.push(byte);
            } else {
                cut = true;
            }
        }
    }
    if !line.is_empty() || cut {
        emit(&log, stderr, &mut line, &mut cut);
    }
}

fn emit(log: &LogBuffer, stderr: bool, line: &mut Vec<u8>, cut: &mut bool) {
    let mut text = sanitize(line);
    if std::mem::take(cut) {
        text.push('…');
    }
    line.clear();
    log.push(stderr, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lines_are_split_capped_and_sanitized() {
        let log = LogBuffer::default();
        let mut input = Vec::new();
        input.extend_from_slice(b"hello\n\x1b[31mred\x00\n");
        input.extend_from_slice(&vec![b'a'; MAX_LINE * 3]);
        input.extend_from_slice(b"\nlast without newline");
        pump(&input[..], log.clone(), true).await;
        let lines = log.tail(10);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[1].text, "\u{fffd}[31mred\u{fffd}");
        assert_eq!(lines[2].text.chars().count(), MAX_LINE + 1);
        assert!(lines[2].text.ends_with('…'));
        assert_eq!(lines[3].text, "last without newline");
        assert!(lines.iter().all(|l| l.stderr));
    }

    #[tokio::test]
    async fn a_flood_keeps_only_the_newest_lines() {
        let log = LogBuffer::default();
        let input: Vec<u8> = (0..MAX_LINES * 3)
            .flat_map(|i| format!("line {i}\n").into_bytes())
            .collect();
        pump(&input[..], log.clone(), false).await;
        let all = log.tail(usize::MAX);
        assert_eq!(all.len(), MAX_LINES);
        assert_eq!(
            all.last().unwrap().text,
            format!("line {}", MAX_LINES * 3 - 1)
        );
        assert_eq!(log.tail(2).len(), 2);
        let before = log.version();
        log.clear();
        assert!(log.tail(5).is_empty() && log.version() > before);
    }
}
