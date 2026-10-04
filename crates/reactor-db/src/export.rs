//! Handing a case over: one archive, compressed and encrypted
//! ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)).
//!
//! A case may hold live malware, so an export must not be runnable by accident, and no
//! scanner should quarantine it on sight. The archive is a ZIP with AES-256 encryption,
//! which 7-Zip and most archivers open. Its password is [`DEFAULT_PASSWORD`] — the
//! convention for sharing samples — unless the person gives one: the default is a
//! safeguard against accidents, not a secret, and only a password of their own is one.
//!
//! What goes in is the whole case folder: the evidence, the agent's data file, and
//! `.reactor/` — with the databases as *consistent snapshots* (`VACUUM INTO`), never a raw
//! copy of a file another process may be writing. The write-ahead logs and lock files are
//! left out, and so is any link, which would lead out of the case. Files are archived
//! without their executable bit, and come back without it.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

use crate::browse;
use crate::case::{Case, DIR};
use crate::error::{Error, Result};

/// What an export is encrypted with when the person gives no password.
pub const DEFAULT_PASSWORD: &str = "infected";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exported {
    pub files: usize,
    pub bytes: u64,
    /// What was left out and why: a link, a file that could not be read.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub files: usize,
    pub bytes: u64,
    /// Blobs the database names that are not in the archive.
    pub missing_blobs: Vec<String>,
}

/// Files that are never archived as they are on disk.
fn is_derived(rel: &str) -> bool {
    rel.starts_with(&format!("{DIR}/run/"))
        || ["-wal", "-shm", "-journal"]
            .iter()
            .any(|suffix| rel.ends_with(suffix))
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>, skipped: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        skipped.push(format!("{}: could not be read", dir.display()));
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let rel: Vec<String> = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let rel = rel.join("/");
        match e.file_type() {
            Ok(t) if t.is_symlink() => {
                skipped.push(format!("{rel}: a link, which could lead out of the case"))
            }
            Ok(t) if t.is_dir() => walk(&path, root, out, skipped),
            Ok(t) if t.is_file() => {
                if !is_derived(&rel) {
                    out.push((rel, path));
                }
            }
            _ => skipped.push(format!("{rel}: not a regular file")),
        }
    }
}

/// A consistent copy of a SQLite file, in `scratch`. `VACUUM INTO` works on a database
/// that is open read-only, and sees one moment of it whatever else is writing.
fn snapshot(source: &Path, scratch: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute("VACUUM INTO ?1", [scratch.to_string_lossy().as_ref()])?;
    Ok(())
}

fn is_sqlite_file(path: &Path) -> bool {
    let mut head = [0u8; 16];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && browse::is_sqlite(&head)
}

/// Write `case` to `dest`. Refuses to overwrite a file, and an unsaved state (which is not
/// yet a case in a folder).
pub fn export(case: &Case, dest: &Path, password: &str) -> Result<Exported> {
    if !case.is_saved() {
        return Err(Error::Invalid(
            "this is an unsaved state: save it as a case first".into(),
        ));
    }
    if password.is_empty() {
        return Err(Error::Invalid("the password cannot be empty".into()));
    }
    if dest.exists() {
        return Err(Error::Io(format!("{}: already exists", dest.display())));
    }
    let root = case.root();
    let dest_abs = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| Error::io(dest, e))?
        .join(dest.file_name().unwrap_or_default());

    let mut report = Exported::default();
    let mut files = Vec::new();
    walk(root, root, &mut files, &mut report.skipped);
    // The archive being written is not part of what it holds.
    files.retain(|(_, p)| p.canonicalize().ok().as_deref() != Some(dest_abs.as_path()));

    let scratch = case
        .dir()
        .join("run")
        .join(format!("export-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| Error::io(&scratch, e))?;
    let result = write_archive(case, dest, password, &files, &scratch, &mut report);
    let _ = std::fs::remove_dir_all(&scratch);
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result.map(|()| report)
}

fn write_archive(
    case: &Case,
    dest: &Path,
    password: &str,
    files: &[(String, PathBuf)],
    scratch: &Path,
    report: &mut Exported,
) -> Result<()> {
    let out = File::create(dest).map_err(|e| Error::io(dest, e))?;
    let mut zip = ZipWriter::new(BufWriter::new(out));
    let mut add = |name: &str, path: &Path, report: &mut Exported| -> Result<()> {
        let size = std::fs::metadata(path)
            .map_err(|e| Error::io(path, e))?
            .len();
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            // Readable, writable, not executable: a sample must not run from an archive.
            .unix_permissions(0o644)
            .large_file(size >= u32::MAX as u64)
            .with_aes_encryption(AesMode::Aes256, password);
        zip.start_file(name, options)
            .map_err(|e| Error::Io(format!("{name}: {e}")))?;
        let mut input = BufReader::new(File::open(path).map_err(|e| Error::io(path, e))?);
        let copied =
            std::io::copy(&mut input, &mut zip).map_err(|e| Error::Io(format!("{name}: {e}")))?;
        report.files += 1;
        report.bytes += copied;
        Ok(())
    };

    // The case's own database, from a snapshot made through its writer.
    let db_name = format!("{DIR}/analysis.db");
    let db_copy = scratch.join("analysis.db");
    case.db().backup_to(&db_copy)?;
    add(&db_name, &db_copy, report)?;

    for (rel, path) in files {
        if *rel == db_name {
            continue;
        }
        // Any other SQLite file is archived as a consistent snapshot too — the agent's
        // data file is written by scripts while the case is open.
        if is_sqlite_file(path) {
            let copy = scratch.join(format!("snap-{}", report.files));
            if snapshot(path, &copy).is_ok() {
                add(rel, &copy, report)?;
                let _ = std::fs::remove_file(&copy);
                continue;
            }
        }
        if let Err(e) = add(rel, path, report) {
            // A file that cannot be read is said so, and the rest goes on.
            report.skipped.push(format!("{rel}: {e}"));
        }
    }
    zip.finish()
        .map_err(|e| Error::Io(format!("{}: {e}", dest.display())))?
        .flush()
        .map_err(|e| Error::io(dest, e))?;
    Ok(())
}

/// A name inside an archive as a path under `dest`, or why it is not safe: nothing may be
/// written outside the folder the archive is unpacked into.
fn inside(dest: &Path, name: &str) -> Result<PathBuf> {
    let rel = Path::new(name);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::Invalid(format!(
            "the archive holds `{name}`, which would be written outside the case"
        )));
    }
    Ok(dest.join(rel))
}

/// Unpack an archive made by [`export`] into `dest`, which must be empty or not exist yet.
/// The folder is then a case. Nothing is left behind if it fails.
pub fn import(archive: &Path, dest: &Path, password: &str) -> Result<(Case, Imported)> {
    if dest.exists()
        && std::fs::read_dir(dest)
            .map_err(|e| Error::io(dest, e))?
            .next()
            .is_some()
    {
        return Err(Error::Io(format!(
            "{}: not empty -- a case is unpacked into an empty folder",
            dest.display()
        )));
    }
    let created = !dest.exists();
    std::fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    let outcome = unpack(archive, dest, password);
    if outcome.is_err() {
        // Remove what was unpacked, and the folder if this call made it.
        if created {
            let _ = std::fs::remove_dir_all(dest);
        } else if let Ok(entries) = std::fs::read_dir(dest) {
            for e in entries.flatten() {
                let p = e.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
    }
    outcome
}

fn unpack(archive: &Path, dest: &Path, password: &str) -> Result<(Case, Imported)> {
    let file = File::open(archive).map_err(|e| Error::io(archive, e))?;
    let mut zip = ZipArchive::new(BufReader::new(file))
        .map_err(|e| Error::Io(format!("{}: not an archive ({e})", archive.display())))?;
    let marker = format!("{DIR}/case.json");
    if zip.index_for_name(&marker).is_none() {
        return Err(Error::Invalid(format!(
            "{}: not an exported case (it has no {marker})",
            archive.display()
        )));
    }
    let mut report = Imported {
        files: 0,
        bytes: 0,
        missing_blobs: Vec::new(),
    };
    for i in 0..zip.len() {
        let (name, encrypted, is_dir) = {
            let raw = zip.by_index_raw(i).map_err(|e| Error::Io(e.to_string()))?;
            (raw.name().to_string(), raw.encrypted(), raw.is_dir())
        };
        let target = inside(dest, &name)?;
        if is_dir {
            std::fs::create_dir_all(&target).map_err(|e| Error::io(&target, e))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let mut entry = if encrypted {
            zip.by_index_decrypt(i, password.as_bytes())
        } else {
            zip.by_index(i)
        }
        .map_err(|e| match e {
            zip::result::ZipError::InvalidPassword => {
                Error::Invalid("wrong password for this archive".into())
            }
            other => Error::Io(format!("{name}: {other}")),
        })?;
        let mut out = BufWriter::new(File::create(&target).map_err(|e| Error::io(&target, e))?);
        let copied = std::io::copy(&mut entry, &mut out).map_err(|e| {
            Error::Io(format!(
                "{name}: {e} (a wrong password, or a damaged archive)"
            ))
        })?;
        out.flush().map_err(|e| Error::io(&target, e))?;
        report.files += 1;
        report.bytes += copied;
    }
    let case = Case::open(dest)?;
    report.missing_blobs = case.blobs().fsck()?.missing;
    Ok((case, report))
}
