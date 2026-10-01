//! The session store: an append-only JSONL log, a parent pointer per entry, and a
//! derived, disposable index ([ADR-0036](../../../docs/adr/0036-reactor-owns-its-session-store-format.md)).
//!
//! ```text
//! <session dir>/
//!   session.jsonl   one entry per line, written once, never rewritten
//!   index.json      derived: rebuilt from the log whenever it is missing or stale
//!   blobs/          the whole of any tool output the model saw only part of
//! ```
//!
//! **Append-only is the crash story.** A killed process loses at most the line it
//! was writing; on open, a torn last line is dropped and the file is trimmed back
//! to the last complete one so the next append cannot land on the debris. There is
//! no rebuild step and no lock file.
//!
//! **A branch is a parent pointer.** Entry ids are dense (`id == index in the log`),
//! `head` is the tip new entries hang from, and moving `head` back to an older entry
//! and appending forks the tree without touching a byte already written.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use reactor_core::json::write_json_atomic;
use serde::{Deserialize, Serialize};

use crate::entry::{Blob, Entry, EntryId, Kind};
use crate::error::{Error, Result};

pub const FORMAT_VERSION: u32 = 1;

pub struct Store {
    dir: PathBuf,
    log: File,
    entries: Vec<Entry>,
    head: EntryId,
    /// Bytes in the log file, for the index's staleness check.
    log_bytes: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Store {
    /// A new session in `dir` (which must not already hold one).
    pub fn create(dir: impl Into<PathBuf>, session: &str, cwd: &Path) -> Result<Store> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let path = dir.join("session.jsonl");
        if path.exists() {
            return Err(Error::Store(format!(
                "{}: a session already lives here",
                dir.display()
            )));
        }
        let log = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        let mut store = Store {
            dir,
            log,
            entries: Vec::new(),
            head: 0,
            log_bytes: 0,
        };
        store.push(
            None,
            Kind::Session {
                version: FORMAT_VERSION,
                session: session.to_string(),
                cwd: cwd.display().to_string(),
            },
        )?;
        Ok(store)
    }

    /// Open an existing session, tolerating a torn last line.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Store> {
        let dir = dir.into();
        let path = dir.join("session.jsonl");
        let file = File::open(&path).map_err(|e| Error::io(&path, e))?;
        let mut reader = BufReader::new(file);
        let (mut entries, mut good_bytes) = (Vec::<Entry>::new(), 0u64);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .map_err(|e| Error::io(&path, e))?;
            if n == 0 {
                break;
            }
            let complete = line.ends_with('\n');
            match serde_json::from_str::<Entry>(line.trim_end()) {
                Ok(entry) if complete => {
                    if entry.id as usize != entries.len() {
                        return Err(Error::Store(format!(
                            "{}: entry ids are not dense (found {} at position {})",
                            path.display(),
                            entry.id,
                            entries.len()
                        )));
                    }
                    entries.push(entry);
                    good_bytes += n as u64;
                }
                // A torn final line is the crash story: drop it. Anywhere else it
                // is corruption, and silently skipping it would hide history.
                _ => {
                    let mut rest = String::new();
                    let more = reader
                        .read_line(&mut rest)
                        .map_err(|e| Error::io(&path, e))?;
                    if more != 0 {
                        return Err(Error::Store(format!(
                            "{}: an unreadable entry at position {} is followed by more log",
                            path.display(),
                            entries.len()
                        )));
                    }
                    break;
                }
            }
        }
        if entries.is_empty() {
            return Err(Error::Store(format!("{}: empty log", path.display())));
        }
        // Trim the debris so the next append starts on a clean line.
        let on_disk = std::fs::metadata(&path)
            .map_err(|e| Error::io(&path, e))?
            .len();
        if on_disk != good_bytes {
            OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|f| f.set_len(good_bytes))
                .map_err(|e| Error::io(&path, e))?;
        }
        let log = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        let head = entries.len() as EntryId - 1;
        Ok(Store {
            dir,
            log,
            entries,
            head,
            log_bytes: good_bytes,
        })
    }

    fn push(&mut self, parent: Option<EntryId>, kind: Kind) -> Result<EntryId> {
        let id = self.entries.len() as EntryId;
        let entry = Entry {
            id,
            parent,
            ts: now_ms(),
            kind,
        };
        let mut line = serde_json::to_string(&entry).map_err(|e| Error::Store(e.to_string()))?;
        line.push('\n');
        let path = self.dir.join("session.jsonl");
        self.log
            .write_all(line.as_bytes())
            .and_then(|_| self.log.flush())
            .map_err(|e| Error::io(&path, e))?;
        self.log_bytes += line.len() as u64;
        self.entries.push(entry);
        self.head = id;
        Ok(id)
    }

    /// Append under the current head.
    pub fn append(&mut self, kind: Kind) -> Result<EntryId> {
        let parent = self.head;
        self.push(Some(parent), kind)
    }

    /// Move the tip. The next append forks from `id`.
    pub fn set_head(&mut self, id: EntryId) -> Result<()> {
        if self.get(id).is_none() {
            return Err(Error::Store(format!("no entry #{id}")));
        }
        self.head = id;
        Ok(())
    }

    pub fn head(&self) -> EntryId {
        self.head
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: EntryId) -> Option<&Entry> {
        self.entries.get(id as usize)
    }

    /// Every entry ever written, in log order — including the ones a reduction hides.
    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    /// The path from the root to `id`, oldest first.
    pub fn path_to(&self, id: EntryId) -> Vec<&Entry> {
        let mut out = Vec::new();
        let mut cur = self.get(id);
        while let Some(e) = cur {
            out.push(e);
            cur = e.parent.and_then(|p| self.get(p));
        }
        out.reverse();
        out
    }

    /// The current branch: root to head.
    pub fn branch(&self) -> Vec<&Entry> {
        self.path_to(self.head)
    }

    pub fn children(&self, id: EntryId) -> Vec<EntryId> {
        self.entries
            .iter()
            .filter(|e| e.parent == Some(id))
            .map(|e| e.id)
            .collect()
    }

    /// Leaves of the tree: entries nothing hangs from.
    pub fn leaves(&self) -> Vec<EntryId> {
        let parents: std::collections::HashSet<EntryId> =
            self.entries.iter().filter_map(|e| e.parent).collect();
        self.entries
            .iter()
            .map(|e| e.id)
            .filter(|id| !parents.contains(id))
            .collect()
    }

    // -- blobs ----------------------------------------------------------------

    /// Keep the whole of some bytes. Named after the entry they belong to.
    pub fn write_blob(&mut self, label: &str, bytes: &[u8]) -> Result<Blob> {
        let dir = self.dir.join("blobs");
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let mut n = 0;
        let file = loop {
            let f = format!("{}-{n}.txt", sanitize(label));
            if !dir.join(&f).exists() {
                break f;
            }
            n += 1;
        };
        let path = dir.join(&file);
        std::fs::write(&path, bytes).map_err(|e| Error::io(&path, e))?;
        Ok(Blob {
            file,
            bytes: bytes.len() as u64,
        })
    }

    /// Take a file a producer already spilled to disk into the session, by copy, then
    /// remove the original.
    pub fn adopt_blob(&mut self, label: &str, file: &Path) -> Result<Blob> {
        let dir = self.dir.join("blobs");
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let mut n = 0;
        let name = loop {
            let f = format!("{}-{n}.txt", sanitize(label));
            if !dir.join(&f).exists() {
                break f;
            }
            n += 1;
        };
        let to = dir.join(&name);
        let bytes = std::fs::copy(file, &to).map_err(|e| Error::io(&to, e))?;
        let _ = std::fs::remove_file(file);
        Ok(Blob { file: name, bytes })
    }

    pub fn read_blob(&self, blob: &Blob) -> Result<Vec<u8>> {
        let path = self.dir.join("blobs").join(&blob.file);
        std::fs::read(&path).map_err(|e| Error::io(&path, e))
    }

    pub fn blob_path(&self, blob: &Blob) -> PathBuf {
        self.dir.join("blobs").join(&blob.file)
    }

    // -- the derived index ------------------------------------------------------

    /// The index, from `index.json` if it is current, else rebuilt from the log
    /// (and written back). Its format can change without migrating anything.
    pub fn index(&self) -> Result<Index> {
        let path = self.dir.join("index.json");
        if let Some(idx) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Index>(&t).ok())
            && idx.log_bytes == self.log_bytes
            && idx.version == INDEX_VERSION
        {
            return Ok(idx);
        }
        let idx = Index::build(self);
        // Best effort: a read-only session directory still answers.
        let _ = write_json_atomic(&path, &idx);
        Ok(idx)
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// -- index ---------------------------------------------------------------------

const INDEX_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: EntryId,
    pub parent: Option<EntryId>,
    pub kind: String,
    /// Bytes of model-visible text in the entry.
    pub bytes: u64,
    /// The first line, cut short.
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    /// The log's size when this was built — the staleness check.
    pub log_bytes: u64,
    pub entries: Vec<IndexEntry>,
}

impl Index {
    pub fn build(store: &Store) -> Index {
        Index {
            version: INDEX_VERSION,
            log_bytes: store.log_bytes,
            entries: store
                .all()
                .iter()
                .map(|e| IndexEntry {
                    id: e.id,
                    parent: e.parent,
                    kind: e.kind.label().to_string(),
                    bytes: crate::history::text_of(&e.kind).len() as u64,
                    preview: crate::history::preview(&e.kind, 80),
                })
                .collect(),
        }
    }
}
