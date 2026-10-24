//! A case: one investigation, held in one directory
//! ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)).
//!
//! ```text
//! <case>/                         the workdir; evidence lives here
//!   .reactor/
//!     case.json                   id, title, created, format version
//!     analysis.db
//!     blobs/sha256/ab/cd/<sha256>
//!     sessions/<id>/              per-session files (activation.json)
//!     run/                        lock files; safe to delete
//! ```
//!
//! **A case is created by saving.** Opening a folder that is not a case gives an
//! *unsaved state*: the same layout under `<config dir>/unsaved/<id>/`, with the folder
//! it belongs to written *beside* it (`<id>.workdir`), never inside. [`Case::save`]
//! moves it into the folder.
//!
//! **Nothing inside the case directory stores an absolute path**, because it is moved as
//! a unit — by saving, and by anyone copying a case.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::blob::BlobStore;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::time::now_rfc3339;

/// The directory inside a workdir that makes it a case.
pub const DIR: &str = ".reactor";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// RFC 3339 UTC.
    pub created: String,
    pub format: u32,
    /// The name the person gave the case, which a reference to it carries instead of the id
    /// ([ADR-0066](../../../docs/adr/0066-the-case-is-addressed-by-references.md)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Aliases the case had before: references written with them still name it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_aliases: Vec<String>,
}

/// Whether `alias` is a name a case may have: letters, digits, `-` and `_`, no spaces.
pub fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

struct Inner {
    root: PathBuf,
    dir: PathBuf,
    /// The unsaved state's `<id>.workdir` marker; `None` for a saved case.
    marker: Option<PathBuf>,
    meta: RwLock<Meta>,
    db: Db,
    blobs: BlobStore,
}

/// An open case, or an unsaved state that can become one. Cheap to clone.
#[derive(Clone)]
pub struct Case {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Case")
            .field("id", &self.meta().id)
            .field("root", &self.inner.root)
            .field("saved", &self.is_saved())
            .finish()
    }
}

/// Held while a frontend has a session open: one frontend per session.
pub struct SessionLock {
    _file: File,
}

/// Held while vectors are being made for the case: one embedding run per case.
pub struct EmbedLock {
    _file: File,
}

fn read_meta(dir: &Path) -> Result<Meta> {
    let path = dir.join("case.json");
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let meta: Meta =
        serde_json::from_str(&text).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    if meta.format > FORMAT_VERSION {
        return Err(Error::Build(format!(
            "{}: case format {} is newer than this build of REactor understands ({FORMAT_VERSION})",
            path.display(),
            meta.format
        )));
    }
    Ok(meta)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::write(&tmp, text + "\n").map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
}

fn canonical(root: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(root).map_err(|e| Error::io(root, e))
}

impl Case {
    /// Whether `root` is a case: its `.reactor/` holds a `case.json`. (The directory
    /// alone does not count — it may only hold project-scoped tool state.)
    pub fn is_case(root: &Path) -> bool {
        root.join(DIR).join("case.json").is_file()
    }

    fn open_dir(root: PathBuf, dir: PathBuf, marker: Option<PathBuf>) -> Result<Case> {
        let meta = read_meta(&dir)?;
        for sub in ["blobs", "sessions", "run"] {
            let p = dir.join(sub);
            std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
        }
        let db = Db::open(&dir.join("analysis.db"))?;
        let blobs = BlobStore::new(dir.join("blobs"), db.clone());
        Ok(Case {
            inner: Arc::new(Inner {
                root,
                dir,
                marker,
                meta: RwLock::new(meta),
                db,
                blobs,
            }),
        })
    }

    fn init_dir(dir: &Path, title: Option<&str>) -> Result<()> {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        write_json(
            &dir.join("case.json"),
            &Meta {
                id: uuid::Uuid::now_v7().to_string(),
                title: title.map(str::to_string).filter(|t| !t.is_empty()),
                created: now_rfc3339(),
                format: FORMAT_VERSION,
                alias: None,
                former_aliases: Vec::new(),
            },
        )
    }

    /// Make `root` a case, straight away (the headless path; the GUI saves instead).
    pub fn create(root: &Path, title: Option<&str>) -> Result<Case> {
        let root = canonical(root)?;
        if Case::is_case(&root) {
            return Err(Error::Io(format!("{}: already a case", root.display())));
        }
        let dir = root.join(DIR);
        Case::init_dir(&dir, title)?;
        Case::open_dir(root, dir, None)
    }

    /// Open the case in `root`.
    pub fn open(root: &Path) -> Result<Case> {
        let root = canonical(root)?;
        if !Case::is_case(&root) {
            return Err(Error::Io(format!("{}: not a case", root.display())));
        }
        let dir = root.join(DIR);
        Case::open_dir(root, dir, None)
    }

    /// The unsaved state for `root`: the one it already has, else a new one.
    pub fn unsaved(root: &Path, config_dir: &Path) -> Result<Case> {
        let root = canonical(root)?;
        let base = config_dir.join("unsaved");
        std::fs::create_dir_all(&base).map_err(|e| Error::io(&base, e))?;
        if let Some((dir, marker)) = find_unsaved(&base, &root) {
            return Case::open_dir(root, dir, Some(marker));
        }
        let id = uuid::Uuid::now_v7().to_string();
        let (dir, marker) = (base.join(&id), base.join(format!("{id}.workdir")));
        Case::init_dir(&dir, None)?;
        std::fs::write(&marker, root.to_string_lossy().as_bytes())
            .map_err(|e| Error::io(&marker, e))?;
        Case::open_dir(root, dir, Some(marker))
    }

    /// What opening REactor on `root` means: its case, else its unsaved state.
    pub fn open_any(root: &Path, config_dir: &Path) -> Result<Case> {
        if Case::is_case(root) {
            Case::open(root)
        } else {
            Case::unsaved(root, config_dir)
        }
    }

    /// Whether `root` has an unsaved state waiting.
    pub fn has_unsaved(root: &Path, config_dir: &Path) -> bool {
        canonical(root)
            .ok()
            .and_then(|root| find_unsaved(&config_dir.join("unsaved"), &root))
            .is_some()
    }

    /// The workdir.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Where REactor's own files for this case are.
    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    pub fn is_saved(&self) -> bool {
        self.inner.marker.is_none()
    }

    /// The case's identity file as it stands now.
    pub fn meta(&self) -> Meta {
        self.inner.meta.read().unwrap().clone()
    }

    /// The name the person gave the case, if they did.
    pub fn alias(&self) -> Option<String> {
        self.inner.meta.read().unwrap().alias.clone()
    }

    /// What a reference to this case calls it: the alias, else the id.
    pub fn authority(&self) -> String {
        let m = self.inner.meta.read().unwrap();
        m.alias.clone().unwrap_or_else(|| m.id.clone())
    }

    /// Whether a reference's case part names this case: its id, its alias, or an alias it
    /// had before. Names are compared without regard to case.
    pub fn is_named(&self, authority: &str) -> bool {
        let m = self.inner.meta.read().unwrap();
        m.id.eq_ignore_ascii_case(authority)
            || m.alias
                .iter()
                .chain(m.former_aliases.iter())
                .any(|a| a.eq_ignore_ascii_case(authority))
    }

    /// Give the case an alias, or take it away (`None`). The alias it had becomes a former
    /// one, so references written with it keep working.
    pub fn set_alias(&self, alias: Option<&str>) -> Result<()> {
        let alias = alias.map(str::trim).filter(|a| !a.is_empty());
        if let Some(a) = alias
            && !valid_alias(a)
        {
            return Err(Error::Invalid(format!(
                "`{a}`: an alias is letters, digits, `-` and `_`, up to 64, no spaces"
            )));
        }
        let mut m = self.inner.meta.write().unwrap();
        let mut next = m.clone();
        if let Some(old) = next.alias.take()
            && !old.eq_ignore_ascii_case(alias.unwrap_or(""))
            && !next.former_aliases.iter().any(|f| f.eq_ignore_ascii_case(&old))
        {
            next.former_aliases.push(old);
        }
        next.former_aliases
            .retain(|f| !alias.is_some_and(|a| f.eq_ignore_ascii_case(a)));
        next.alias = alias.map(str::to_string);
        write_json(&self.inner.dir.join("case.json"), &next)?;
        *m = next;
        Ok(())
    }

    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.inner.blobs
    }

    /// A session's own directory (created on demand): `activation.json` lives here.
    pub fn session_dir(&self, session: &str) -> Result<PathBuf> {
        let dir = self.inner.dir.join("sessions").join(session);
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        Ok(dir)
    }

    /// Claim a session for this process. Fails if another frontend has it.
    pub fn lock_session(&self, session: &str) -> Result<SessionLock> {
        let path = self
            .inner
            .dir
            .join("run")
            .join(format!("session-{session}.lock"));
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(SessionLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::Io(format!(
                "session `{session}` is open in another window"
            ))),
            Err(std::fs::TryLockError::Error(e)) => Err(Error::io(&path, e)),
        }
    }

    /// Claim the case's embedding run. `None` if another is already going, in this process
    /// or another: two would embed the same passages twice.
    pub fn lock_embedding(&self) -> Result<Option<EmbedLock>> {
        let path = self.inner.dir.join("run").join("embed.lock");
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(EmbedLock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(Error::io(&path, e)),
        }
    }

    /// Whether a frontend — in this process or another — has one of the sessions open.
    pub fn in_use(&self) -> bool {
        crate::sessions::list(self.db())
            .map(|sessions| sessions.iter().any(|s| self.lock_session(&s.id).is_err()))
            .unwrap_or(true)
    }

    /// Turn an unsaved state into the case of its folder: move it to `<root>/.reactor/`.
    /// Every other handle to it must be gone, because the database is closed for the
    /// move.
    pub fn save(self) -> Result<Case> {
        if self.in_use() {
            return Err(Error::Io(
                "a session of this case is open in a window; close it first".into(),
            ));
        }
        let inner = Arc::try_unwrap(self.inner).map_err(|_| {
            Error::Io("the case is still in use and cannot be saved right now".into())
        })?;
        let Inner {
            root,
            dir,
            marker,
            meta,
            db,
            blobs,
        } = inner;
        let Some(marker) = marker else {
            return Err(Error::Io(format!("{}: already a case", root.display())));
        };
        let target = root.join(DIR);
        if target.join("case.json").exists() {
            return Err(Error::Io(format!("{}: already a case", root.display())));
        }
        // Close the database: the writer is joined and the WAL folded in before a byte
        // moves.
        drop(blobs);
        drop(db);
        drop(meta);
        std::fs::create_dir_all(&target).map_err(|e| Error::io(&target, e))?;
        let entries = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            // Lock files belong to the place they were taken in.
            if name == "run" {
                continue;
            }
            move_path(&entry.path(), &target.join(&name))?;
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&marker);
        Case::open_dir(root, target, None)
    }

    /// Throw an unsaved state away. Files written into the workdir are not touched.
    pub fn discard(self) -> Result<()> {
        if self.in_use() {
            return Err(Error::Io(
                "a session of this case is open in a window; close it first".into(),
            ));
        }
        let inner = Arc::try_unwrap(self.inner).map_err(|_| {
            Error::Io("the case is still in use and cannot be discarded right now".into())
        })?;
        let Some(marker) = inner.marker.clone() else {
            return Err(Error::Io(format!(
                "{}: a saved case is not discarded, it is deleted by hand",
                inner.root.display()
            )));
        };
        let dir = inner.dir.clone();
        drop(inner);
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let _ = std::fs::remove_file(&marker);
        Ok(())
    }
}

/// The unsaved state that belongs to `root`, if there is one.
fn find_unsaved(base: &Path, root: &Path) -> Option<(PathBuf, PathBuf)> {
    let want = root.to_string_lossy();
    for entry in std::fs::read_dir(base).ok()?.flatten() {
        let marker = entry.path();
        if marker.extension().and_then(|e| e.to_str()) != Some("workdir") {
            continue;
        }
        let dir = marker.with_extension("");
        if std::fs::read_to_string(&marker).ok().as_deref() == Some(want.as_ref())
            && dir.join("case.json").is_file()
        {
            return Some((dir, marker));
        }
    }
    None
}

/// Rename, or copy and delete when the two are on different filesystems.
fn move_path(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_tree(from, to)?;
    if from.is_dir() {
        std::fs::remove_dir_all(from).map_err(|e| Error::io(from, e))
    } else {
        std::fs::remove_file(from).map_err(|e| Error::io(from, e))
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
        for entry in std::fs::read_dir(from)
            .map_err(|e| Error::io(from, e))?
            .flatten()
        {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| Error::io(to, e))
    }
}

// -- the cases this machine knows -----------------------------------------------------

/// A case REactor has opened on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub root: PathBuf,
    /// `None` when the folder is gone or no longer a case.
    pub meta: Option<Meta>,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    cases: Vec<PathBuf>,
}

fn index_path(config_dir: &Path) -> PathBuf {
    config_dir.join("cases.json")
}

fn read_index(config_dir: &Path) -> Index {
    std::fs::read_to_string(index_path(config_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Note that `root` is a case, most recent first. Cases live wherever their folders are,
/// so this list in the config directory is the only way to enumerate them.
pub fn remember(config_dir: &Path, root: &Path) -> Result<()> {
    let root = canonical(root)?;
    let mut index = read_index(config_dir);
    index.cases.retain(|p| *p != root);
    index.cases.insert(0, root);
    std::fs::create_dir_all(config_dir).map_err(|e| Error::io(config_dir, e))?;
    write_json(&index_path(config_dir), &index)
}

/// The cases this machine knows, most recent first.
pub fn known(config_dir: &Path) -> Vec<Known> {
    read_index(config_dir)
        .cases
        .into_iter()
        .map(|root| {
            let meta = Case::is_case(&root)
                .then(|| read_meta(&root.join(DIR)).ok())
                .flatten();
            Known { root, meta }
        })
        .collect()
}
