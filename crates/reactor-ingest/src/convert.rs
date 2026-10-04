//! One entry point: bytes (or a file) in, Markdown documents out.

use std::fmt;
use std::io::Read;
use std::path::Path;

/// A document, as Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub title: Option<String>,
    pub markdown: String,
    /// What it was: `text/markdown`, `application/pdf`, `text/html`, …
    pub mime: String,
    /// Where in the source this document is, when the source holds several (a page of a
    /// wiki dump): appended to the source's URI as a fragment.
    pub fragment: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    Io(String),
    /// The file is of a kind that is understood, and is damaged or unreadable.
    Unreadable(String),
    /// A kind this crate does not read.
    Unsupported(String),
    /// It was read and had no text in it.
    Empty(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m) | Error::Unreadable(m) | Error::Unsupported(m) | Error::Empty(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Larger files are not read into memory whole. (A wiki dump is streamed, and has no limit.)
pub const MAX_BYTES: u64 = 512 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Markdown,
    Text,
    Html,
    Pdf,
    Docx,
    Odt,
    Epub,
    MediaWiki,
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// What these bytes are. Content decides; the name is only a tie-breaker where the
/// content cannot say (Markdown is plain text, and so is a log).
fn sniff(name: &str, bytes: &[u8]) -> Kind {
    if bytes.starts_with(b"%PDF-") {
        return Kind::Pdf;
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return match extension(name).as_str() {
            "odt" => Kind::Odt,
            "epub" => Kind::Epub,
            _ => zip_kind(bytes).unwrap_or(Kind::Docx),
        };
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.contains("<mediawiki") {
        return Kind::MediaWiki;
    }
    if head.starts_with("<!doctype html") || head.contains("<html") || head.contains("<body") {
        return Kind::Html;
    }
    match extension(name).as_str() {
        "html" | "htm" | "xhtml" => Kind::Html,
        "md" | "markdown" | "mdown" => Kind::Markdown,
        "xml" if head.contains("<page") => Kind::MediaWiki,
        _ => Kind::Text,
    }
}

/// Which office format a zip is, from the files inside it.
fn zip_kind(bytes: &[u8]) -> Option<Kind> {
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let names: Vec<&str> = zip.file_names().collect();
    if names.contains(&"word/document.xml") {
        Some(Kind::Docx)
    } else if names.contains(&"content.xml") {
        Some(Kind::Odt)
    } else if names.iter().any(|n| n.ends_with(".opf")) {
        Some(Kind::Epub)
    } else {
        None
    }
}

/// Read text as UTF-8, or as Latin-1 when it is not: a document is never refused for its
/// encoding, and a lossy reading is better than none.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

fn first_heading(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|l| {
        let t = l.trim_start();
        let rest = t.strip_prefix("# ")?;
        Some(rest.trim().to_string()).filter(|s| !s.is_empty())
    })
}

/// Tidy what a converter produced: no trailing spaces, no runs of blank lines.
pub(crate) fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.replace("\r\n", "\n").lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// One document. Its title is the one the format gave it, else its first heading, else
/// `fallback` (the file's name).
fn one(
    title: Option<String>,
    markdown: String,
    mime: &str,
    what: &str,
    fallback: Option<String>,
) -> Result<Vec<Converted>> {
    let markdown = tidy(&markdown);
    if markdown.is_empty() {
        return Err(Error::Empty(format!(
            "{what}: no text in it (a scan or an image needs OCR, which this does not do)"
        )));
    }
    Ok(vec![Converted {
        title: title.or_else(|| first_heading(&markdown)).or(fallback),
        markdown,
        mime: mime.to_string(),
        fragment: None,
    }])
}

/// Convert a document held in memory. `name` is its file name or URL, used for the title
/// and to break ties; one source may hold several documents.
pub fn convert(name: &str, bytes: &[u8]) -> Result<Vec<Converted>> {
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty());
    match sniff(name, bytes) {
        Kind::Markdown => one(None, decode(bytes), "text/markdown", name, stem),
        Kind::Text => {
            // A NUL in the first kilobytes: it is a binary file, and its "text" would be noise.
            if bytes[..bytes.len().min(8192)].contains(&0) {
                return Err(Error::Unsupported(format!(
                    "{name}: a binary file, not a document"
                )));
            }
            one(None, decode(bytes), "text/plain", name, stem)
        }
        Kind::Html => {
            let page = crate::html::to_markdown(&decode(bytes));
            one(page.title, page.markdown, "text/html", name, stem)
        }
        Kind::Pdf => {
            let text = crate::office::pdf(bytes)?;
            one(None, text, "application/pdf", name, stem)
        }
        Kind::Docx => {
            let text = crate::office::docx(bytes)?;
            one(
                None,
                text,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                name,
                stem,
            )
        }
        Kind::Odt => {
            let text = crate::office::odt(bytes)?;
            one(
                None,
                text,
                "application/vnd.oasis.opendocument.text",
                name,
                stem,
            )
        }
        Kind::Epub => {
            let (title, text) = crate::office::epub(bytes)?;
            one(title, text, "application/epub+zip", name, stem)
        }
        Kind::MediaWiki => {
            let mut out = Vec::new();
            crate::wiki::pages(std::io::Cursor::new(bytes), &mut |c| {
                out.push(c);
                Ok(())
            })?;
            if out.is_empty() {
                return Err(Error::Empty(format!("{name}: no pages in the dump")));
            }
            Ok(out)
        }
    }
}

/// What receives each document of a file: the document, and the source's bytes if it is
/// that file.
pub type Sink<'a> = dyn FnMut(Converted, Option<&[u8]>) -> Result<()> + 'a;

/// Convert a file, handing each document to `sink` as it is made, with the source's bytes
/// when the document *is* the file (`None` when it is one of many in a stream, and the
/// file is too large to keep per page). A wiki dump is read as a stream, so a
/// multi-gigabyte one costs time and not memory; anything else is read whole and limited
/// to [`MAX_BYTES`].
pub fn convert_file(path: &Path, sink: &mut Sink) -> Result<()> {
    let name = path.to_string_lossy();
    let mut file = std::fs::File::open(path).map_err(|e| Error::Io(format!("{name}: {e}")))?;
    let size = file
        .metadata()
        .map_err(|e| Error::Io(format!("{name}: {e}")))?
        .len();
    let mut head = vec![0u8; 4096.min(size as usize)];
    file.read_exact(&mut head)
        .map_err(|e| Error::Io(format!("{name}: {e}")))?;
    if sniff(&name, &head) == Kind::MediaWiki {
        let file = std::fs::File::open(path).map_err(|e| Error::Io(format!("{name}: {e}")))?;
        return crate::wiki::pages(std::io::BufReader::new(file), &mut |c| sink(c, None));
    }
    if size > MAX_BYTES {
        return Err(Error::Unsupported(format!(
            "{name}: {} MiB is over the {} MiB limit for one document",
            size >> 20,
            MAX_BYTES >> 20
        )));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::Io(format!("{name}: {e}")))?;
    let docs = convert(&name, &bytes)?;
    // A source that holds several documents (a small dump) is not any one of them.
    let alone = docs.len() == 1;
    for doc in docs {
        sink(doc, alone.then_some(bytes.as_slice()))?;
    }
    Ok(())
}
