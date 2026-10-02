use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use crate::VPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Other,
}

/// One row of a directory listing.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: VPath,
    pub kind: Kind,
    pub is_symlink: bool,
    pub size: Option<u64>,
}

/// Everything known about one path. `extra` carries what only this filesystem has — an
/// inode, an NTFS stream, a deleted flag — as label/value rows the properties popup lists
/// after the common ones.
#[derive(Debug, Clone, Default)]
pub struct Stat {
    pub kind: Option<Kind>,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    /// Unix permission bits, where the filesystem has them.
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub symlink_target: Option<String>,
    pub readonly: bool,
    pub extra: Vec<(String, String)>,
}

/// Random access to one file's bytes.
pub trait ReadAt: Send + Sync {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Fills as much of `buf` as the file has from `offset`; returns the count (0 at the end).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// Up to `len` bytes from `offset`.
    fn read_range(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len.min(self.len().saturating_sub(offset) as usize)];
        let mut filled = 0;
        while filled < buf.len() {
            let n = self.read_at(offset + filled as u64, &mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }
}

fn unsupported<T>(what: &str) -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} is not supported here"),
    ))
}

/// A browsable, readable filesystem. Mutations default to "unsupported": a forensic image is
/// read-only, and the explorer greys out what [`Vfs::writable`] says it cannot do.
pub trait Vfs: Send + Sync {
    /// What the tree shows at its root ("." for the working directory, an image's name…).
    fn label(&self) -> String;
    fn read_dir(&self, path: &VPath) -> io::Result<Vec<Entry>>;
    fn stat(&self, path: &VPath) -> io::Result<Stat>;
    fn open(&self, path: &VPath) -> io::Result<Box<dyn ReadAt>>;

    /// The same file as a native path, for handing to another program. None for anything
    /// that does not exist on the host's disk.
    fn local_path(&self, _path: &VPath) -> Option<PathBuf> {
        None
    }
    fn writable(&self) -> bool {
        false
    }
    fn create_file(&self, _path: &VPath) -> io::Result<()> {
        unsupported("creating a file")
    }
    fn create_dir(&self, _path: &VPath) -> io::Result<()> {
        unsupported("creating a folder")
    }
    fn rename(&self, _from: &VPath, _to: &VPath) -> io::Result<()> {
        unsupported("renaming")
    }
    fn remove(&self, _path: &VPath) -> io::Result<()> {
        unsupported("deleting")
    }
    /// Replaces the file's contents.
    fn write(&self, _path: &VPath, _bytes: &[u8]) -> io::Result<()> {
        unsupported("saving")
    }
}

/// A file somewhere: the filesystem and the path in it. What every viewer is opened on.
#[derive(Clone)]
pub struct FileRef {
    pub fs: Arc<dyn Vfs>,
    pub path: VPath,
}

impl FileRef {
    pub fn new(fs: Arc<dyn Vfs>, path: VPath) -> Self {
        Self { fs, path }
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(str::to_owned)
            .unwrap_or_else(|| self.fs.label())
    }

    pub fn open(&self) -> io::Result<Box<dyn ReadAt>> {
        self.fs.open(&self.path)
    }

    /// The whole file, refusing past `limit` bytes — viewers that hold the bytes in memory
    /// say so rather than quietly truncating.
    pub fn read_all(&self, limit: u64) -> io::Result<Vec<u8>> {
        let file = self.open()?;
        if file.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!("{} bytes is over the {limit} byte limit", file.len()),
            ));
        }
        file.read_range(0, file.len() as usize)
    }
}
