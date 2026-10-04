//! Looking into any SQLite file: its tables, and their rows as text.
//!
//! For the table viewer: the agent's `reactor_data.sqlite`, a tool's database a pull reads
//! from, the case's own `analysis.db`. The file is opened **read-only and never changed** —
//! its journal mode is not touched, and nothing is written — and with sqlite-vec available,
//! so a case database's vector tables read too.

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use crate::error::{Error, Result};
use crate::sqlite::register_extensions;

/// The first bytes of every SQLite file.
pub const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// More rows than this are not read; the caller is told the rest was left out.
pub const MAX_ROWS: usize = 100_000;

/// Whether these leading bytes are those of a SQLite database.
pub fn is_sqlite(head: &[u8]) -> bool {
    head.starts_with(MAGIC)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    /// A view has no rows of its own and is not counted.
    pub is_view: bool,
    /// Rows, counted up to a limit; see [`TableInfo::rows_label`].
    pub rows: Option<u64>,
}

/// How far a row count is counted, so a huge table does not stall the listing.
const COUNT_UP_TO: u64 = 1_000_000;

impl TableInfo {
    pub fn rows_label(&self) -> String {
        match self.rows {
            Some(n) if n > COUNT_UP_TO => format!("{COUNT_UP_TO}+ rows"),
            Some(1) => "1 row".to_string(),
            Some(n) => format!("{n} rows"),
            None => "view".to_string(),
        }
    }
}

/// The rows of one table, as text, the column names first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<Vec<String>>,
    /// The table has more rows than were read.
    pub truncated: bool,
}

fn open(path: &Path) -> Result<Connection> {
    register_extensions();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| Error::Sqlite(format!("{}: {e}", path.display())))?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(conn)
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// A cell as the viewer shows it. A NULL is not an empty string, so it says so; bytes are
/// shown as a hex preview with their length.
fn text(v: ValueRef) -> String {
    match v {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(b) => {
            let shown: String = b.iter().take(16).map(|x| format!("{x:02x}")).collect();
            if b.len() > 16 {
                format!("x'{shown}…' ({} bytes)", b.len())
            } else {
                format!("x'{shown}'")
            }
        }
    }
}

/// The tables and views of a database, by name; SQLite's own and a virtual table's
/// shadow tables left out.
pub fn tables(path: &Path) -> Result<Vec<TableInfo>> {
    let conn = open(path)?;
    let mut stmt = conn.prepare(
        "SELECT name, type, sql FROM sqlite_master
          WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
          ORDER BY name",
    )?;
    let listed: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    // The tables that exist only to back a virtual table (`chunk_fts_data`, `chunk_vec_1_rowids`)
    // are named after it; a person looks at the virtual one.
    let virtuals: Vec<&str> = listed
        .iter()
        .filter(|(_, _, sql)| {
            sql.as_deref()
                .is_some_and(|s| s.to_ascii_uppercase().contains("VIRTUAL TABLE"))
        })
        .map(|(n, _, _)| n.as_str())
        .collect();
    let mut out = Vec::new();
    for (name, kind, _) in &listed {
        if virtuals
            .iter()
            .any(|v| name.len() > v.len() + 1 && name.starts_with(&format!("{v}_")))
        {
            continue;
        }
        let is_view = kind == "view";
        let rows = if is_view {
            None
        } else {
            conn.query_row(
                &format!(
                    "SELECT count(*) FROM (SELECT 1 FROM {} LIMIT {})",
                    quote(name),
                    COUNT_UP_TO + 1
                ),
                [],
                |r| r.get::<_, i64>(0),
            )
            .ok()
            .map(|n| n as u64)
        };
        out.push(TableInfo {
            name: name.clone(),
            is_view,
            rows,
        });
    }
    Ok(out)
}

/// Read a table: its column names, then up to `limit` rows in rowid order where it has
/// one.
pub fn read_table(path: &Path, table: &str, limit: usize) -> Result<Rows> {
    let conn = open(path)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT * FROM {} LIMIT {}",
            quote(table),
            limit + 1
        ))
        .map_err(|e| Error::Sqlite(format!("{table}: {e}")))?;
    let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let mut rows = vec![columns.clone()];
    let mut query = stmt.query([])?;
    let mut truncated = false;
    while let Some(row) = query.next()? {
        if rows.len() > limit {
            truncated = true;
            break;
        }
        rows.push(
            (0..columns.len())
                .map(|i| row.get_ref(i).map(text))
                .collect::<std::result::Result<_, _>>()?,
        );
    }
    Ok(Rows { rows, truncated })
}
