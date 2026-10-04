//! The session store: an append-only log in the case's database, a parent pointer per
//! entry ([ADR-0036](../../../docs/adr/0036-reactor-owns-its-session-store-format.md),
//! [ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
//!
//! A session is rows in `analysis.db`: one per entry, never updated and never deleted —
//! the database refuses both. The entries are also held here in memory, so reading a
//! branch costs nothing and only an append reaches the disk.
//!
//! **Append-only is the crash story.** An append is one transaction: a killed process
//! loses at most the entry it was writing, and what is on disk always reopens.
//!
//! **A branch is a parent pointer.** Entry ids are dense (`id == index in the log`),
//! `head` is the tip new entries hang from, and moving `head` back to an older entry
//! and appending forks the tree without touching a row already written.
//!
//! **One frontend per session.** Opening a session takes its lock; a second opener is
//! told so instead of interleaving ids.

use std::path::{Path, PathBuf};

use reactor_db::Case;
use reactor_db::case::SessionLock;
use reactor_db::sessions::{self, Row};

use crate::entry::{Blob, Entry, EntryId, Kind};
use crate::error::{Error, Result};

pub const FORMAT_VERSION: u32 = 2;

pub struct Store {
    case: Case,
    session: String,
    dir: PathBuf,
    entries: Vec<Entry>,
    head: EntryId,
    _lock: SessionLock,
}

fn now_ms() -> u64 {
    reactor_db::time::now_ms()
}

fn row_of(entry: &Entry) -> Result<Row> {
    Ok(Row {
        id: entry.id,
        parent: entry.parent,
        ts: entry.ts,
        kind: entry.kind.label().to_string(),
        body: serde_json::to_string(entry).map_err(|e| Error::Store(e.to_string()))?,
        blob_sha256: match &entry.kind {
            Kind::ToolResult { blob: Some(b), .. } => Some(b.sha256.clone()),
            _ => None,
        },
    })
}

impl Store {
    /// A new session in `case` (which must not already have one of that name).
    pub fn create(case: &Case, session: &str) -> Result<Store> {
        let lock = case.lock_session(session)?;
        let first = Entry {
            id: 0,
            parent: None,
            ts: now_ms(),
            kind: Kind::Session {
                version: FORMAT_VERSION,
                session: session.to_string(),
            },
        };
        sessions::create(case.db(), session, FORMAT_VERSION, row_of(&first)?)?;
        Ok(Store {
            case: case.clone(),
            session: session.to_string(),
            dir: case.session_dir(session)?,
            entries: vec![first],
            head: 0,
            _lock: lock,
        })
    }

    /// Open an existing session.
    pub fn open(case: &Case, session: &str) -> Result<Store> {
        let lock = case.lock_session(session)?;
        let rows = sessions::load(case.db(), session)?;
        if rows.is_empty() {
            return Err(Error::Store(format!("no session `{session}` in this case")));
        }
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            let entry: Entry = serde_json::from_str(&row.body).map_err(|e| {
                Error::Store(format!(
                    "session `{session}`: entry #{} is unreadable: {e}",
                    row.id
                ))
            })?;
            if entry.id as usize != entries.len() {
                return Err(Error::Store(format!(
                    "session `{session}`: entry ids are not dense (found {} at position {})",
                    entry.id,
                    entries.len()
                )));
            }
            entries.push(entry);
        }
        let head = entries.len() as EntryId - 1;
        Ok(Store {
            case: case.clone(),
            session: session.to_string(),
            dir: case.session_dir(session)?,
            entries,
            head,
            _lock: lock,
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
        sessions::append(self.case.db(), &self.session, row_of(&entry)?)?;
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

    /// The case this session belongs to.
    pub fn case(&self) -> &Case {
        &self.case
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// The session's own directory in the case, for per-session files
    /// (`activation.json`).
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

    /// Keep the whole of some bytes, in the case's blob store.
    pub fn write_blob(&mut self, bytes: &[u8]) -> Result<Blob> {
        let (sha256, bytes) = self.case.blobs().put(bytes)?;
        Ok(Blob { sha256, bytes })
    }

    /// Take a file a producer already spilled to disk into the case, then remove the
    /// original.
    pub fn adopt_blob(&mut self, file: &Path) -> Result<Blob> {
        let (sha256, bytes) = self.case.blobs().adopt(file)?;
        Ok(Blob { sha256, bytes })
    }

    pub fn read_blob(&self, blob: &Blob) -> Result<Vec<u8>> {
        Ok(self.case.blobs().read(&blob.sha256)?)
    }

    pub fn blob_path(&self, blob: &Blob) -> PathBuf {
        self.case.blobs().path(&blob.sha256)
    }
}
