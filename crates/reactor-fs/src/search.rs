//! Finding files by name and by content, through [`Vfs`] alone — so it works the same on a
//! mounted image as on the working directory.

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use regex::RegexBuilder;

use crate::VPath;
use crate::vfs::{Kind, ReadAt, Vfs};

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub case_insensitive: bool,
    /// Stop after this many results.
    pub limit: usize,
    /// Content search skips files larger than this.
    pub max_file_size: u64,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_insensitive: true,
            limit: 2000,
            max_file_size: 32 << 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHit {
    pub path: VPath,
    pub line: u64,
    pub text: String,
}

/// Visits every file and folder below `root`, depth first, in listing order. Unreadable
/// folders are skipped: a search is not stopped by one permission error.
fn walk(
    fs: &dyn Vfs,
    root: &VPath,
    cancel: &AtomicBool,
    visit: &mut dyn FnMut(&VPath, Kind) -> bool,
) {
    let Ok(entries) = fs.read_dir(root) else {
        return;
    };
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if !visit(&entry.path, entry.kind) {
            return;
        }
        // A symlinked folder is listed but not entered, so a loop cannot hang the walk.
        if entry.kind == Kind::Dir && !entry.is_symlink {
            walk(fs, &entry.path, cancel, visit);
        }
    }
}

/// `*` and `?` make the query a glob matched against the whole file name; anything else is
/// a substring of the path relative to `root`.
fn name_matcher(query: &str, case_insensitive: bool) -> Result<regex::Regex, regex::Error> {
    let pattern = if query.contains(['*', '?']) {
        let body: String = query
            .chars()
            .map(|c| match c {
                '*' => ".*".to_owned(),
                '?' => ".".to_owned(),
                c => regex::escape(&c.to_string()),
            })
            .collect();
        format!("^{body}$")
    } else {
        regex::escape(query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(case_insensitive)
        .build()
}

pub fn search_names(
    fs: &dyn Vfs,
    root: &VPath,
    query: &str,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<Vec<VPath>, String> {
    let matcher = name_matcher(query, options.case_insensitive).map_err(|e| e.to_string())?;
    let glob = query.contains(['*', '?']);
    let mut found = Vec::new();
    walk(fs, root, cancel, &mut |path, _| {
        let haystack = if glob {
            path.file_name().unwrap_or("")
        } else {
            path.relative()
        };
        if matcher.is_match(haystack) {
            found.push(path.clone());
        }
        found.len() < options.limit
    });
    Ok(found)
}

/// Adapts random access to the `Read` the searcher wants.
struct Sequential<'a> {
    file: &'a dyn ReadAt,
    offset: u64,
}

impl Read for Sequential<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read_at(self.offset, buf)?;
        self.offset += n as u64;
        Ok(n)
    }
}

/// A regex search of file contents, ripgrep-style: binary files are skipped, results arrive
/// as `(path, line, text)`.
pub fn search_content(
    fs: &dyn Vfs,
    root: &VPath,
    pattern: &str,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<Vec<ContentHit>, String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(options.case_insensitive)
        .build(pattern)
        .map_err(|e| e.to_string())?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();
    let mut hits: Vec<ContentHit> = Vec::new();
    walk(fs, root, cancel, &mut |path, kind| {
        if kind != Kind::File {
            return true;
        }
        let Ok(file) = fs.open(path) else { return true };
        if file.len() > options.max_file_size {
            return true;
        }
        let reader = Sequential {
            file: file.as_ref(),
            offset: 0,
        };
        let _ = searcher.search_reader(
            &matcher,
            reader,
            UTF8(|line, text| {
                hits.push(ContentHit {
                    path: path.clone(),
                    line,
                    text: text.trim_end().chars().take(300).collect(),
                });
                Ok(hits.len() < options.limit && !cancel.load(Ordering::Relaxed))
            }),
        );
        hits.len() < options.limit
    });
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalFs;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, LocalFs) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("src/deep/notes.txt"), "say hello\nbye\n").unwrap();
        fs::write(dir.path().join("blob.bin"), b"hello\0\0\0").unwrap();
        let fs = LocalFs::new(dir.path(), ".");
        (dir, fs)
    }

    #[test]
    fn names_by_substring_and_glob() {
        let (_dir, fs) = fixture();
        let no = AtomicBool::new(false);
        let opts = SearchOptions::default();
        let sub = search_names(&fs, &VPath::root(), "NOTES", &opts, &no).unwrap();
        assert_eq!(sub, [VPath::new("/src/deep/notes.txt")]);
        let glob = search_names(&fs, &VPath::root(), "*.rs", &opts, &no).unwrap();
        assert_eq!(glob, [VPath::new("/src/main.rs")]);
    }

    #[test]
    fn content_regex_skips_binary_and_reports_lines() {
        let (_dir, fs) = fixture();
        let hits = search_content(
            &fs,
            &VPath::root(),
            "hel+o",
            &SearchOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let places: Vec<_> = hits.iter().map(|h| (h.path.as_str(), h.line)).collect();
        assert_eq!(places, [("/src/deep/notes.txt", 1), ("/src/main.rs", 2)]);
    }

    #[test]
    fn a_bad_pattern_is_an_error_not_a_panic() {
        let (_dir, fs) = fixture();
        let result = search_content(
            &fs,
            &VPath::root(),
            "(",
            &SearchOptions::default(),
            &AtomicBool::new(false),
        );
        assert!(result.is_err());
    }
}
