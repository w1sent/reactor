//! The filesystem panels: the explorer and the viewers it opens files in.
//!
//! Every viewer is opened on a [`reactor_fs::FileRef`] — a filesystem and a path in it — so
//! nothing here knows whether the bytes are on the host's disk or inside a forensic image.
//! Viewers are created on demand, any number of them, as tabs in the centre; the explorer is
//! the one fixed panel ([`crate::layout::PanelKind::Files`]).

pub mod hex;

use gpui_kit::assets::IconName;
use reactor_fs::FileRef;

/// A way of looking at a file. The explorer's *Open with* lists these, and `/open` takes
/// the slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Hex,
}

impl Viewer {
    pub const ALL: [Viewer; 1] = [Viewer::Hex];

    pub fn label(self) -> &'static str {
        match self {
            Viewer::Hex => "Hex viewer",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
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
            Viewer::Hex => IconName::Binary,
        }
    }
}

/// The viewer a file opens in when the user did not choose one.
pub fn suggest(_file: &FileRef) -> Viewer {
    Viewer::Hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_viewer_is_addressable_by_its_slug() {
        for viewer in Viewer::ALL {
            assert_eq!(Viewer::from_slug(viewer.slug()), Some(viewer));
            assert!(!viewer.label().is_empty());
        }
    }
}
