use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Lines longer than this are split so one runaway line cannot exhaust memory.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// Monotonic per-buffer sequence number, used to fetch lines incrementally.
    pub seq: u64,
    pub stream: Stream,
    /// Decoded lossily; invalid UTF-8 becomes U+FFFD.
    pub text: String,
}

/// Bounded in-memory log of a process. Writers never block on readers: once full, the
/// oldest lines are discarded.
pub struct LogBuffer {
    inner: Mutex<Ring>,
}

struct Ring {
    lines: VecDeque<LogLine>,
    capacity: usize,
    next_seq: u64,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "log buffer capacity must be positive");
        Self {
            inner: Mutex::new(Ring {
                lines: VecDeque::with_capacity(capacity.min(1024)),
                capacity,
                next_seq: 0,
            }),
        }
    }

    pub fn push(&self, stream: Stream, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut ring = self.lock();
        if ring.lines.len() == ring.capacity {
            ring.lines.pop_front();
        }
        let seq = ring.next_seq;
        ring.next_seq += 1;
        ring.lines.push_back(LogLine { seq, stream, text });
    }

    /// Total number of lines ever pushed, including discarded ones.
    pub fn total(&self) -> u64 {
        self.lock().next_seq
    }

    /// Returns retained lines with `seq >= from`, oldest first.
    pub fn since(&self, from: u64) -> Vec<LogLine> {
        let ring = self.lock();
        let skip = ring
            .lines
            .front()
            .map_or(0, |first| from.saturating_sub(first.seq) as usize);
        ring.lines.iter().skip(skip).cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Reads `reader` to EOF, pushing each line (without the trailing `\n` / `\r\n`) to `log`.
pub(crate) async fn pump_lines<R>(mut reader: R, stream: Stream, log: &LogBuffer)
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    // Set after a line was cut at MAX_LINE_BYTES, so a newline right at the cut is not
    // reported as an extra empty line.
    let mut cut = false;
    loop {
        let chunk = match reader.fill_buf().await {
            Ok(chunk) => chunk,
            Err(err) => {
                tracing::warn!(?stream, %err, "failed to read process output");
                break;
            }
        };
        if chunk.is_empty() {
            break;
        }
        if std::mem::take(&mut cut) && chunk[0] == b'\n' {
            reader.consume(1);
            continue;
        }

        let room = MAX_LINE_BYTES - line.len();
        let window = &chunk[..chunk.len().min(room)];
        let (taken, complete) = match window.iter().position(|&b| b == b'\n') {
            Some(i) => {
                line.extend_from_slice(&window[..i]);
                (i + 1, true)
            }
            None => {
                line.extend_from_slice(window);
                cut = line.len() == MAX_LINE_BYTES;
                (window.len(), cut)
            }
        };
        reader.consume(taken);

        if complete {
            emit(&mut line, stream, log);
        }
    }
    if !line.is_empty() {
        emit(&mut line, stream, log);
    }
}

fn emit(line: &mut Vec<u8>, stream: Stream, log: &LogBuffer) {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    log.push(stream, line);
    line.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(log: &LogBuffer) -> Vec<String> {
        log.since(0).into_iter().map(|l| l.text).collect()
    }

    async fn pump(input: &[u8], log: &LogBuffer) {
        pump_lines(input, Stream::Stdout, log).await;
    }

    #[tokio::test]
    async fn splits_lines_and_handles_crlf_and_missing_newline() {
        let log = LogBuffer::new(100);
        pump(b"a\r\nb\n\nlast", &log).await;
        assert_eq!(texts(&log), ["a", "b", "", "last"]);
    }

    #[tokio::test]
    async fn splits_overlong_lines() {
        let log = LogBuffer::new(100);
        let input = vec![b'x'; MAX_LINE_BYTES * 2 + 10];
        pump(&input, &log).await;
        let lens: Vec<_> = texts(&log).iter().map(String::len).collect();
        assert_eq!(lens, [MAX_LINE_BYTES, MAX_LINE_BYTES, 10]);
    }

    #[tokio::test]
    async fn newline_right_after_cut_is_not_an_extra_line() {
        let log = LogBuffer::new(100);
        let mut input = vec![b'x'; MAX_LINE_BYTES];
        input.extend_from_slice(b"\nnext\n");
        pump(&input, &log).await;
        let lens: Vec<_> = texts(&log).iter().map(String::len).collect();
        assert_eq!(lens, [MAX_LINE_BYTES, 4]);
    }

    #[tokio::test]
    async fn decodes_invalid_utf8_lossily() {
        let log = LogBuffer::new(100);
        pump(b"ok \xff\xfe\n", &log).await;
        assert_eq!(texts(&log), ["ok \u{fffd}\u{fffd}"]);
    }

    #[test]
    fn ring_discards_oldest_and_keeps_sequence() {
        let log = LogBuffer::new(3);
        for i in 0..5 {
            log.push(Stream::Stdout, i.to_string().as_bytes());
        }
        assert_eq!(log.total(), 5);
        assert_eq!(texts(&log), ["2", "3", "4"]);
        let tail: Vec<_> = log.since(4).into_iter().map(|l| l.seq).collect();
        assert_eq!(tail, [4]);
        assert!(log.since(5).is_empty());
    }
}
