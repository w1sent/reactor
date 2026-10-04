//! The registry of referenced files
//! ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)).
//!
//! A file in the workdir — a sample, a dump, a capture, something the agent unpacked —
//! is **referenced in place**: the registry holds its path relative to the case root
//! and its hashes, and never a copy. The file can change behind the registry's back;
//! [`verify`] says so, and nothing pretends it did not.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use md5::Md5;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::case::Case;
use crate::error::{Error, Result};
use crate::manifest::{Author, author_id};
use crate::time::now_rfc3339;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub id: String,
    /// Relative to the case root, with forward slashes.
    pub path: String,
    pub sha256: String,
    pub sha1: String,
    pub md5: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The registered file this one came out of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub registered_at: String,
}

/// What a registered file looks like on disk now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Unchanged,
    /// The content is not what was registered.
    Changed,
    Missing,
}

struct Hashes {
    sha256: String,
    sha1: String,
    md5: String,
    size: u64,
    mime: Option<String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(path: &Path) -> Result<Hashes> {
    let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
    let (mut h256, mut h1, mut h5) = (Sha256::new(), Sha1::new(), Md5::new());
    let (mut size, mut buf, mut head) = (0u64, vec![0u8; 64 * 1024], Vec::new());
    loop {
        let n = file.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        if head.len() < 8192 {
            head.extend_from_slice(&buf[..n.min(8192 - head.len())]);
        }
        h256.update(&buf[..n]);
        h1.update(&buf[..n]);
        h5.update(&buf[..n]);
        size += n as u64;
    }
    Ok(Hashes {
        sha256: hex(&h256.finalize()),
        sha1: hex(&h1.finalize()),
        md5: hex(&h5.finalize()),
        size,
        mime: infer::get(&head).map(|t| t.mime_type().to_string()),
    })
}

/// A path as the registry stores it — relative to the case root, forward slashes — and
/// where it is on disk. A path that leaves the case, through `..` or a symlink, is
/// refused: a case never refers to local files outside its root.
pub fn locate(case: &Case, path: &str) -> Result<(String, PathBuf)> {
    let given = Path::new(path.trim());
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        case.root().join(given)
    };
    let real = std::fs::canonicalize(&joined).map_err(|e| Error::io(&joined, e))?;
    let rel = real.strip_prefix(case.root()).map_err(|_| {
        Error::Invalid(format!(
            "{}: outside the case -- a case refers only to files inside {}",
            path.trim(),
            case.root().display()
        ))
    })?;
    if rel
        .components()
        .next()
        .is_some_and(|c| c == Component::Normal(crate::case::DIR.as_ref()))
    {
        return Err(Error::Invalid(format!(
            "{}: REactor's own files are not evidence",
            path.trim()
        )));
    }
    let stored = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if stored.is_empty() || !real.is_file() {
        return Err(Error::Invalid(format!("{}: not a file", path.trim())));
    }
    Ok((stored, real))
}

/// The SHA-256 of a file in the case, as it is now.
pub fn sha256_of(path: &Path) -> Result<String> {
    Ok(hash(path)?.sha256)
}

/// Add a file to the registry, or bring its entry up to date with what is on disk.
pub fn register(
    case: &Case,
    path: &str,
    kind: Option<&str>,
    parent: Option<&str>,
    author: &Author,
    session: Option<&str>,
) -> Result<Artifact> {
    let (stored, real) = locate(case, path)?;
    let h = hash(&real)?;
    let parent_path = match parent {
        Some(p) => Some(locate(case, p)?.0),
        None => None,
    };
    let (kind, author, session) = (
        kind.map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty()),
        author.clone(),
        session.map(str::to_string),
    );
    let path_for_read = stored.clone();
    case.db().write(move |tx| {
        let parent_id: Option<String> = match &parent_path {
            Some(p) => Some(
                tx.query_row("SELECT id FROM artifact WHERE path = ?1", [p], |r| r.get(0))
                    .optional()?
                    .ok_or_else(|| {
                        Error::Invalid(format!("{p}: the parent is not registered yet"))
                    })?,
            ),
            None => None,
        };
        let existing: Option<(String, String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT id, sha256, kind, parent_id FROM artifact WHERE path = ?1",
                [&stored],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        // Registered already, and nothing about it differs: nothing to write.
        if let Some((_, sha, k, p)) = &existing
            && *sha == h.sha256
            && (kind.is_none() || *k == kind)
            && (parent_id.is_none() || *p == parent_id)
        {
            return Ok(());
        }
        let author_id = author_id(tx, &author)?;
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO event(id, ts, session_id, author_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, 'artifact', json_object('path', ?5, 'sha256', ?6))",
            params![
                uuid::Uuid::now_v7().to_string(),
                now,
                session,
                author_id,
                stored,
                h.sha256
            ],
        )?;
        let event = tx.last_insert_rowid();
        match existing {
            Some((id, _, k, p)) => tx.execute(
                "UPDATE artifact SET sha256 = ?2, sha1 = ?3, md5 = ?4, size = ?5, mime = ?6,
                        kind = ?7, parent_id = ?8, registered_at = ?9, event_seq = ?10
                  WHERE id = ?1",
                params![
                    id,
                    h.sha256,
                    h.sha1,
                    h.md5,
                    h.size as i64,
                    h.mime,
                    kind.or(k),
                    parent_id.or(p),
                    now,
                    event
                ],
            )?,
            None => tx.execute(
                "INSERT INTO artifact(id, path, sha256, sha1, md5, size, mime, kind, parent_id,
                        registered_at, event_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    uuid::Uuid::now_v7().to_string(),
                    stored,
                    h.sha256,
                    h.sha1,
                    h.md5,
                    h.size as i64,
                    h.mime,
                    kind,
                    parent_id,
                    now,
                    event
                ],
            )?,
        };
        Ok(())
    })?;
    get(case, &path_for_read)?.ok_or_else(|| Error::Sqlite("the registry lost a row".into()))
}

const SELECT: &str = "SELECT a.id, a.path, a.sha256, a.sha1, a.md5, a.size, a.mime, a.kind,
        (SELECT p.path FROM artifact p WHERE p.id = a.parent_id), a.registered_at
   FROM artifact a";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: r.get(0)?,
        path: r.get(1)?,
        sha256: r.get(2)?,
        sha1: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        md5: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        size: r.get::<_, i64>(5)? as u64,
        mime: r.get(6)?,
        kind: r.get(7)?,
        parent: r.get(8)?,
        registered_at: r.get(9)?,
    })
}

/// The entry for a stored path.
pub fn get(case: &Case, stored_path: &str) -> Result<Option<Artifact>> {
    case.db().read(|conn| {
        Ok(conn
            .query_row(&format!("{SELECT} WHERE a.path = ?1"), [stored_path], row)
            .optional()?)
    })
}

/// Every registered file, by path.
pub fn list(case: &Case) -> Result<Vec<Artifact>> {
    case.db().read(|conn| {
        let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY a.path"))?;
        let rows = stmt
            .query_map([], row)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Compare every registered file with what is on disk now.
pub fn verify(case: &Case) -> Result<Vec<(Artifact, State)>> {
    list(case)?
        .into_iter()
        .map(|a| {
            let path = case.root().join(&a.path);
            let state = if !path.is_file() {
                State::Missing
            } else if hash(&path)?.sha256 == a.sha256 {
                State::Unchanged
            } else {
                State::Changed
            };
            Ok((a, state))
        })
        .collect()
}
