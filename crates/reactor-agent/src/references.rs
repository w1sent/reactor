//! `#path` and `#doc:name` in a message: what they name, and what is sent with the message.
//!
//! A reference stays in the text as the person wrote it. For each one that names something,
//! the content goes along as an attachment ([`crate::entry::Attachment`]), cut to
//! [`MAX_TOKENS`] where it is longer. One that names nothing is left as text.

use std::path::{Path, PathBuf};

use reactor_db::Db;
use reactor_db::corpus::{self, DocumentInfo};

use crate::context::estimate_tokens;
use crate::entry::Attachment;

/// The most one attachment carries: about 20k tokens, as the design says.
pub const MAX_TOKENS: u64 = 20_000;

/// What a reference names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A path, as written.
    File(String),
    /// A document by source, filename or title, and optionally a section of it.
    Doc { name: String, section: Option<String> },
}

/// A reference found in a text, with the span it was written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub target: Target,
    pub span: std::ops::Range<usize>,
}

/// The references in `text`. A `#` counts at the start of the text or after whitespace and
/// when something follows it: `# Heading` and `C#` are not references. A name with spaces is
/// written in quotes: `#"my notes.txt"`, `#doc:"Pump manual"`.
pub fn find(text: &str) -> Vec<Reference> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // `#` is one byte, so `i` is on a character boundary whenever it is looked at below;
        // asking what comes before `i` first would cut a multi-byte character in two.
        if bytes[i] != b'#' {
            i += 1;
            continue;
        }
        let at_start = i == 0 || text[..i].chars().next_back().is_some_and(char::is_whitespace);
        if !at_start {
            i += 1;
            continue;
        }
        let rest = &text[i + 1..];
        let (is_doc, body) = match rest.strip_prefix("doc:") {
            Some(b) => (true, b),
            None => (false, rest),
        };
        let consumed_prefix = rest.len() - body.len();
        let start = i + 1 + consumed_prefix;
        let (name, end) = if let Some(quoted) = body.strip_prefix('"') {
            match quoted.find('"') {
                Some(close) => (quoted[..close].to_string(), start + 1 + close + 1),
                None => {
                    i += 1;
                    continue;
                }
            }
        } else {
            let len = body.find(char::is_whitespace).unwrap_or(body.len());
            let raw = &body[..len];
            // Sentence punctuation after a reference is not part of it.
            let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
            (trimmed.to_string(), start + trimmed.len())
        };
        if name.is_empty() {
            i += 1;
            continue;
        }
        let target = if is_doc {
            // `name#section`: the first `#` after the name starts the section.
            match name.split_once('#') {
                Some((n, s)) if !n.is_empty() && !s.is_empty() => Target::Doc {
                    name: n.to_string(),
                    section: Some(s.to_string()),
                },
                _ => Target::Doc { name, section: None },
            }
        } else {
            Target::File(name)
        };
        out.push(Reference {
            target,
            span: i..end,
        });
        i = end.max(i + 1);
    }
    out
}

/// The documents `name` could mean: the source as stored, its filename, or its title; one
/// kind of match at a time, strongest first, so a name that is a whole source is not made
/// ambiguous by longer ones.
pub fn match_documents<'a>(docs: &'a [DocumentInfo], name: &str) -> Vec<&'a DocumentInfo> {
    let lower = name.to_ascii_lowercase();
    let basename = |d: &DocumentInfo| {
        d.source_uri
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&d.source_uri)
            .to_ascii_lowercase()
    };
    let tiers: [Box<dyn Fn(&DocumentInfo) -> bool>; 4] = [
        Box::new(|d| d.source_uri.eq_ignore_ascii_case(name)),
        Box::new(|d| basename(d) == lower),
        Box::new(|d| {
            d.title
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(name))
        }),
        Box::new(|d| d.source_uri.to_ascii_lowercase().ends_with(&format!("/{lower}"))),
    ];
    for tier in tiers {
        let found: Vec<&DocumentInfo> = docs.iter().filter(|d| tier(d)).collect();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

/// `text` cut to the attachment limit, and whether it was cut.
pub fn limit(text: String) -> (String, bool) {
    if estimate_tokens(&text) <= MAX_TOKENS {
        return (text, false);
    }
    let keep: String = text.chars().take((MAX_TOKENS * 4) as usize).collect();
    (keep, true)
}

/// A text file of the case (or any path the person wrote), as an attachment.
pub fn file(cwd: &Path, written: &str) -> Result<Attachment, String> {
    let path: PathBuf = {
        let p = Path::new(written);
        if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
    };
    let meta = std::fs::metadata(&path).map_err(|e| format!("{written}: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{written} is a folder"));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{written}: {e}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{written} is not a text file: ask the agent to open it with a tool"))?;
    let (text, truncated) = limit(text);
    Ok(Attachment {
        label: written.to_string(),
        source: written.to_string(),
        part: String::new(),
        tokens: estimate_tokens(&text),
        text,
        truncated,
        via: "reference".into(),
    })
}

/// A document of the case, whole or one section of it, as an attachment.
pub fn document(db: &Db, name: &str, section: Option<&str>) -> Result<Attachment, String> {
    let docs = corpus::documents(db).map_err(|e| e.to_string())?;
    let matches = match_documents(&docs, name);
    let doc = match matches.as_slice() {
        [one] => *one,
        [] => return Err(format!("no document called {name}")),
        many => {
            return Err(format!(
                "{name} could be {} documents; write more of its source",
                many.len()
            ));
        }
    };
    let passages = corpus::document_passages(db, &doc.id).map_err(|e| e.to_string())?;
    let wanted = section.map(str::to_ascii_lowercase);
    let chosen: Vec<_> = passages
        .iter()
        .filter(|p| {
            wanted
                .as_deref()
                .is_none_or(|w| p.heading_path.to_ascii_lowercase().contains(w))
        })
        .collect();
    if chosen.is_empty() {
        return Err(match section {
            Some(s) => format!("{} has no section {s}", doc.source_uri),
            None => format!("{} has no text", doc.source_uri),
        });
    }
    let text = chosen
        .iter()
        .map(|p| {
            if p.heading_path.is_empty() {
                p.text.clone()
            } else {
                format!("### {}\n{}", p.heading_path, p.text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let (text, truncated) = limit(text);
    Ok(Attachment {
        label: doc.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| doc.source_uri.clone()),
        source: doc.source_uri.clone(),
        part: match section {
            Some(s) => s.to_string(),
            None => "whole document".into(),
        },
        tokens: estimate_tokens(&text),
        text,
        truncated,
        via: "reference".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source: &str, title: Option<&str>) -> DocumentInfo {
        DocumentInfo {
            id: source.into(),
            source_uri: source.into(),
            title: title.map(str::to_string),
            mime: None,
            origin: "file".into(),
            chunks: 1,
            added_at: String::new(),
        }
    }

    #[test]
    fn text_with_multi_byte_characters_before_and_around_a_hash_does_not_panic() {
        // An em dash is three bytes; a scan that steps by byte lands inside it.
        let text = "Task — enumerate the imports — then #notes.txt — and “quoted” #doc:\"Pump — manual\"";
        let found = find(text);
        let targets: Vec<_> = found.iter().map(|r| r.target.clone()).collect();
        assert_eq!(
            targets,
            vec![
                Target::File("notes.txt".into()),
                Target::Doc { name: "Pump — manual".into(), section: None }
            ]
        );
        for r in &found {
            assert!(text.is_char_boundary(r.span.start) && text.is_char_boundary(r.span.end));
        }
        assert!(find("—#").is_empty() && find("日本語 #").is_empty() && find("é").is_empty());
    }

    #[test]
    fn a_hash_after_whitespace_with_something_after_it_is_a_reference() {
        let found = find("see #logs/a.txt, and #doc:manual.pdf#2.2. # Heading, C# and #");
        let targets: Vec<_> = found.iter().map(|r| r.target.clone()).collect();
        assert_eq!(
            targets,
            [
                Target::File("logs/a.txt".into()),
                Target::Doc {
                    name: "manual.pdf".into(),
                    section: Some("2.2".into())
                },
            ]
        );
    }

    #[test]
    fn quotes_carry_spaces() {
        let found = find(r#"#"my notes.txt" and #doc:"Pump manual""#);
        assert_eq!(found[0].target, Target::File("my notes.txt".into()));
        assert_eq!(
            found[1].target,
            Target::Doc {
                name: "Pump manual".into(),
                section: None
            }
        );
    }

    #[test]
    fn spans_cover_what_was_written() {
        let text = "x #doc:a.pdf, y";
        let r = &find(text)[0];
        assert_eq!(&text[r.span.clone()], "#doc:a.pdf");
    }

    #[test]
    fn the_strongest_kind_of_match_wins() {
        let docs = [
            doc("docs/vendor/ps-manual.pdf", Some("Pump manual")),
            doc("old/ps-manual.pdf", None),
            doc("notes/ps-manual.pdf.bak", None),
        ];
        assert_eq!(match_documents(&docs, "docs/vendor/ps-manual.pdf").len(), 1);
        // The filename names two documents: ambiguous, so the caller can say so.
        assert_eq!(match_documents(&docs, "ps-manual.pdf").len(), 2);
        assert_eq!(match_documents(&docs, "pump manual").len(), 1);
        assert!(match_documents(&docs, "missing").is_empty());
    }

    #[test]
    fn long_text_is_cut_and_says_so() {
        let (kept, cut) = limit("x".repeat(MAX_TOKENS as usize * 4 + 10));
        assert!(cut);
        assert_eq!(kept.chars().count(), MAX_TOKENS as usize * 4);
        assert!(!limit("short".into()).1);
    }
}
