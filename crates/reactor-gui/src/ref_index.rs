//! What `@` in the composer completes from: the files of the working directory, and every kind
//! of reference of the case ([`reactor_agent::references`] says what they name;
//! [ADR-0066](../../../docs/adr/0066-the-case-is-addressed-by-references.md)).

use std::ops::Range;
use std::path::Path;
use std::time::{Duration, Instant};

/// Directories never listed: version control, build output, dependencies.
const SKIPPED: [&str; 6] = [".git", "target", "node_modules", ".venv", "__pycache__", ".reactor"];

/// The most files listed, and the deepest folder looked in.
const MAX_FILES: usize = 20_000;
const MAX_DEPTH: usize = 10;

/// How long a scan is trusted before the folder is read again.
const FRESH: Duration = Duration::from_secs(8);

/// A file of the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// Relative to the working directory, with `/`.
    pub path: String,
    pub bytes: u64,
}

#[derive(Default)]
pub struct FileIndex {
    root: std::path::PathBuf,
    built: Option<Instant>,
    pub files: Vec<FileRow>,
}

impl FileIndex {
    /// The files under `root`, read again when the scan is older than a few seconds.
    pub fn refresh(&mut self, root: &Path) {
        if self.root == root && self.built.is_some_and(|t| t.elapsed() < FRESH) {
            return;
        }
        self.root = root.to_path_buf();
        self.files.clear();
        scan(root, root, 0, &mut self.files);
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.built = Some(Instant::now());
    }
}

fn scan(root: &Path, dir: &Path, depth: usize, out: &mut Vec<FileRow>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if name.starts_with('.') || SKIPPED.contains(&name.as_str()) {
                continue;
            }
            scan(root, &entry.path(), depth + 1, out);
        } else if kind.is_file() {
            let Ok(rel) = entry.path().strip_prefix(root).map(|p| p.to_path_buf()) else {
                continue;
            };
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(FileRow {
                path: rel.to_string_lossy().replace('\\', "/"),
                bytes,
            });
        }
    }
}

/// Whether `candidate` has enough of `query`'s letters to be worth ranking: three quarters of
/// them, in any order (the ranker is stricter). Keeps a typo from hiding a file, and keeps a
/// keystroke from ranking every file.
pub fn plausible(query: &str, candidate: &str) -> bool {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    if q.is_empty() {
        return true;
    }
    let c = candidate.to_lowercase();
    let have = q.iter().filter(|ch| c.contains(**ch)).count();
    have * 4 >= q.len() * 3
}

/// Bytes, as people read them.
pub fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} kB", b / 1024),
        b => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

/// What kind of reference is being typed: a file (after a bare `@`), or one kind of the
/// grammar (`@graph:`, `@doc:`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    File,
    Kind(reactor_db::reference::Kind),
}

/// The reference being typed at `cursor`: its kind, what was typed after the trigger, and the
/// byte range of the whole token (`@` included), which is what a completion replaces.
pub fn reference_at(text: &str, cursor: usize) -> Option<(Trigger, String, Range<usize>)> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let start = before
        .rfind(char::is_whitespace)
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let token = &before[start..];
    let rest = token.strip_prefix('@')?;
    // The rest of the word after the cursor is replaced with it.
    let after = &text[cursor..];
    let tail = after.find(char::is_whitespace).unwrap_or(after.len());
    let end = cursor + tail;
    let kind = rest
        .split_once(':')
        .and_then(|(scheme, q)| reactor_db::reference::Kind::from_scheme(scheme).map(|k| (k, q)));
    Some(match kind {
        Some((kind, q)) => (Trigger::Kind(kind), q.trim_matches('"').to_string(), start..end),
        None => (Trigger::File, rest.trim_matches('"').to_string(), start..end),
    })
}

/// A reference's text as inserted: the `@` and the reference itself, which is already written
/// so that nothing in it needs quoting.
pub fn written(reference: &str) -> String {
    format!("@{reference}")
}

/// The (line, UTF-16 column) of a byte offset, which is what the composer's cursor is set in.
pub fn position(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let column: usize = before
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .map(char::len_utf16)
        .sum();
    (line, column as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_found_at_the_cursor() {
        let text = "compare @logs/ac with the rest";
        let (t, q, r) = reference_at(text, "compare @logs/ac".len()).unwrap();
        assert_eq!(t, Trigger::File);
        assert_eq!(q, "logs/ac");
        assert_eq!(&text[r], "@logs/ac");
    }

    #[test]
    fn a_kind_is_its_own_trigger_and_at_alone_is_a_file_trigger() {
        use reactor_db::reference::Kind;
        let (t, q, _) = reference_at("see @doc:ps-man", 15).unwrap();
        assert_eq!((t, q.as_str()), (Trigger::Kind(Kind::Doc), "ps-man"));
        let (t, q, _) = reference_at("@graph:beac", 11).unwrap();
        assert_eq!((t, q.as_str()), (Trigger::Kind(Kind::Graph), "beac"));
        let (t, q, _) = reference_at("@", 1).unwrap();
        assert_eq!((t, q.as_str()), (Trigger::File, ""));
        assert!(reference_at("no reference here", 5).is_none());
        // An at inside a word is not one.
        assert!(reference_at("me@", 3).is_none());
    }

    #[test]
    fn what_is_inserted_is_the_reference_with_its_at() {
        assert_eq!(written("artifact:a%20b.txt"), "@artifact:a%20b.txt");
    }

    #[test]
    fn plausible_forgives_a_missing_letter() {
        assert!(plausible("acess", "logs/access-2026-04.txt"));
        assert!(!plausible("zzzz", "logs/access-2026-04.txt"));
        assert!(plausible("", "anything"));
    }

    #[test]
    fn positions_count_lines_and_utf16_columns() {
        assert_eq!(position("ab\ncd", 4), (1, 1));
        assert_eq!(position("é@", 3), (0, 2));
    }
}
