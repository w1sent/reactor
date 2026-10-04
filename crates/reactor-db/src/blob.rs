//! Content with no file of its own, kept by its SHA-256
//! ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)): the whole of
//! a tool output the model saw only part of, a fetched page. Anything that *is* a file
//! in the workdir is referenced in place and never comes here.
//!
//! ```text
//! blobs/sha256/ab/cd/<sha256>
//! ```
//!
//! **A `blob` row exists only if its file does.** The file is written first — to a
//! temporary name in the same directory, synced, then renamed — and the row after. A
//! crash between the two leaves a file without a row, which [`BlobStore::fsck`] adopts.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::time::now_rfc3339;

#[derive(Clone)]
pub struct BlobStore {
    root: PathBuf,
    db: Db,
}

/// What [`BlobStore::fsck`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fsck {
    /// Rows whose file is gone. Nothing can bring the bytes back.
    pub missing: Vec<String>,
    /// Files that had no row and were given one.
    pub adopted: Vec<String>,
    /// Files under `blobs/` whose content does not match their name. Left alone.
    pub corrupt: Vec<PathBuf>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
    let (mut hasher, mut size, mut buf) = (Sha256::new(), 0u64, vec![0u8; 64 * 1024]);
    loop {
        let n = file.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex(&hasher.finalize()), size))
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl BlobStore {
    pub fn new(root: PathBuf, db: Db) -> BlobStore {
        BlobStore { root, db }
    }

    /// Where a blob's bytes are (whether or not it exists).
    pub fn path(&self, sha256: &str) -> PathBuf {
        let (a, b) = (
            sha256.get(0..2).unwrap_or("__"),
            sha256.get(2..4).unwrap_or("__"),
        );
        self.root.join("sha256").join(a).join(b).join(sha256)
    }

    fn record(&self, sha256: &str, size: u64) -> Result<()> {
        let sha = sha256.to_string();
        self.db.write(move |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO blob(sha256, size, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![sha, size as i64, now_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// Keep `bytes`. The same bytes twice are one blob.
    pub fn put(&self, bytes: &[u8]) -> Result<(String, u64)> {
        let sha = hex(&Sha256::digest(bytes));
        let dest = self.path(&sha);
        if !dest.exists() {
            let dir = dest.parent().expect("a blob path has a parent");
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
            let tmp = dir.join(format!(".{sha}.{}.tmp", std::process::id()));
            let mut f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            f.write_all(bytes)
                .and_then(|_| f.sync_all())
                .map_err(|e| Error::io(&tmp, e))?;
            drop(f);
            place(&tmp, &dest)?;
        }
        self.record(&sha, bytes.len() as u64)?;
        Ok((sha, bytes.len() as u64))
    }

    /// Take a file a producer spilled to disk, then remove the original.
    pub fn adopt(&self, file: &Path) -> Result<(String, u64)> {
        let (sha, size) = hash_file(file)?;
        let dest = self.path(&sha);
        if !dest.exists() {
            let dir = dest.parent().expect("a blob path has a parent");
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
            let tmp = dir.join(format!(".{sha}.{}.tmp", std::process::id()));
            // A copy, not a rename: the spill file is usually on another filesystem.
            std::fs::copy(file, &tmp).map_err(|e| Error::io(&tmp, e))?;
            File::open(&tmp)
                .and_then(|f| f.sync_all())
                .map_err(|e| Error::io(&tmp, e))?;
            place(&tmp, &dest)?;
        }
        let _ = std::fs::remove_file(file);
        self.record(&sha, size)?;
        Ok((sha, size))
    }

    pub fn read(&self, sha256: &str) -> Result<Vec<u8>> {
        let path = self.path(sha256);
        std::fs::read(&path).map_err(|e| Error::io(&path, e))
    }

    /// Reconcile rows and files.
    pub fn fsck(&self) -> Result<Fsck> {
        let mut report = Fsck::default();
        let rows: Vec<String> = self.db.read(|conn| {
            let mut stmt = conn.prepare("SELECT sha256 FROM blob ORDER BY sha256")?;
            let rows = stmt
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            Ok(rows)
        })?;
        for sha in &rows {
            if !self.path(sha).is_file() {
                report.missing.push(sha.clone());
            }
        }
        let known: std::collections::HashSet<&String> = rows.iter().collect();
        let mut files = Vec::new();
        walk(&self.root.join("sha256"), &mut files);
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !is_sha256(&name) {
                // Debris of an interrupted write.
                if name.ends_with(".tmp") {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            if known.contains(&name) {
                continue;
            }
            let (sha, size) = hash_file(&path)?;
            if sha == name {
                self.record(&sha, size)?;
                report.adopted.push(sha);
            } else {
                report.corrupt.push(path);
            }
        }
        Ok(report)
    }
}

/// Move a finished temporary file to its name. Someone else getting there first is
/// success: the name is the content.
fn place(tmp: &Path, dest: &Path) -> Result<()> {
    match std::fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if dest.exists() => {
            let _ = std::fs::remove_file(tmp);
            Ok(())
        }
        Err(e) => Err(Error::io(dest, e)),
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}
