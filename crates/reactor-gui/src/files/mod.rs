//! The filesystem panels: the explorer and the viewers it opens files in.
//!
//! Every viewer is opened on a [`reactor_fs::FileRef`] — a filesystem and a path in it — so
//! nothing here knows whether the bytes are on the host's disk or inside a forensic image.
//! Viewers are created on demand, any number of them, as tabs in the centre; the explorer is
//! the one fixed panel ([`crate::layout::PanelKind::Files`]).

pub mod editor;
pub mod explorer;
pub mod hex;
pub mod language;
pub mod props;

use gpui_kit::assets::IconName;
use reactor_fs::FileRef;

/// A way of looking at a file. The explorer's *Open with* lists these, and `/open` takes
/// the slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Editor,
    Hex,
}

impl Viewer {
    pub const ALL: [Viewer; 2] = [Viewer::Editor, Viewer::Hex];

    pub fn label(self) -> &'static str {
        match self {
            Viewer::Editor => "Text editor",
            Viewer::Hex => "Hex viewer",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Viewer::Editor => "edit",
            Viewer::Hex => "hex",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Viewer> {
        Viewer::ALL
            .into_iter()
            .find(|v| v.slug().eq_ignore_ascii_case(slug))
    }

    pub fn icon(self) -> IconName {
        match self {
            Viewer::Editor => IconName::FileCode,
            Viewer::Hex => IconName::Binary,
        }
    }
}

/// The viewer a file opens in when the user did not choose one.
pub fn suggest(file: &FileRef) -> Viewer {
    let head = file
        .open()
        .and_then(|reader| reader.read_range(0, reactor_fs::HEAD_BYTES))
        .unwrap_or_default();
    if reactor_fs::identify(&head).is_text {
        Viewer::Editor
    } else {
        Viewer::Hex
    }
}

/// Hands a file or folder to the desktop's default program for it.
pub fn open_external(path: &std::path::Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    let mut child = command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reaped on a thread: the opener may outlive this call, and must not become a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// `1.5 MiB (1572864 bytes)`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(3 << 30), "3.0 GiB");
    }

    #[test]
    fn every_viewer_is_addressable_by_its_slug() {
        for viewer in Viewer::ALL {
            assert_eq!(Viewer::from_slug(viewer.slug()), Some(viewer));
            assert!(!viewer.label().is_empty());
        }
    }
}
