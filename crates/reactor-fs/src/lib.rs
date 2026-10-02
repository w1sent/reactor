//! A virtual filesystem seam: browse, stat, read, hash, identify and search — without gpui.
//!
//! The GUI's filesystem explorer and every viewer read through [`Vfs`] rather than
//! `std::fs`, so the day a forensic image's filesystem is mounted behind the same trait
//! nothing above it changes. [`LocalFs`] is the only implementation today.
//!
//! Paths inside a filesystem are [`VPath`]s: normalised, `/`-separated, rooted. A viewer is
//! opened on a [`FileRef`] — a filesystem plus a path — never on a native `PathBuf`.

mod find;
mod hash;
mod identify;
mod local;
mod path;
mod search;
mod vfs;

pub use find::{Found, find_bytes, parse_hex};
pub use hash::{Algorithm, hash_file};
pub use identify::{HEAD_BYTES, Identity, identify, identify_file};
pub use local::LocalFs;
pub use path::VPath;
pub use search::{ContentHit, SearchOptions, search_content, search_names};
pub use vfs::{Entry, FileRef, Kind, ReadAt, Stat, Vfs};
