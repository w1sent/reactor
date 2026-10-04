//! Embedding models and their vectors.
//!
//! One vector table per model, because a vector table has one dimension and a query must
//! be embedded by the model that embedded what it is compared with. Choosing a new model
//! adds a table and leaves the old one in place: **search keeps using the old vectors until
//! the new set is complete**, so changing the model never leaves the corpus unsearchable.

use rusqlite::params;
use serde::Serialize;

use crate::db::Db;
use crate::error::{Error, Result};
use crate::time::now_rfc3339;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub revision: String,
    pub dim: usize,
}

/// The vector table of a model.
pub(super) fn table(model: i64) -> String {
    format!("chunk_vec_{model}")
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub(super) fn blob(v: &[f32]) -> Vec<u8> {
    to_blob(v)
}

/// Make a model known, and its vector table with it. Registering it again is finding it —
/// but a model that answers with another dimension than it did is refused, because its
/// old vectors could no longer be compared with its new ones.
pub fn register_model(db: &Db, name: &str, revision: &str, dim: usize) -> Result<Model> {
    if dim == 0 || dim > 8192 {
        return Err(Error::Invalid(format!("a model with {dim} dimensions")));
    }
    let (name, revision) = (name.trim().to_string(), revision.trim().to_string());
    db.write(move |tx| {
        let existing: Option<(i64, usize)> = tx
            .query_row(
                "SELECT id, dim FROM embedding_model WHERE name = ?1 AND revision = ?2",
                params![name, revision],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)),
            )
            .ok();
        let id = match existing {
            Some((id, have)) if have == dim => id,
            Some((_, have)) => {
                return Err(Error::Invalid(format!(
                    "{name} was registered with {have} dimensions and now answers with {dim}; register the new one as another revision"
                )));
            }
            None => {
                tx.execute(
                    "INSERT INTO embedding_model(name, revision, dim, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![name, revision, dim as i64, now_rfc3339()],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {} USING vec0(
               chunk_id INTEGER PRIMARY KEY, embedding float[{dim}] distance_metric=cosine)",
            table(id)
        ))?;
        Ok(Model {
            id,
            name,
            revision,
            dim,
        })
    })
}

pub fn models(db: &Db) -> Result<Vec<Model>> {
    db.read(|conn| {
        let mut stmt =
            conn.prepare("SELECT id, name, revision, dim FROM embedding_model ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Model {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    revision: r.get(2)?,
                    dim: r.get::<_, i64>(3)? as usize,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Passages with no vector from this model yet, oldest first.
pub fn pending(db: &Db, model: i64, limit: usize) -> Result<Vec<(i64, String)>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.text FROM chunk c
              WHERE NOT EXISTS (SELECT 1 FROM embedding_status s
                                 WHERE s.chunk_id = c.id AND s.model_id = ?1)
              ORDER BY c.id LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![model, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Store vectors for passages, together with the fact that they are stored. A passage
/// that has gone since it was handed out is skipped, not an error.
pub fn store_vectors(db: &Db, model: &Model, vectors: Vec<(i64, Vec<f32>)>) -> Result<usize> {
    for (chunk, v) in &vectors {
        if v.len() != model.dim {
            return Err(Error::Invalid(format!(
                "passage {chunk}: a vector of {} for a model of {} dimensions",
                v.len(),
                model.dim
            )));
        }
    }
    let model = model.clone();
    db.write(move |tx| {
        let table = table(model.id);
        let mut stored = 0;
        let mut gone = tx.prepare("SELECT EXISTS(SELECT 1 FROM chunk WHERE id = ?1)")?;
        let mut delete = tx.prepare(&format!("DELETE FROM {table} WHERE chunk_id = ?1"))?;
        let mut insert = tx.prepare(&format!(
            "INSERT INTO {table}(chunk_id, embedding) VALUES (?1, ?2)"
        ))?;
        let mut status = tx.prepare(
            "INSERT OR REPLACE INTO embedding_status(chunk_id, model_id, embedded_at) VALUES (?1, ?2, ?3)",
        )?;
        let now = now_rfc3339();
        for (chunk, v) in &vectors {
            if !gone.query_row([chunk], |r| r.get::<_, bool>(0))? {
                continue;
            }
            delete.execute([chunk])?;
            insert.execute(params![chunk, blob(v)])?;
            status.execute(params![chunk, model.id, now])?;
            stored += 1;
        }
        Ok(stored)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub embedded: usize,
    pub total: usize,
}

impl Progress {
    pub fn complete(&self) -> bool {
        self.embedded >= self.total
    }
}

pub fn progress(db: &Db, model: i64) -> Result<Progress> {
    db.read(|conn| {
        Ok(Progress {
            embedded: conn.query_row(
                "SELECT count(*) FROM embedding_status WHERE model_id = ?1",
                [model],
                |r| r.get::<_, i64>(0),
            )? as usize,
            total: conn.query_row("SELECT count(*) FROM chunk", [], |r| r.get::<_, i64>(0))?
                as usize,
        })
    })
}

/// The model search should embed its query with: the preferred one if every passage has
/// its vector, else the newest model that is complete, else none (keyword search only).
pub fn usable_model(db: &Db, preferred: Option<&str>) -> Result<Option<Model>> {
    let mut all = models(db)?;
    all.reverse();
    // The preferred one first, then the rest newest first.
    if let Some(name) = preferred {
        all.sort_by_key(|m| m.name != name);
    }
    for m in all {
        let p = progress(db, m.id)?;
        if p.total > 0 && p.complete() {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// Drop a model's vectors and the record of which passages have one, keeping the model
/// itself: every passage is pending for it again.
pub fn clear_vectors(db: &Db, model: i64) -> Result<()> {
    db.write(move |tx| {
        tx.execute_batch(&format!("DELETE FROM {}", table(model)))?;
        tx.execute("DELETE FROM embedding_status WHERE model_id = ?1", [model])?;
        Ok(())
    })
}

/// Drop a model and all its vectors.
pub fn forget_model(db: &Db, model: i64) -> Result<()> {
    db.write(move |tx| {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {}", table(model)))?;
        tx.execute("DELETE FROM embedding_status WHERE model_id = ?1", [model])?;
        tx.execute("DELETE FROM embedding_model WHERE id = ?1", [model])?;
        Ok(())
    })
}

/// Regenerate the derived indexes from the passages: the keyword index is rebuilt, and
/// every vector is dropped so that it is made again.
pub fn rebuild_indexes(db: &Db) -> Result<()> {
    db.write(|tx| {
        tx.execute("INSERT INTO chunk_fts(chunk_fts) VALUES ('rebuild')", [])?;
        let models: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM embedding_model")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for m in models {
            tx.execute_batch(&format!("DELETE FROM {}", table(m)))?;
        }
        tx.execute("DELETE FROM embedding_status", [])?;
        Ok(())
    })
}
