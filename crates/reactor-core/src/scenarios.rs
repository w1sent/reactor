//! The scenarios that ship with REactor, and where they live once seeded.
//!
//! A scenario is a directory of numbered step files under `prompts/scenarios/`. The copies in
//! the repository are compiled into the library (`build.rs`) the way `tools.toml` is, so a
//! bare binary can seed them: `reactor setup` writes them to `<config dir>/prompts/scenarios`,
//! which is where the GUI looks once it finds no checkout. Seeding never overwrites a file
//! that is there (ADR-0004): after that the files are the user's.

use std::path::{Path, PathBuf};

use crate::paths::{Paths, Shipped};

/// Where the scenarios are, below a checkout and below the config directory.
pub const DIR: &str = "prompts/scenarios";

/// The compiled-in copies: the path below the scenarios directory, and the bytes.
static EMBEDDED: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/scenarios.rs"));

/// How a seeded file stands against the shipped one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Absent,
    Same,
    Differs,
}

/// One shipped file and how the installed one compares.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Below the scenarios directory, `/`-separated: `patching/01-orientation.md`.
    pub rel: String,
    pub bytes: Vec<u8>,
    pub standing: Standing,
}

impl Paths {
    /// Where the seeded scenarios are.
    pub fn scenarios_dir(&self) -> PathBuf {
        self.config_dir.join(DIR)
    }
}

/// The shipped scenario files: those of the checkout `REACTOR_PACKAGE_ROOT` names when it has
/// any, else the compiled-in copies.
pub fn shipped(paths: &Paths) -> Vec<(String, Vec<u8>)> {
    if let Shipped::Dir(root) = &paths.shipped {
        let mut found = Vec::new();
        walk(&root.join(DIR), "", &mut found);
        if !found.is_empty() {
            return found;
        }
    }
    EMBEDDED
        .iter()
        .map(|(rel, bytes)| (rel.to_string(), bytes.to_vec()))
        .collect()
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let path = e.path();
        if path.is_dir() {
            walk(&path, &rel, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((rel, bytes));
        }
    }
}

/// Every shipped file with how the installed copy stands.
pub fn survey(paths: &Paths) -> Vec<Entry> {
    let dir = paths.scenarios_dir();
    shipped(paths)
        .into_iter()
        .map(|(rel, bytes)| {
            let standing = match std::fs::read(dir.join(&rel)) {
                Err(_) => Standing::Absent,
                Ok(have) if have == bytes => Standing::Same,
                Ok(_) => Standing::Differs,
            };
            Entry {
                rel,
                bytes,
                standing,
            }
        })
        .collect()
}

/// How many scenarios (directories with at least one step file) the seeded directory holds.
pub fn installed_count(paths: &Paths) -> usize {
    let Ok(read) = std::fs::read_dir(paths.scenarios_dir()) else {
        return 0;
    };
    read.flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            std::fs::read_dir(e.path()).is_ok_and(|steps| {
                steps
                    .flatten()
                    .any(|s| s.path().extension().is_some_and(|x| x == "md"))
            })
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_in_copies_are_the_repository_ones() {
        let on_disk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts/scenarios");
        let mut disk = Vec::new();
        walk(&on_disk, "", &mut disk);
        assert!(!disk.is_empty(), "the repository has scenarios");
        let embedded: Vec<(String, Vec<u8>)> = EMBEDDED
            .iter()
            .map(|(r, b)| (r.to_string(), b.to_vec()))
            .collect();
        assert_eq!(embedded, disk);
    }
}
