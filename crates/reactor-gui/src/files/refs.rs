//! File references in the transcript: turning `src/main.rs:42` in the assistant's text into a
//! link that opens the file, and reading such a link back.
//!
//! A word is only linked when it names something that exists, so `e.g.` and `v1.2` stay
//! text. Code blocks are left alone; inline code that is a path becomes a link around the
//! code span, since that is how paths are usually written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;

pub const SCHEME: &str = "reactor-file:";

static CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:~/|/|\.{1,2}/)?[A-Za-z0-9_@+\-.]+(?:/[A-Za-z0-9_@+\-.]+)*(?::\d+(?::\d+)?)?")
        .unwrap()
});

/// The native path a reference names: relative to `cwd`, `~/` from the home directory.
pub fn native_path(cwd: &Path, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Path::new(&home).join(rest);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

static KNOWN: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);

/// Whether the reference names a file or folder that exists. Yeses are remembered — the
/// transcript asks again on every frame — and a no is asked again, since the file may be
/// created later.
pub fn exists(cwd: &Path, path: &str) -> bool {
    let native = native_path(cwd, path);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known.contains(&native) {
        return true;
    }
    if native.exists() {
        if known.len() > 4096 {
            known.clear();
        }
        known.insert(native);
        return true;
    }
    false
}

/// `src/a.rs:42:7` -> (`src/a.rs`, Some(42)).
pub fn split_line(token: &str) -> (&str, Option<u32>) {
    let mut path = token;
    let mut numbers = Vec::new();
    // Up to `:line:column` come off the end; the one nearest the path is the line.
    while numbers.len() < 2
        && let Some((head, tail)) = path.rsplit_once(':')
        && let Ok(number) = tail.parse::<u32>()
        && !head.is_empty()
    {
        numbers.push(number);
        path = head;
    }
    (path, numbers.last().copied())
}

fn is_path_like(path: &str) -> bool {
    if path.ends_with('.') || path.starts_with("http") {
        return false;
    }
    path.contains('/')
        || path
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
}

fn encode(token: &str) -> String {
    let mut out = String::from(SCHEME);
    for byte in token.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'/'
            | b'.'
            | b'_'
            | b'-'
            | b'@'
            | b'+'
            | b'~' => out.push(byte as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The path and line a [`SCHEME`] link carries.
pub fn parse_url(url: &str) -> Option<(String, Option<u32>)> {
    let token = decode(url.strip_prefix(SCHEME)?)?;
    let (path, line) = split_line(&token);
    Some((path.to_owned(), line))
}

fn link_plain(text: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in CANDIDATE.find_iter(text) {
        let token = found.as_str();
        let (path, _) = split_line(token);
        let before = text[..found.start()].chars().next_back();
        let after_link = text[..found.start()].ends_with("](");
        if !is_path_like(path) || after_link || before == Some(':') || !exists(path) {
            continue;
        }
        out.push_str(&text[last..found.start()]);
        out.push_str(&format!("[{token}]({})", encode(token)));
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

fn link_line(line: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        out.push_str(&link_plain(&rest[..start], exists));
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else {
            // An unclosed backtick: the rest is plain text.
            out.push_str(&rest[start..]);
            return out;
        };
        let code = &after[..end];
        let (path, _) = split_line(code.trim());
        if is_path_like(path) && !code.contains(' ') && exists(path) {
            out.push_str(&format!("[`{code}`]({})", encode(code.trim())));
        } else {
            out.push_str(&rest[start..start + end + 2]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(&link_plain(rest, exists));
    out
}

/// `text` with the existing files it names turned into links.
pub fn linkify(text: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push(line.to_owned());
        } else if fenced {
            out.push(line.to_owned());
        } else {
            out.push(link_line(line, exists));
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(path: &str) -> bool {
        matches!(
            path,
            "src/main.rs" | "Cargo.toml" | "/etc/hosts" | "a b/c.txt"
        )
    }

    #[test]
    fn splits_the_line_suffix() {
        assert_eq!(split_line("src/a.rs"), ("src/a.rs", None));
        assert_eq!(split_line("src/a.rs:42"), ("src/a.rs", Some(42)));
        assert_eq!(split_line("src/a.rs:42:7"), ("src/a.rs", Some(42)));
        assert_eq!(split_line("C:x"), ("C:x", None));
    }

    #[test]
    fn links_existing_files_in_prose_and_code() {
        let out = linkify(
            "See src/main.rs:42 and `Cargo.toml`, not foo/bar.rs.",
            &known,
        );
        assert!(
            out.contains("[src/main.rs:42](reactor-file:src/main.rs%3A42)"),
            "{out}"
        );
        assert!(
            out.contains("[`Cargo.toml`](reactor-file:Cargo.toml)"),
            "{out}"
        );
        assert!(out.contains("not foo/bar.rs."), "{out}");
    }

    #[test]
    fn leaves_code_blocks_urls_and_existing_links_alone() {
        let text = "```\nsrc/main.rs\n```\nhttps://example.com/src/main.rs\n[x](src/main.rs)";
        assert_eq!(linkify(text, &known), text);
    }

    #[test]
    fn urls_round_trip() {
        let out = linkify("`/etc/hosts`", &known);
        let url = out.split('(').nth(1).unwrap().trim_end_matches(')');
        assert_eq!(parse_url(url), Some(("/etc/hosts".into(), None)));
        assert_eq!(
            parse_url("reactor-file:src/main.rs%3A42"),
            Some(("src/main.rs".into(), Some(42)))
        );
        assert_eq!(parse_url("https://x"), None);
    }

    #[test]
    fn sentences_are_not_paths() {
        let text = "e.g. version 1.2, and the end.";
        assert_eq!(linkify(text, &known), text);
    }
}
