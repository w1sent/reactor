use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::VPath;
use crate::vfs::{Entry, Kind, ReadAt, Stat, Vfs};

/// The host's disk, rooted at a directory: `/` is that directory, so the explorer shows the
/// tree relative to the working directory.
pub struct LocalFs {
    root: PathBuf,
    label: String,
}

struct LocalFile {
    file: fs::File,
    len: u64,
}

impl ReadAt for LocalFile {
    fn len(&self) -> u64 {
        self.len
    }

    #[cfg(unix)]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
    }

    #[cfg(windows)]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
    }
}

fn kind_of(file_type: fs::FileType) -> Kind {
    if file_type.is_dir() {
        Kind::Dir
    } else if file_type.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

impl LocalFs {
    pub fn new(root: impl Into<PathBuf>, label: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            label: label.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn native(&self, path: &VPath) -> PathBuf {
        let mut native = self.root.clone();
        native.extend(path.components());
        native
    }

    /// The path inside this filesystem of a native one, when it lies under the root.
    pub fn virtual_path(&self, native: &Path) -> Option<VPath> {
        let rest = native.strip_prefix(&self.root).ok()?;
        let text: Vec<_> = rest
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        Some(VPath::new(&text.join("/")))
    }
}

impl Vfs for LocalFs {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn read_dir(&self, path: &VPath) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for item in fs::read_dir(self.native(path))? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            let link = item.file_type()?;
            // A symlink is listed as what it points to, so a link to a folder opens.
            let target = fs::metadata(item.path()).ok();
            let kind = target
                .as_ref()
                .map(|m| kind_of(m.file_type()))
                .unwrap_or(kind_of(link));
            entries.push(Entry {
                path: path.join(&name),
                size: (kind == Kind::File)
                    .then(|| target.map(|m| m.len()))
                    .flatten(),
                is_symlink: link.is_symlink(),
                kind,
                name,
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != Kind::Dir)
                .cmp(&(b.kind != Kind::Dir))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn stat(&self, path: &VPath) -> io::Result<Stat> {
        let native = self.native(path);
        let link = fs::symlink_metadata(&native)?;
        let meta = fs::metadata(&native).unwrap_or_else(|_| link.clone());
        let mut stat = Stat {
            kind: Some(kind_of(meta.file_type())),
            size: meta.len(),
            modified: meta.modified().ok(),
            accessed: meta.accessed().ok(),
            created: meta.created().ok(),
            readonly: meta.permissions().readonly(),
            ..Stat::default()
        };
        if link.is_symlink() {
            stat.symlink_target = fs::read_link(&native)
                .ok()
                .map(|target| target.to_string_lossy().into_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            stat.mode = Some(meta.mode());
            stat.uid = Some(meta.uid());
            stat.gid = Some(meta.gid());
            stat.extra.push(("Inode".into(), meta.ino().to_string()));
            stat.extra
                .push(("Hard links".into(), meta.nlink().to_string()));
        }
        Ok(stat)
    }

    fn open(&self, path: &VPath) -> io::Result<Box<dyn ReadAt>> {
        let file = fs::File::open(self.native(path))?;
        let len = file.metadata()?.len();
        Ok(Box::new(LocalFile { file, len }))
    }

    fn local_path(&self, path: &VPath) -> Option<PathBuf> {
        Some(self.native(path))
    }

    fn writable(&self) -> bool {
        true
    }

    fn create_file(&self, path: &VPath) -> io::Result<()> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.native(path))
            .map(drop)
    }

    fn create_dir(&self, path: &VPath) -> io::Result<()> {
        fs::create_dir(self.native(path))
    }

    fn rename(&self, from: &VPath, to: &VPath) -> io::Result<()> {
        let to = self.native(to);
        if to.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the target already exists",
            ));
        }
        fs::rename(self.native(from), to)
    }

    fn remove(&self, path: &VPath) -> io::Result<()> {
        let native = self.native(path);
        if fs::symlink_metadata(&native)?.is_dir() {
            fs::remove_dir_all(native)
        } else {
            fs::remove_file(native)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, LocalFs) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("b.txt"), "hello").unwrap();
        fs::write(dir.path().join("A.txt"), "x").unwrap();
        let fs = LocalFs::new(dir.path(), ".");
        (dir, fs)
    }

    #[test]
    fn lists_folders_first_then_by_name() {
        let (_dir, fs) = fixture();
        let names: Vec<_> = fs
            .read_dir(&VPath::root())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["sub", "A.txt", "b.txt"]);
    }

    #[test]
    fn reads_a_range_and_stats() {
        let (_dir, fs) = fixture();
        let path = VPath::new("/b.txt");
        assert_eq!(fs.open(&path).unwrap().read_range(1, 3).unwrap(), b"ell");
        assert_eq!(fs.stat(&path).unwrap().size, 5);
    }

    #[test]
    fn mutations_round_trip_and_round_back_to_virtual_paths() {
        let (dir, fs) = fixture();
        fs.create_file(&VPath::new("/new.txt")).unwrap();
        assert!(fs.create_file(&VPath::new("/new.txt")).is_err());
        fs.rename(&VPath::new("/new.txt"), &VPath::new("/sub/moved.txt"))
            .unwrap();
        assert_eq!(
            fs.virtual_path(&dir.path().join("sub/moved.txt")),
            Some(VPath::new("/sub/moved.txt"))
        );
        fs.remove(&VPath::new("/sub")).unwrap();
        assert!(!dir.path().join("sub").exists());
    }

    #[test]
    fn dotdot_cannot_climb_out_of_the_root() {
        let (dir, fs) = fixture();
        assert_eq!(
            fs.native(&VPath::new("/../../etc/passwd")),
            dir.path().join("etc/passwd")
        );
    }
}
