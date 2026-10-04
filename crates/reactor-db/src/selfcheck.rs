//! Does this binary's SQLite have everything REactor needs?
//!
//! The single-executable requirement is where the design is most likely to break per
//! platform, so the answer is asked of the build itself rather than assumed: the SQLite
//! version, that FTS5 is compiled in and round-trips, that sqlite-vec registered and
//! answers a nearest-neighbour query, and that the JSON functions exist.

use std::fmt;

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::sqlite::open_memory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub sqlite_version: String,
    pub vec_version: String,
    pub fts5: bool,
    pub json: bool,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "sqlite      {}", self.sqlite_version)?;
        writeln!(f, "sqlite-vec  {}", self.vec_version)?;
        writeln!(
            f,
            "fts5        {}",
            if self.fts5 { "ok" } else { "missing" }
        )?;
        write!(
            f,
            "json        {}",
            if self.json { "ok" } else { "missing" }
        )
    }
}

fn fts5_round_trip(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE t_fts USING fts5(text, tokenize='unicode61');
         INSERT INTO t_fts(rowid, text) VALUES (1, 'decrypt_config at 0x401000'),
                                               (2, 'unrelated line');",
    )?;
    let hit: i64 = conn.query_row(
        "SELECT rowid FROM t_fts WHERE t_fts MATCH '\"decrypt_config\"' ORDER BY bm25(t_fts)",
        [],
        |r| r.get(0),
    )?;
    if hit != 1 {
        return Err(Error::Build(format!("FTS5 returned row {hit}, expected 1")));
    }
    Ok(())
}

fn vec_round_trip(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE t_vec USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[4]);
         INSERT INTO t_vec(chunk_id, embedding) VALUES
           (1, '[1.0, 0.0, 0.0, 0.0]'),
           (2, '[0.0, 1.0, 0.0, 0.0]'),
           (3, '[0.9, 0.1, 0.0, 0.0]');",
    )?;
    let mut stmt = conn.prepare(
        "SELECT chunk_id FROM t_vec WHERE embedding MATCH '[1.0, 0.0, 0.0, 0.0]' AND k = 2
         ORDER BY distance",
    )?;
    let ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    if ids != [1, 3] {
        return Err(Error::Build(format!(
            "sqlite-vec returned neighbours {ids:?}, expected [1, 3]"
        )));
    }
    Ok(())
}

/// Run every check against a private in-memory database.
pub fn run() -> Result<Report> {
    let conn = open_memory()?;

    let sqlite_version: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;

    let vec_version: String = conn
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .map_err(|e| Error::Build(format!("sqlite-vec is not registered in this build ({e})")))?;

    let mut stmt = conn.prepare("PRAGMA compile_options")?;
    let options: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    if !options.iter().any(|o| o == "ENABLE_FTS5") {
        return Err(Error::Build(
            "this build's SQLite was compiled without FTS5".into(),
        ));
    }
    fts5_round_trip(&conn)?;
    vec_round_trip(&conn)?;

    let json: String = conn
        .query_row(
            "SELECT json_extract('{\"a\":{\"b\":7}}', '$.a.b')",
            [],
            |r| r.get::<_, i64>(0).map(|n| n.to_string()),
        )
        .map_err(|e| Error::Build(format!("SQLite's JSON functions are missing ({e})")))?;
    if json != "7" {
        return Err(Error::Build(format!(
            "json_extract returned {json}, expected 7"
        )));
    }

    Ok(Report {
        sqlite_version,
        vec_version,
        fts5: true,
        json: true,
    })
}
