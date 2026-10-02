//! The filesystem panels: the explorer and the viewers it opens files in.
//!
//! Every viewer is opened on a [`reactor_fs::FileRef`] — a filesystem and a path in it — so
//! nothing here knows whether the bytes are on the host's disk or inside a forensic image.
//! Viewers are created on demand, any number of them, as tabs in the centre; the explorer is
//! the one fixed panel ([`crate::layout::PanelKind::Files`]).

pub mod data;
pub mod data_model;
pub mod editor;
pub mod explorer;
pub mod hex;
pub mod language;
pub mod props;
pub mod refs;
pub mod table;
pub mod table_model;

use gpui_kit::assets::IconName;
use reactor_fs::FileRef;

/// A way of looking at a file. The explorer's *Open with* lists these, and `/open` takes
/// the slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Editor,
    Table,
    Data,
    Hex,
}

impl Viewer {
    pub const ALL: [Viewer; 4] = [Viewer::Editor, Viewer::Table, Viewer::Data, Viewer::Hex];

    pub fn label(self) -> &'static str {
        match self {
            Viewer::Editor => "Text editor",
            Viewer::Table => "Table viewer",
            Viewer::Data => "Data viewer",
            Viewer::Hex => "Hex viewer",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Viewer::Editor => "edit",
            Viewer::Table => "table",
            Viewer::Data => "data",
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
            Viewer::Table => IconName::Table,
            Viewer::Data => IconName::Image,
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

/// Asks the operating system's own program picker which program to open the file with.
///
/// Linux: the desktop portal's OpenURI with `ask`, which shows the desktop's "Open With"
/// chooser, handing it the open file (and falls back to the default program when there is
/// no portal). macOS: the
/// system's application chooser, then `open -a`. Windows: the shell's Open With dialog.
pub fn open_with_picker(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // The portal wants the file itself, as a descriptor, so this cannot be a command line.
        let file = std::fs::File::open(path)?;
        let path = path.to_owned();
        std::thread::spawn(move || {
            let asked = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|runtime| {
                    runtime.block_on(async {
                        ashpd::desktop::open_uri::OpenFileRequest::default()
                            .ask(true)
                            .send_file(&file)
                            .await
                            // The request holds a D-Bus subscription whose drop needs this runtime, so
                            // it is dropped here, inside it, and not by whoever receives the result.
                            .map(drop)
                            .map_err(|e| e.to_string())
                    })
                });
            // No portal: the default program is better than nothing.
            if asked.is_err() {
                let _ = open_external(&path);
            }
        });
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        use std::process::{Command, Stdio};
        #[cfg(target_os = "windows")]
        let mut command = {
            let mut c = Command::new("rundll32.exe");
            c.arg("shell32.dll,OpenAs_RunDLL").arg(path);
            c
        };
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(
                    "app=$(osascript -e 'set a to choose application' -e 'POSIX path of (path to a)') \\
                     && open -a \"$app\" \"$1\"",
                )
                .arg("sh")
                .arg(path);
            c
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
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
