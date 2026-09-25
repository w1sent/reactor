//! JSONL framing for pi's RPC mode.
//!
//! pi's docs are explicit about the framing: records are delimited by `\n`
//! **only**; a client must not treat Unicode separators (`U+2028`, `U+2029`)
//! as newlines because they are valid inside JSON strings, and must accept an
//! optional trailing `\r` (CRLF writers). Splitting bytes on `b'\n'` gives both
//! properties by construction: those code points are multi-byte UTF-8 whose
//! bytes never collide with `0x0A`, so they stay inside the record.

use std::io::{self, Write};

/// Accumulates bytes and yields complete JSONL records.
///
/// Feeds whatever the stream gave you; complete lines come back as lossy
/// UTF-8 strings. A record that never terminates (peer died mid-line) is
/// retrievable via [`LineSplitter::finish`] — a half-written line is still
/// worth showing in a debug view rather than dropping.
#[derive(Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed a chunk; returns every complete line it completed, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // the delimiter
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        out
    }

    /// Whatever is left without a terminator — `None` when the stream ended
    /// on a clean record boundary.
    pub fn finish(mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        if self.buf.last() == Some(&b'\r') {
            self.buf.pop();
        }
        Some(String::from_utf8_lossy(&self.buf).into_owned())
    }
}

/// Serialize one JSON value as a single `\n`-terminated record and flush.
///
/// The flush is part of the contract: pi only sees a command once its line is
/// complete, and a GUI that batches writes would hang the agent for a frame.
pub fn write_line<W: Write>(out: &mut W, value: &serde_json::Value) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_on_lf_only() {
        let mut s = LineSplitter::new();
        assert_eq!(
            s.push(b"{\"a\":1}\n{\"b\":2}\n"),
            vec!["{\"a\":1}", "{\"b\":2}"]
        );
    }

    /// U+2028 (E2 80 A8) is a line separator to generic readers and valid
    /// inside a JSON string. pi's docs warn Node's readline is non-compliant
    /// for exactly this reason; our splitter must keep the record whole.
    #[test]
    fn unicode_separators_stay_inside_the_record() {
        let mut s = LineSplitter::new();
        let line = "{\"text\":\"line next\"}\n";
        let got = s.push(line.as_bytes());
        assert_eq!(got.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&got[0]).unwrap();
        assert_eq!(v["text"], "line next");
    }

    /// A `\r` is stripped only when it precedes the `\n` — pi's docs say to
    /// accept optional CRLF, not to split on carriage returns. A bare `\r`
    /// stays inside the record (a `\r` is a normal character mid-line).
    #[test]
    fn strips_trailing_carriage_return() {
        let mut s = LineSplitter::new();
        assert_eq!(s.push(b"{}\r\n"), vec!["{}"]);
        // No newline yet: the `\r` is still inside an unterminated record.
        assert!(s.push(b"{}\r").is_empty());
        // The next `\n` closes the record, and the `\r` goes with it.
        assert_eq!(s.push(b"\n"), vec!["{}"]);
    }

    #[test]
    fn partial_chunks_reassemble() {
        let mut s = LineSplitter::new();
        assert!(s.push(b"{\"a\"").is_empty());
        // The same chunk completes one record and opens the next.
        assert_eq!(s.push(b":1}\n{"), vec!["{\"a\":1}"]);
        assert_eq!(s.push(b"}\n"), vec!["{}"]);
    }

    #[test]
    fn finish_yields_trailing_partial() {
        let mut s = LineSplitter::new();
        assert!(s.push(b"{\"partial\"").is_empty());
        assert_eq!(s.finish().as_deref(), Some("{\"partial\""));
        assert_eq!(LineSplitter::new().finish(), None);
    }

    #[test]
    fn write_line_is_one_flushed_record() {
        let mut buf = Vec::new();
        write_line(&mut buf, &json!({"type": "prompt", "message": "hi"})).unwrap();
        // Compare as JSON, not bytes: serde_json sorts map keys, so the
        // field order in the record is not the field order of the literal.
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed, json!({"type": "prompt", "message": "hi"}));
        assert_eq!(buf.last(), Some(&b'\n'));
    }
}
