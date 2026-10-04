//! Context documents: turning files and pages into Markdown, and Markdown into chunks
//! ([ADR-0046](../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
//!
//! Reference material — a wiki dump, a vendor PDF, a web page, a report — becomes text the
//! case can search by keyword and by meaning. This crate knows formats and nothing else:
//! no database, no rig, no window. What a format cannot give up (the layout of a scanned
//! page, an image's content) is not guessed at; a file that yields no text is an error
//! that says so, not an empty document.
//!
//! Formats are told apart by what is *inside* the file where that is possible — magic
//! bytes, a root element — and by the name only where it is not: a download without an
//! extension is still a PDF.

pub mod chunk;
pub mod convert;
pub mod fetch;
pub mod html;
pub mod office;
pub mod wiki;
mod xml;

pub use chunk::{CHUNKER, Chunk, chunk};
pub use convert::{Converted, Error, Result, Sink, convert, convert_file};
pub use fetch::{Fetched, fetch};
