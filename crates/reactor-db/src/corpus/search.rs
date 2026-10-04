//! Finding passages: by the words in them, and by what they mean, fused.
//!
//! Keywords matter because the things an analyst looks for — a function name, a hash, an
//! address — are *strings*, and meaning-based search does not find strings. Meaning
//! matters because the question is rarely in the document's words. So both run, and their
//! rankings are combined by reciprocal rank fusion, which needs no tuning and does not care
//! that the two scores are not comparable.

use std::collections::HashMap;

use rusqlite::params;
use serde::Serialize;

use super::vectors::{self, Model};
use crate::db::Db;
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub chunk_id: i64,
    pub document_id: String,
    pub source_uri: String,
    pub title: Option<String>,
    pub heading_path: String,
    pub text: String,
    pub origin: String,
    /// `file`, `url` or `wiki`, and the document's own metadata: what a filter asks about.
    pub source_kind: String,
    pub mime: Option<String>,
    pub meta: serde_json::Value,
    /// The fused score; larger is better.
    pub score: f64,
    /// Where each ranking put it (1 is first), if it was in it at all.
    pub keyword_rank: Option<usize>,
    pub meaning_rank: Option<usize>,
    /// How close its meaning is to the query's: the cosine similarity, 1 being identical,
    /// if the meaning ranking had it. Larger is closer. (The score above is made of ranks,
    /// not of this.)
    pub similarity: Option<f32>,
}

/// How a search is run, beyond what it is for.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Look the words up. False: by meaning alone.
    pub keywords: bool,
    /// A passage whose meaning is less similar than this (cosine, 0 to 1) is not a match by
    /// meaning. 0 asks for no threshold.
    pub min_similarity: f32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            keywords: true,
            min_similarity: 0.0,
        }
    }
}

/// What to look for.
pub struct Query<'a> {
    pub text: &'a str,
    /// The query as the model embedded it, with that model. Absent: keywords only.
    pub vector: Option<(&'a Model, &'a [f32])>,
    /// How many to return.
    pub limit: usize,
}

/// A passage's fused score, and where each ranking put it.
type Scored = (f64, Option<usize>, Option<usize>);

/// The RRF constant: how quickly a lower rank stops mattering.
const K: f64 = 60.0;

/// A user's words as an FTS5 query: each word quoted, so that nothing in it — a hyphen, a
/// colon, `AND`, a quote — is read as syntax, and joined with OR so that a question is
/// ranked by how many of its words a passage has.
pub fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

fn keyword_ids(db: &Db, text: &str, k: usize) -> Result<Vec<i64>> {
    let Some(q) = fts_query(text) else {
        return Ok(Vec::new());
    };
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT rowid FROM chunk_fts WHERE chunk_fts MATCH ?1
              ORDER BY bm25(chunk_fts, 1.0, 1.5) LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![q, k as i64], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// `ids` without the passages that have no words of their own, order kept. (Passages stored
/// before such passages were joined to their neighbours can still be hollow.)
fn with_words(db: &Db, ids: Vec<i64>) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(ids);
    }
    let hollow: std::collections::HashSet<i64> = db.read(|conn| {
        let marks = vec!["?"; ids.len()].join(",");
        let mut stmt =
            conn.prepare(&format!("SELECT id, text FROM chunk WHERE id IN ({marks})"))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            let (id, text) = row?;
            if super::is_hollow(&text) {
                out.insert(id);
            }
        }
        Ok(out)
    })?;
    Ok(ids.into_iter().filter(|id| !hollow.contains(id)).collect())
}

/// The nearest passages to a vector, nearest first, with their similarity (cosine distance
/// from the index, turned round so that larger is closer).
fn meaning_ids(db: &Db, model: &Model, v: &[f32], k: usize) -> Result<Vec<(i64, f32)>> {
    if v.len() != model.dim {
        return Err(crate::error::Error::Invalid(format!(
            "a query vector of {} for a model of {} dimensions",
            v.len(),
            model.dim
        )));
    }
    let blob = vectors::blob(v);
    db.read(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT chunk_id, distance FROM {} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
            vectors::table(model.id)
        ))?;
        let rows = stmt
            .query_map(params![blob, k as i64], |r| {
                Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Search the corpus: by keyword, and by meaning when the query has a vector, fused.
pub fn search(db: &Db, q: &Query) -> Result<Vec<Hit>> {
    search_with(db, q, &Options::default())
}

/// As [`search`], but with `keywords` false the words are not looked up: the ranking is by
/// meaning alone (it is empty without a vector).
pub fn search_ranked(db: &Db, q: &Query, keywords: bool) -> Result<Vec<Hit>> {
    search_with(
        db,
        q,
        &Options {
            keywords,
            ..Options::default()
        },
    )
}

/// [`search`] with its options: whether to use the words, and how close in meaning a passage
/// has to be.
pub fn search_with(db: &Db, q: &Query, options: &Options) -> Result<Vec<Hit>> {
    let keywords = options.keywords;
    let limit = q.limit.max(1);
    // Each ranking is cut deeper than the answer, so that something both put fairly high
    // can beat something only one put first.
    let depth = (limit * 4).max(40);
    let keyword = if keywords {
        keyword_ids(db, q.text, depth)?
    } else {
        Vec::new()
    };
    let nearest = match q.vector {
        Some((model, v)) => meaning_ids(db, model, v, depth)?,
        None => Vec::new(),
    };
    // Too far in meaning is not a match, however it ranks among the rest.
    let nearest: Vec<(i64, f32)> = nearest
        .into_iter()
        .filter(|(_, sim)| options.min_similarity <= 0.0 || *sim >= options.min_similarity)
        .collect();
    let similarity: HashMap<i64, f32> = nearest.iter().copied().collect();
    let meaning: Vec<i64> = nearest.into_iter().map(|(id, _)| id).collect();
    // A passage with nothing in it is never an answer, whichever ranking liked it.
    let (keyword, meaning) = (with_words(db, keyword)?, with_words(db, meaning)?);
    let mut score: HashMap<i64, Scored> = HashMap::new();
    for (i, id) in keyword.iter().enumerate() {
        let e = score.entry(*id).or_default();
        e.0 += 1.0 / (K + (i + 1) as f64);
        e.1 = Some(i + 1);
    }
    for (i, id) in meaning.iter().enumerate() {
        let e = score.entry(*id).or_default();
        e.0 += 1.0 / (K + (i + 1) as f64);
        e.2 = Some(i + 1);
    }
    let mut ranked: Vec<(i64, Scored)> = score.into_iter().collect();
    // Ties keep a stable order: the earlier passage first.
    ranked.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.0.cmp(&b.0)));
    ranked.truncate(limit);
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.document_id, d.source_uri, d.title, c.heading_path, c.text, d.origin,
                    d.source_kind, d.mime, d.meta
               FROM chunk c JOIN document d ON d.id = c.document_id WHERE c.id = ?1",
        )?;
        let mut hits = Vec::new();
        for (id, (score, keyword_rank, meaning_rank)) in ranked {
            let hit = stmt.query_row([id], |r| {
                Ok(Hit {
                    chunk_id: id,
                    document_id: r.get(0)?,
                    source_uri: r.get(1)?,
                    title: r.get(2)?,
                    heading_path: r.get(3)?,
                    text: r.get(4)?,
                    origin: r.get(5)?,
                    source_kind: r.get(6)?,
                    mime: r.get(7)?,
                    meta: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
                    score,
                    keyword_rank,
                    meaning_rank,
                    similarity: similarity.get(&id).copied(),
                })
            });
            // A passage removed between the ranking and now is not a hit.
            if let Ok(h) = hit {
                hits.push(h);
            }
        }
        Ok(hits)
    })
}

/// The passages of a document under a section, as hits with no ranking: how a passage that
/// was handed over as text (a tool's result) is found again in the corpus.
pub fn hits_at(db: &Db, source_uri: &str, heading_path: &str) -> Result<Vec<Hit>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.document_id, d.source_uri, d.title, c.heading_path, c.text, d.origin,
                    d.source_kind, d.mime, d.meta
               FROM chunk c JOIN document d ON d.id = c.document_id
              WHERE d.source_uri = ?1 AND c.heading_path = ?2 ORDER BY c.ord",
        )?;
        let rows = stmt
            .query_map(params![source_uri, heading_path], |r| {
                Ok(Hit {
                    chunk_id: r.get(0)?,
                    document_id: r.get(1)?,
                    source_uri: r.get(2)?,
                    title: r.get(3)?,
                    heading_path: r.get(4)?,
                    text: r.get(5)?,
                    origin: r.get(6)?,
                    source_kind: r.get(7)?,
                    mime: r.get(8)?,
                    meta: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
                    score: 0.0,
                    keyword_rank: None,
                    meaning_rank: None,
                    similarity: None,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}
