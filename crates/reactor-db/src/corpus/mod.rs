//! The context corpus
//! ([ADR-0046](../../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)):
//! reference documents — a wiki, vendor papers, advisories — kept so the agent can find
//! what they say by keyword and by meaning.
//!
//! A document is stored whole (its bytes and its Markdown, as blobs: they come from
//! anywhere, and a case must hold what it was built on) and as passages. Passages are
//! what is indexed: a keyword index (FTS5) always, and vectors for each embedding model
//! that has been chosen. The two are *derived* — [`rebuild_indexes`] regenerates them from
//! the passages — and never the only copy of anything.
//!
//! This crate stores and searches. Converting a file into Markdown is `reactor-ingest`'s,
//! and making a vector is the agent's (it has the model provider); both arrive here as
//! plain data.

mod search;
mod vectors;

pub use search::{Hit, Options, Query, hits_at, search, search_ranked, search_with};
pub use vectors::{
    Model, Progress, clear_vectors, forget_model, models, pending, progress, rebuild_indexes,
    register_model, store_vectors, usable_model,
};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::case::Case;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::manifest::{Author, author_id};
use crate::time::now_rfc3339;

/// Where a document says it came from, for what a reader should make of it. Reference
/// text is *data*: an instruction inside it is not the user's.
pub const ORIGIN_EXTERNAL: &str = "external";

/// A document ready to be stored.
#[derive(Debug, Clone)]
pub struct NewDocument {
    /// The file's path or the page's URL, with a fragment where the source holds several
    /// documents (`dump.xml#Emotet`).
    pub source_uri: String,
    /// `file`, `url`, `wiki` or `import`.
    pub source_kind: String,
    pub mime: Option<String>,
    pub title: Option<String>,
    pub origin: String,
    /// When a page was fetched, as RFC 3339.
    pub fetched_at: Option<String>,
    pub meta: Value,
    /// The source's bytes, as they were. Kept so the document can be converted again.
    pub raw: Vec<u8>,
    pub markdown: String,
    /// What the chunker was called, and the passages: heading path and text.
    pub chunker: String,
    pub chunks: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// The same source with the same content is already there: nothing was written.
    Unchanged { id: String },
    /// Stored. `replaced` earlier versions of the same source were removed.
    Stored {
        id: String,
        chunks: usize,
        replaced: usize,
    },
}

impl Added {
    pub fn id(&self) -> &str {
        match self {
            Added::Unchanged { id } | Added::Stored { id, .. } => id,
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether a passage has nothing in it to find: only headings, code-fence lines and rules,
/// no words of its own. A splitter that cuts at a heading can leave one behind (`## Consequences`,
/// ```` ```sql ````). Its embedding says only "a heading", which sits near the middle of
/// every corpus and so comes out as a close match for any vague question.
pub fn is_hollow(text: &str) -> bool {
    text.lines().map(str::trim).all(|line| {
        line.is_empty()
            // An ATX heading: the marks, then a space or nothing.
            || line.trim_start_matches('#').is_empty()
            || (line.starts_with('#') && line.trim_start_matches('#').starts_with(' '))
            // A code fence, with its language, or a rule, or a setext underline.
            || line.starts_with("```")
            || line.starts_with("~~~")
            || line.chars().all(|c| matches!(c, '=' | '-' | '*' | '_'))
    })
}

/// Join passages that have nothing in them to the one that follows, so that a heading is
/// stored with what it heads, and a fence opener with its code. One at the end joins the
/// one before it; a document of nothing else keeps them as they are.
fn merge_hollow(chunks: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(chunks.len());
    let mut held: Vec<(String, String)> = Vec::new();
    for (heading, text) in chunks {
        if is_hollow(&text) {
            held.push((heading, text));
            continue;
        }
        let mut joined = String::new();
        for (_, hollow) in held.drain(..) {
            joined.push_str(hollow.trim_end());
            joined.push_str("\n\n");
        }
        joined.push_str(&text);
        out.push((heading, joined));
    }
    if !held.is_empty() {
        match out.last_mut() {
            Some((_, last)) => {
                for (_, hollow) in held {
                    last.push_str("\n\n");
                    last.push_str(hollow.trim());
                }
            }
            None => out = held,
        }
    }
    out
}

/// Store a document. The same source and content again is a no-op; the same source with
/// new content replaces what was there, passages and vectors with it. Passages with
/// nothing in them are joined to their neighbours first ([`is_hollow`]).
pub fn add_document(case: &Case, mut doc: NewDocument) -> Result<Added> {
    doc.chunks = merge_hollow(std::mem::take(&mut doc.chunks));
    if doc.chunks.is_empty() {
        return Err(Error::Invalid(format!(
            "{}: nothing to search in it",
            doc.source_uri
        )));
    }
    // Bytes first, rows after: a row never names a file that is not there.
    let (raw_sha, _) = case.blobs().put(&doc.raw)?;
    let (md_sha, _) = case.blobs().put(doc.markdown.as_bytes())?;
    case.db().write(move |tx| {
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM document WHERE source_uri = ?1 AND content_sha256 = ?2",
                params![doc.source_uri, raw_sha],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            // Stored by a version that left passages with nothing in them: store it again,
            // as it would be now, so that importing the folder once more repairs the case.
            let hollow = {
                let mut stmt = tx.prepare("SELECT text FROM chunk WHERE document_id = ?1")?;
                let texts: Vec<String> = stmt
                    .query_map([&id], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?;
                texts.iter().any(|t| is_hollow(t))
            };
            if !hollow {
                return Ok(Added::Unchanged { id });
            }
        }
        let older: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM document WHERE source_uri = ?1")?;
            stmt.query_map([&doc.source_uri], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for id in &older {
            remove_in(tx, id)?;
        }
        let id = uuid::Uuid::now_v7().to_string();
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO document(id, source_uri, source_kind, content_sha256, normalized_md_sha256,
               mime, title, origin, fetched_at, added_at, meta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                doc.source_uri,
                doc.source_kind,
                raw_sha,
                md_sha,
                doc.mime,
                doc.title,
                doc.origin,
                doc.fetched_at,
                now,
                doc.meta.to_string()
            ],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO chunk(document_id, ord, heading_path, text, text_sha256, chunker)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (ord, (heading, text)) in doc.chunks.iter().enumerate() {
                insert.execute(params![
                    id,
                    ord as i64,
                    heading,
                    text,
                    sha256_hex(text.as_bytes()),
                    doc.chunker
                ])?;
            }
        }
        let author = author_id(tx, &Author::analyst())?;
        tx.execute(
            "INSERT INTO event(id, ts, author_id, type, payload) VALUES (?1, ?2, ?3, 'ingest', ?4)",
            params![
                uuid::Uuid::now_v7().to_string(),
                now,
                author,
                json!({
                    "source": doc.source_uri,
                    "document": id,
                    "chunks": doc.chunks.len(),
                    "replaced": older.len()
                })
                .to_string()
            ],
        )?;
        Ok(Added::Stored {
            id,
            chunks: doc.chunks.len(),
            replaced: older.len(),
        })
    })
}

/// Delete a document, its passages, their keyword entries and their vectors — together or
/// not at all. A vector table cannot carry a foreign key, so this is where they go.
fn remove_in(tx: &rusqlite::Transaction, id: &str) -> Result<()> {
    let chunks: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM chunk WHERE document_id = ?1")?;
        stmt.query_map([id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    let models: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM embedding_model")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    for model in models {
        let table = vectors::table(model);
        // A model whose vector table is not there has nothing to remove.
        if let Ok(mut delete) = tx.prepare(&format!("DELETE FROM {table} WHERE chunk_id = ?1")) {
            for chunk in &chunks {
                delete.execute([chunk])?;
            }
        }
    }
    // Chunks go with their document (cascade), which also clears the keyword index and the
    // embedding bookkeeping.
    tx.execute("DELETE FROM document WHERE id = ?1", [id])?;
    Ok(())
}

/// Remove a document from the corpus.
pub fn remove_document(db: &Db, id: &str) -> Result<()> {
    let id = id.to_string();
    db.write(move |tx| {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM document WHERE id = ?1)",
            [&id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Error::Invalid(format!("no document {id}")));
        }
        remove_in(tx, &id)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentInfo {
    pub id: String,
    pub source_uri: String,
    pub title: Option<String>,
    pub mime: Option<String>,
    pub origin: String,
    pub chunks: usize,
    pub added_at: String,
}

/// The documents, newest first.
pub fn documents(db: &Db) -> Result<Vec<DocumentInfo>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT d.id, d.source_uri, d.title, d.mime, d.origin, d.added_at,
                    (SELECT count(*) FROM chunk c WHERE c.document_id = d.id)
               FROM document d ORDER BY d.added_at DESC, d.source_uri",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(DocumentInfo {
                    id: r.get(0)?,
                    source_uri: r.get(1)?,
                    title: r.get(2)?,
                    mime: r.get(3)?,
                    origin: r.get(4)?,
                    added_at: r.get(5)?,
                    chunks: r.get::<_, i64>(6)? as usize,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// A passage with its place in its document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Passage {
    pub chunk_id: i64,
    /// Its position among the document's passages, from 0.
    pub ord: usize,
    pub heading_path: String,
    pub text: String,
}

/// The passages of a passage's document from `before` places ahead of it to `after` places
/// behind, in reading order and with the passage itself among them: what a hit was in the
/// middle of.
pub fn surrounding(db: &Db, chunk_id: i64, before: usize, after: usize) -> Result<Vec<Passage>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.ord, c.heading_path, c.text
               FROM chunk c
              WHERE c.document_id = (SELECT document_id FROM chunk WHERE id = ?1)
                AND c.ord BETWEEN (SELECT ord FROM chunk WHERE id = ?1) - ?2
                              AND (SELECT ord FROM chunk WHERE id = ?1) + ?3
              ORDER BY c.ord",
        )?;
        let rows = stmt
            .query_map(params![chunk_id, before as i64, after as i64], |r| {
                Ok(Passage {
                    chunk_id: r.get(0)?,
                    ord: r.get::<_, i64>(1)? as usize,
                    heading_path: r.get(2)?,
                    text: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// All the passages of a document, in reading order.
pub fn document_passages(db: &Db, document_id: &str) -> Result<Vec<Passage>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ord, heading_path, text FROM chunk WHERE document_id = ?1 ORDER BY ord",
        )?;
        let rows = stmt
            .query_map(params![document_id], |r| {
                Ok(Passage {
                    chunk_id: r.get(0)?,
                    ord: r.get::<_, i64>(1)? as usize,
                    heading_path: r.get(2)?,
                    text: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Whether there is anything to search.
pub fn has_documents(db: &Db) -> Result<bool> {
    db.read(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM chunk)", [], |r| r.get(0))?))
}

/// How much is in the corpus.
pub fn counts(db: &Db) -> Result<(usize, usize)> {
    db.read(|conn| {
        Ok((
            conn.query_row("SELECT count(*) FROM document", [], |r| r.get::<_, i64>(0))? as usize,
            conn.query_row("SELECT count(*) FROM chunk", [], |r| r.get::<_, i64>(0))? as usize,
        ))
    })
}
