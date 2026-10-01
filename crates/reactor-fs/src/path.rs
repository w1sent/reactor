use std::fmt;

/// A normalised path inside a [`crate::Vfs`]: always rooted at `/`, `/`-separated, with no
/// `.` or `..` components and no trailing slash. Its text is not a native path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VPath(String);

impl VPath {
    pub fn root() -> Self {
        VPath("/".into())
    }

    /// Normalise any `/`-separated text: `..` pops (never above the root), `.` and empty
    /// components vanish.
    pub fn new(text: &str) -> Self {
        let mut parts: Vec<&str> = Vec::new();
        for part in text.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        if parts.is_empty() {
            return Self::root();
        }
        VPath(format!("/{}", parts.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|part| !part.is_empty())
    }

    pub fn file_name(&self) -> Option<&str> {
        self.components().last()
    }

    pub fn parent(&self) -> Option<VPath> {
        if self.is_root() {
            return None;
        }
        let cut = self.0.rfind('/').unwrap_or(0);
        Some(VPath::new(&self.0[..cut]))
    }

    pub fn join(&self, name: &str) -> VPath {
        VPath::new(&format!("{}/{}", self.0, name))
    }

    /// Without the leading slash — the form a tree shows ("" for the root).
    pub fn relative(&self) -> &str {
        self.0.trim_start_matches('/')
    }

    /// The text after the last `.` of the file name, lowercased; none for `Makefile`,
    /// `.bashrc`, or `archive.`.
    pub fn extension(&self) -> Option<String> {
        let name = self.file_name()?;
        let dot = name.rfind('.')?;
        (dot > 0 && dot + 1 < name.len()).then(|| name[dot + 1..].to_ascii_lowercase())
    }
}

impl fmt::Display for VPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises() {
        assert_eq!(VPath::new("a//b/./c/").as_str(), "/a/b/c");
        assert_eq!(VPath::new("/a/../../b").as_str(), "/b");
        assert_eq!(VPath::new("").as_str(), "/");
    }

    #[test]
    fn parent_and_join() {
        let p = VPath::new("/a/b");
        assert_eq!(p.parent().unwrap().as_str(), "/a");
        assert_eq!(VPath::new("/a").parent().unwrap(), VPath::root());
        assert_eq!(VPath::root().parent(), None);
        assert_eq!(p.join("../c").as_str(), "/a/c");
        assert_eq!(p.file_name(), Some("b"));
        assert_eq!(p.relative(), "a/b");
    }

    #[test]
    fn extension() {
        assert_eq!(VPath::new("/x/Foo.RS").extension().as_deref(), Some("rs"));
        assert_eq!(VPath::new("/x/.bashrc").extension(), None);
        assert_eq!(VPath::new("/x/Makefile").extension(), None);
        assert_eq!(VPath::new("/x/a.tar.gz").extension().as_deref(), Some("gz"));
    }
}
