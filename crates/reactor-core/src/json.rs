//! JSON in the exact byte shape the Python CLI produced (ADR-0034 freezes it).
//!
//! Python's `json.dump(indent=2)` differs from serde_json's pretty printer in
//! one way that matters: `ensure_ascii` is on, so every character outside
//! printable ASCII leaves as a `\uXXXX` escape (surrogate pairs above the BMP).
//! Harnesses parse this and the registry block is hashed into a prompt
//! cache, so the bytes are the contract, not just the meaning.

use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::{ReactorError, Result};

/// `json.dumps(v, indent=2)` — keys in the order the value serializes them.
pub fn to_string_pretty<T: Serialize>(value: &T) -> String {
    let raw = serde_json::to_string_pretty(value).expect("serializing a payload cannot fail");
    escape_non_ascii(raw)
}

/// `json.dumps(v, indent=2, sort_keys=True)`. Goes through [`Value`], whose map
/// is a `BTreeMap`, so keys come out sorted at every depth. (A test pins this:
/// enabling serde_json's `preserve_order` anywhere in the graph would silently
/// change file bytes.)
pub fn to_string_sorted<T: Serialize>(value: &T) -> String {
    let value: Value = serde_json::to_value(value).expect("serializing a payload cannot fail");
    to_string_pretty(&value)
}

/// Non-ASCII can only occur inside string literals (serde emits pure ASCII for
/// everything structural), so a flat pass over the output is exactly right.
fn escape_non_ascii(s: String) -> String {
    if s.bytes().all(|b| b < 0x7f) {
        return s;
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        // Python's ESCAPE_ASCII is `[^ -~]`: DEL goes too.
        if c.is_ascii() && c != '\x7f' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{:04x}", u);
            }
        }
    }
    out
}

/// Atomic write of a sorted-key JSON file with a trailing newline.
///
/// The temp name carries the pid because two processes can be here at once:
/// every extension shells out on its own and shares nothing but this cache
/// (ADR-0014). The rename is atomic, so a reader never sees half a file — but a
/// *shared* temp name lets two writers interleave into it and then rename the
/// mixture, publishing a state that was never anyone's.
pub fn write_json_atomic<T: Serialize>(path: &Path, data: &T) -> Result<()> {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let body = to_string_sorted(data) + "\n";
    let attempt = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)
    })();
    attempt.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ReactorError::new(format!("{}: {}", path.display(), io_reason(&e)))
    })
}

/// io::Error without the "(os error N)" suffix Rust appends, which Python's
/// messages never had.
pub fn io_reason(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}
