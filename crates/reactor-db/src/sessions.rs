//! Sessions and their append-only logs, as rows
//! ([ADR-0046](../../../docs/adr/0046-the-analysis-database-is-sqlite-inside-the-case.md)).
//!
//! This crate stores an entry as the caller's JSON plus the few columns it needs to
//! order and address it. What an entry *means* — its kinds, its text — belongs to
//! `reactor-agent`, which is why the body is opaque here.

use std::io::Write;

use rusqlite::params;

use crate::db::Db;
use crate::error::{Error, Result};
use crate::time::now_rfc3339;

/// One entry as it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: u64,
    pub parent: Option<u64>,
    /// Unix milliseconds.
    pub ts: u64,
    pub kind: String,
    /// The entry as JSON.
    pub body: String,
    pub blob_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    /// RFC 3339 UTC.
    pub created_at: String,
    pub entries: usize,
    /// The first thing the person said, if anything.
    pub first_prompt: Option<String>,
}

/// A new session and its first entry, together or not at all.
pub fn create(db: &Db, session: &str, format_version: u32, first: Row) -> Result<()> {
    let id = session.to_string();
    db.write(move |tx| {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session WHERE id = ?1)",
            [&id],
            |r| r.get(0),
        )?;
        if exists {
            return Err(Error::Sqlite(format!("session `{id}` already exists")));
        }
        tx.execute(
            "INSERT INTO session(id, format_version, created_at) VALUES (?1, ?2, ?3)",
            params![id, format_version, now_rfc3339()],
        )?;
        insert(tx, &id, &first)
    })
}

fn insert(tx: &rusqlite::Transaction, session: &str, row: &Row) -> Result<()> {
    tx.execute(
        "INSERT INTO entry(session_id, id, parent, ts, type, body, blob_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session,
            row.id as i64,
            row.parent.map(|p| p as i64),
            row.ts as i64,
            row.kind,
            row.body,
            row.blob_sha256
        ],
    )?;
    Ok(())
}

/// Append one entry. Its id must be the next one: ids are dense per session.
pub fn append(db: &Db, session: &str, row: Row) -> Result<()> {
    let session = session.to_string();
    db.write(move |tx| {
        let next: i64 = tx.query_row(
            "SELECT count(*) FROM entry WHERE session_id = ?1",
            [&session],
            |r| r.get(0),
        )?;
        if next as u64 != row.id {
            return Err(Error::Sqlite(format!(
                "session `{session}`: entry #{} cannot follow {next} entries -- is the session open elsewhere?",
                row.id
            )));
        }
        insert(tx, &session, &row)
    })
}

/// Every entry of a session, in id order.
pub fn load(db: &Db, session: &str) -> Result<Vec<Row>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, parent, ts, type, body, blob_sha256 FROM entry
             WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([session], |r| {
                Ok(Row {
                    id: r.get::<_, i64>(0)? as u64,
                    parent: r.get::<_, Option<i64>>(1)?.map(|p| p as u64),
                    ts: r.get::<_, i64>(2)? as u64,
                    kind: r.get(3)?,
                    body: r.get(4)?,
                    blob_sha256: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn exists(db: &Db, session: &str) -> Result<bool> {
    db.read(|conn| {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session WHERE id = ?1)",
            [session],
            |r| r.get(0),
        )?)
    })
}

/// The case's sessions, newest first.
pub fn list(db: &Db) -> Result<Vec<SessionSummary>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT s.id, s.title, s.created_at,
                    (SELECT count(*) FROM entry e WHERE e.session_id = s.id),
                    (SELECT json_extract(e.body, '$.text') FROM entry e
                      WHERE e.session_id = s.id AND e.type = 'user' ORDER BY e.id LIMIT 1)
             FROM session s ORDER BY s.created_at DESC, s.id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SessionSummary {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    entries: r.get::<_, i64>(3)? as usize,
                    first_prompt: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// A session as JSON lines, one entry per line in id order — what the log file used to
/// be, for `grep`, `jq` and anything else that reads text.
pub fn export_jsonl(db: &Db, session: &str, out: &mut dyn Write) -> Result<usize> {
    let rows = load(db, session)?;
    if rows.is_empty() {
        return Err(Error::Sqlite(format!("no session `{session}`")));
    }
    for row in &rows {
        writeln!(out, "{}", row.body).map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(rows.len())
}
