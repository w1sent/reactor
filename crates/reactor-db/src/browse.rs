//! Looking into any SQLite file: its tables, their rows, and queries over them.
//!
//! For the table viewer ([ADR-0050]): the agent's `reactor_data.sqlite`, a tool's database
//! a pull reads from, the case's own `analysis.db`, a database taken from a machine. The
//! file is **never changed**, and nothing is created beside it:
//!
//! - It is opened read-only, with `query_only` on, and with sqlite-vec available, so a case
//!   database's vector tables read too.
//! - A query runs behind an authorizer that allows reading and nothing else.
//! - The open mode follows the file's WAL state ([`open_mode`]), because an ordinary
//!   read-only open of a WAL file in a writable folder leaves `-shm` and `-wal` behind.
//!
//! [`Database`] is the open handle. [`tables`] and [`read_table`] are the older one-call
//! forms over it.
//!
//! [ADR-0050]: ../../../docs/adr/0050-the-table-viewer-is-a-grid-over-sources.md

use std::ffi::OsString;
use std::io::Read;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use rusqlite::InterruptHandle;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags};

use crate::error::{Error, Result};
use crate::sqlite::register_extensions;

/// The first bytes of every SQLite file.
pub const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// [`read_table`] callers that do not stream read at most this many rows.
pub const MAX_ROWS: usize = 100_000;

/// How far a row count is counted, so a huge table does not stall the listing.
const COUNT_UP_TO: u64 = 1_000_000;

/// Whether these leading bytes are those of a SQLite database.
pub fn is_sqlite(head: &[u8]) -> bool {
    head.starts_with(MAGIC)
}

// ---------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------

/// One cell, with the type SQLite stored it as.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    fn from_ref(v: ValueRef<'_>) -> Value {
        match v {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(i) => Value::Integer(i),
            ValueRef::Real(f) => Value::Real(f),
            ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => Value::Blob(b.to_vec()),
        }
    }

    /// The cell as a viewer shows it. A NULL is not an empty string, so it says so; bytes
    /// are a hex preview with their length.
    pub fn display(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Integer(i) => i.to_string(),
            Value::Real(f) => f.to_string(),
            Value::Text(t) => t.clone(),
            Value::Blob(b) => {
                let shown: String = b.iter().take(16).map(|x| format!("{x:02x}")).collect();
                if b.len() > 16 {
                    format!("x'{shown}…' ({} bytes)", b.len())
                } else {
                    format!("x'{shown}'")
                }
            }
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// About what the cell costs to hold, for the budget.
    fn weight(&self) -> usize {
        const CELL: usize = std::mem::size_of::<Value>();
        CELL + match self {
            Value::Text(t) => t.len(),
            Value::Blob(b) => b.len(),
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------------------

/// How a file was opened, which depends on its journal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// `mode=ro`, as SQLite's own read-only open.
    ReadOnly,
    /// `immutable=1`: the file is assumed not to change, and no sidecar is touched. Chosen
    /// for a WAL-mode file that has no frames to replay.
    Immutable,
    /// Read from a temporary copy of the database and its `-wal`, because reading the
    /// original would create an `-shm` beside it.
    Copy,
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Whether the file's header says it is in WAL mode (bytes 18 and 19 are 2).
fn is_wal(path: &Path) -> Result<bool> {
    let mut head = [0u8; 20];
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut read = 0;
    while read < head.len() {
        match file
            .read(&mut head[read..])
            .map_err(|e| Error::io(path, e))?
        {
            0 => break,
            n => read += n,
        }
    }
    Ok(read == head.len() && (head[18] == 2 || head[19] == 2))
}

/// The mode this file is opened in:
///
/// - not WAL: [`OpenMode::ReadOnly`];
/// - WAL with an `-shm` beside it (in use, or its sidecars are already there):
///   [`OpenMode::ReadOnly`], which creates nothing;
/// - WAL with no `-shm` and no frames in the `-wal`: [`OpenMode::Immutable`];
/// - WAL with no `-shm` and frames in the `-wal`: [`OpenMode::Copy`].
pub fn open_mode(path: &Path) -> Result<OpenMode> {
    if !is_wal(path)? {
        return Ok(OpenMode::ReadOnly);
    }
    if sidecar(path, "-shm").exists() {
        return Ok(OpenMode::ReadOnly);
    }
    let frames = std::fs::metadata(sidecar(path, "-wal")).is_ok_and(|m| m.len() > 0);
    Ok(if frames {
        OpenMode::Copy
    } else {
        OpenMode::Immutable
    })
}

/// `file:<path>?<query>`, with the path percent-encoded.
fn file_uri(path: &Path, query: &str) -> String {
    #[cfg(unix)]
    let bytes: Vec<u8> = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes: Vec<u8> = path.to_string_lossy().replace('\\', "/").into_bytes();
    let mut out = String::from("file:");
    // A Windows drive path needs the slash a URI path starts with.
    if !bytes.starts_with(b"/") && cfg!(not(unix)) {
        out.push('/');
    }
    for b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out.push('?');
    out.push_str(query);
    out
}

/// An open SQLite file, for looking.
pub struct Database {
    // Declared before the copy, so the connection closes before the directory goes.
    conn: Connection,
    mode: OpenMode,
    /// What `conn` was opened on: the file, or its copy.
    target: PathBuf,
    /// The copy, shared by every connection opened from it, so it goes with the last.
    _copy: Option<Arc<tempfile::TempDir>>,
}

impl Database {
    /// Open `path` for looking, in the mode its journal state allows.
    pub fn open(path: &Path) -> Result<Database> {
        register_extensions();
        let mode = open_mode(path)?;
        match Database::open_as(path, mode) {
            Ok(db) => Ok(db),
            // A WAL file whose sidecars cannot be used where it is (read-only media, a
            // stale `-shm`) reads from a copy instead.
            Err(first) if mode == OpenMode::ReadOnly && is_wal(path).unwrap_or(false) => {
                Database::open_as(path, OpenMode::Copy).map_err(|_| first)
            }
            Err(e) => Err(e),
        }
    }

    fn open_as(path: &Path, mode: OpenMode) -> Result<Database> {
        let copy = match mode {
            OpenMode::Copy => Some(Arc::new(copy_for_reading(path)?)),
            _ => None,
        };
        let target = match &copy {
            Some(dir) => dir.path().join(path.file_name().unwrap_or_default()),
            None => path.to_path_buf(),
        };
        Database::connect(path, target, mode, copy)
    }

    /// One connection on `target`. `named` is what an error calls the file.
    fn connect(
        named: &Path,
        target: PathBuf,
        mode: OpenMode,
        copy: Option<Arc<tempfile::TempDir>>,
    ) -> Result<Database> {
        let named = |e: rusqlite::Error| Error::Sqlite(format!("{}: {e}", named.display()));
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = match mode {
            OpenMode::Immutable => Connection::open_with_flags(
                file_uri(&target, "immutable=1"),
                flags | OpenFlags::SQLITE_OPEN_URI,
            ),
            _ => Connection::open_with_flags(&target, flags),
        }
        .map_err(named)?;
        conn.pragma_update(None, "query_only", "ON")
            .map_err(named)?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(named)?;
        // Reading the schema is what fails when the file is not a database, or its
        // sidecars cannot be used.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(named)?;
        Ok(Database {
            conn,
            mode,
            target,
            _copy: copy,
        })
    }

    /// Another connection on the same file, in the same mode, sharing the copy if there is
    /// one. A query that runs for a while gets its own, so browsing is not held up behind it.
    pub fn reopen(&self) -> Result<Database> {
        Database::connect(
            &self.target,
            self.target.clone(),
            self.mode,
            self._copy.clone(),
        )
    }

    pub fn mode(&self) -> OpenMode {
        self.mode
    }

    /// A handle another thread can use to stop a running statement.
    pub fn interrupt_handle(&self) -> InterruptHandle {
        self.conn.get_interrupt_handle()
    }
}

/// The database and its `-wal`, in a directory of their own.
fn copy_for_reading(path: &Path) -> Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new()
        .prefix("reactor-sqlite-")
        .tempdir()
        .map_err(|e| Error::Io(format!("a place to read a copy of {}: {e}", path.display())))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid(format!("{} has no file name", path.display())))?;
    std::fs::copy(path, dir.path().join(name)).map_err(|e| Error::io(path, e))?;
    let wal = sidecar(path, "-wal");
    if wal.exists() {
        let to = sidecar(&dir.path().join(name), "-wal");
        std::fs::copy(&wal, to).map_err(|e| Error::io(&wal, e))?;
    }
    Ok(dir)
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

// ---------------------------------------------------------------------------------------
// Tables and their shape
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    /// A view has no rows of its own and is not counted.
    pub is_view: bool,
    /// Rows, counted up to a limit; see [`TableInfo::rows_label`].
    pub rows: Option<u64>,
}

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

/// A row count that may have stopped counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub rows: u64,
    /// `false`: there are at least this many.
    pub exact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    /// As declared; empty for an expression or an untyped column.
    pub declared_type: String,
    pub not_null: bool,
    /// 1-based position in the primary key, 0 when it is not part of it.
    pub primary_key: u32,
    /// A hidden column of a virtual table: not in `SELECT *`.
    pub hidden: bool,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub from: String,
    pub table: String,
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchema {
    pub name: String,
    pub is_view: bool,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKey>,
    /// The `CREATE` statement, as stored.
    pub sql: Option<String>,
}

impl Database {
    /// The tables and views of a database, by name; SQLite's own and a virtual table's
    /// shadow tables left out.
    pub fn tables(&self) -> Result<Vec<TableInfo>> {
        let conn = &self.conn;
        let mut stmt = conn.prepare(
            "SELECT name, type, sql FROM sqlite_master
              WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
              ORDER BY name",
        )?;
        let listed: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        // The tables that exist only to back a virtual table (`chunk_fts_data`,
        // `chunk_vec_1_rowids`) are named after it; a person looks at the virtual one.
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

    /// A table's or view's columns, indexes, foreign keys and `CREATE` statement.
    pub fn schema(&self, table: &str) -> Result<TableSchema> {
        let conn = &self.conn;
        let (kind, sql): (String, Option<String>) = conn
            .query_row(
                "SELECT type, sql FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?1",
                [table],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    Error::Invalid(format!("no table or view named {table}"))
                }
                other => other.into(),
            })?;

        let mut columns = Vec::new();
        let mut stmt = conn.prepare(&format!("PRAGMA table_xinfo({})", quote(table)))?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            columns.push(ColumnInfo {
                name: r.get("name")?,
                declared_type: r.get::<_, Option<String>>("type")?.unwrap_or_default(),
                not_null: r.get::<_, i64>("notnull")? != 0,
                primary_key: r.get::<_, i64>("pk")?.max(0) as u32,
                hidden: r.get::<_, i64>("hidden")? == 1,
                default: r.get("dflt_value")?,
            });
        }
        drop(rows);
        drop(stmt);

        let mut indexes = Vec::new();
        if kind == "table" {
            let mut stmt = conn.prepare(&format!("PRAGMA index_list({})", quote(table)))?;
            let listed: Vec<(String, bool)> = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>("name")?, r.get::<_, i64>("unique")? != 0))
                })?
                .collect::<std::result::Result<_, _>>()?;
            for (name, unique) in listed {
                let mut stmt = conn.prepare(&format!("PRAGMA index_info({})", quote(&name)))?;
                let columns = stmt
                    .query_map([], |r| r.get::<_, Option<String>>("name"))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_iter()
                    // An expression in an index has no column name.
                    .map(|c| c.unwrap_or_else(|| "<expression>".to_string()))
                    .collect();
                indexes.push(IndexInfo {
                    name,
                    unique,
                    columns,
                });
            }
        }

        let mut foreign_keys = Vec::new();
        if kind == "table" {
            let mut stmt = conn.prepare(&format!("PRAGMA foreign_key_list({})", quote(table)))?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                foreign_keys.push(ForeignKey {
                    from: r.get("from")?,
                    table: r.get("table")?,
                    to: r.get("to")?,
                });
            }
        }

        Ok(TableSchema {
            name: table.to_string(),
            is_view: kind == "view",
            columns,
            indexes,
            foreign_keys,
            sql,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Browsing a table: a window of rows, ordered and filtered by SQL
// ---------------------------------------------------------------------------------------

/// Which rows of a table or view to fetch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowSpec {
    /// Rows where any column, read as text, contains this (case-insensitively for ASCII).
    /// A NULL never matches. Empty: every row.
    pub filter: String,
    /// A column name, and whether ascending. NULLs are last either way round.
    pub sort: Option<(String, bool)>,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// A `LIKE` pattern that contains `filter` literally.
fn like_pattern(filter: &str) -> String {
    let mut out = String::from("%");
    for c in filter.trim().chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// ` WHERE …`, and the pattern it binds as `?1`; both empty when there is no filter.
fn where_clause(columns: &[String], filter: &str) -> (String, Option<String>) {
    if filter.trim().is_empty() || columns.is_empty() {
        return (String::new(), None);
    }
    let terms: Vec<String> = columns
        .iter()
        .map(|c| format!("CAST({} AS TEXT) LIKE ?1 ESCAPE '\\'", quote(c)))
        .collect();
    (
        format!(" WHERE {}", terms.join(" OR ")),
        Some(like_pattern(filter)),
    )
}

impl Database {
    /// The names `SELECT *` gives.
    fn visible_columns(&self, table: &str) -> Result<Vec<String>> {
        Ok(self
            .schema(table)?
            .columns
            .into_iter()
            .filter(|c| !c.hidden)
            .map(|c| c.name)
            .collect())
    }

    /// Rows of a table or view, `spec.offset` onwards, in `spec`'s order. Only what is
    /// asked for is read, so a large table costs what a window of it does.
    pub fn window(&self, table: &str, spec: &WindowSpec) -> Result<Page> {
        let columns = self.visible_columns(table)?;
        let (where_sql, pattern) = where_clause(&columns, &spec.filter);
        let order = match &spec.sort {
            None => String::new(),
            Some((name, ascending)) => {
                if !columns.iter().any(|c| c == name) {
                    return Err(Error::Invalid(format!("{table} has no column {name}")));
                }
                format!(
                    " ORDER BY {q} IS NULL, {q} {dir}",
                    q = quote(name),
                    dir = if *ascending { "ASC" } else { "DESC" }
                )
            }
        };
        let (limit_at, offset_at) = if pattern.is_some() { (2, 3) } else { (1, 2) };
        let sql = format!(
            "SELECT * FROM {}{where_sql}{order} LIMIT ?{limit_at} OFFSET ?{offset_at}",
            quote(table)
        );
        let clamp = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        let mut params: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(p) = pattern {
            params.push(p.into());
        }
        params.push(clamp(spec.limit).into());
        params.push(clamp(spec.offset).into());

        let mut stmt = self.conn.prepare(&sql)?;
        let names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let mut out = Vec::new();
        let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            out.push(
                (0..names.len())
                    .map(|i| row.get_ref(i).map(Value::from_ref))
                    .collect::<std::result::Result<_, _>>()?,
            );
        }
        Ok(Page {
            columns: names,
            rows: out,
        })
    }

    /// How many rows match `filter` (all of them when it is empty), counted up to a limit.
    pub fn count(&self, table: &str, filter: &str) -> Result<Count> {
        let columns = self.visible_columns(table)?;
        let (where_sql, pattern) = where_clause(&columns, filter);
        let sql = format!(
            "SELECT count(*) FROM (SELECT 1 FROM {}{where_sql} LIMIT {})",
            quote(table),
            COUNT_UP_TO + 1
        );
        let n: i64 = match pattern {
            Some(p) => self.conn.query_row(&sql, [p], |r| r.get(0))?,
            None => self.conn.query_row(&sql, [], |r| r.get(0))?,
        };
        let rows = n.max(0) as u64;
        Ok(Count {
            rows: rows.min(COUNT_UP_TO),
            exact: rows <= COUNT_UP_TO,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Running a query: a cursor, in chunks, within a budget
// ---------------------------------------------------------------------------------------

/// What a query may hold before it is stopped, and how it hands rows over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Bytes of cells held by one run. The row that crosses it is kept.
    pub bytes: usize,
    /// Rows in a chunk.
    pub chunk_rows: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            bytes: 256 << 20,
            chunk_rows: 2_000,
        }
    }
}

/// What a running query hands over: its columns once, then its rows in chunks.
#[derive(Debug)]
pub enum Event {
    Columns(Vec<String>),
    Rows(Vec<Vec<Value>>),
}

/// Why a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The statement ran to its end.
    Finished,
    /// The rows held reached the [`Budget`].
    Budget,
    /// Interrupted through [`Database::interrupt_handle`], or the receiver said stop.
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    /// Rows handed over.
    pub rows: u64,
    /// What they weighed against the budget.
    pub bytes: usize,
    pub stop: Stop,
    pub elapsed: Duration,
}

/// Why a statement did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError {
    /// SQLite's message, without the statement repeated.
    pub message: String,
    /// Where in the statement it went wrong, in bytes, when SQLite says.
    pub offset: Option<usize>,
    /// The statement would have written, attached or changed settings: it is refused
    /// because this view only reads.
    pub refused: bool,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.refused {
            write!(f, "this view only reads ({})", self.message)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for QueryError {}

impl From<QueryError> for Error {
    fn from(e: QueryError) -> Error {
        if e.refused {
            Error::Invalid(e.to_string())
        } else {
            Error::Sqlite(e.message)
        }
    }
}

fn query_error(e: rusqlite::Error) -> QueryError {
    let denied = |code: ErrorCode| code == ErrorCode::AuthorizationForStatementDenied;
    match e {
        rusqlite::Error::SqlInputError {
            error, msg, offset, ..
        } => QueryError {
            message: msg,
            offset: usize::try_from(offset).ok(),
            refused: denied(error.code),
        },
        rusqlite::Error::SqliteFailure(err, detail) => QueryError {
            message: detail.unwrap_or_else(|| err.to_string()),
            offset: None,
            refused: denied(err.code),
        },
        rusqlite::Error::MultipleStatement => QueryError {
            message: "only one statement can be run at a time".to_string(),
            offset: None,
            refused: false,
        },
        other => QueryError {
            message: other.to_string(),
            offset: None,
            refused: false,
        },
    }
}

fn plain(message: impl Into<String>) -> QueryError {
    QueryError {
        message: message.into(),
        offset: None,
        refused: false,
    }
}

/// What a query is allowed to do: read.
fn reads_only(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Function { .. }
        | AuthAction::Recursive => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

/// The connection reads only while this is alive.
struct Locked<'a>(&'a Connection);

impl<'a> Locked<'a> {
    fn new(conn: &'a Connection) -> std::result::Result<Self, rusqlite::Error> {
        conn.authorizer(Some(reads_only))?;
        Ok(Locked(conn))
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    }
}

fn interrupted(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(err, _) if err.code == ErrorCode::OperationInterrupted)
}

impl Database {
    /// Run one SELECT. `on` gets the column names, then the rows in chunks of
    /// `budget.chunk_rows`, and answers [`ControlFlow::Break`] to stop. The statement ends
    /// when it is done, when `on` breaks, when an interrupt arrives, or when the rows held
    /// reach `budget.bytes`.
    ///
    /// Rows that were read before an error are handed over before it is returned.
    pub fn run(
        &self,
        sql: &str,
        budget: &Budget,
        mut on: impl FnMut(Event) -> ControlFlow<()>,
    ) -> std::result::Result<Finished, QueryError> {
        let started = Instant::now();
        let _locked = Locked::new(&self.conn).map_err(query_error)?;
        let mut stmt = self.conn.prepare(sql).map_err(query_error)?;
        if stmt.column_count() == 0 {
            return Err(plain("this statement returns no rows"));
        }
        let names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let width = names.len();
        let finish = |rows: u64, bytes: usize, stop: Stop| Finished {
            rows,
            bytes,
            stop,
            elapsed: started.elapsed(),
        };
        if on(Event::Columns(names)).is_break() {
            return Ok(finish(0, 0, Stop::Stopped));
        }

        let chunk_rows = budget.chunk_rows.max(1);
        let mut rows = stmt.query([]).map_err(query_error)?;
        let mut chunk: Vec<Vec<Value>> = Vec::with_capacity(chunk_rows.min(4096));
        let (mut total, mut bytes) = (0u64, 0usize);
        let mut stop = Stop::Finished;
        let mut failed = None;
        loop {
            match rows.next() {
                Ok(Some(row)) => {
                    let mut cells = Vec::with_capacity(width);
                    let mut weight = 0;
                    for i in 0..width {
                        match row.get_ref(i) {
                            Ok(v) => {
                                let v = Value::from_ref(v);
                                weight += v.weight();
                                cells.push(v);
                            }
                            Err(e) => {
                                failed = Some(e);
                                break;
                            }
                        }
                    }
                    if failed.is_some() {
                        break;
                    }
                    bytes += weight;
                    total += 1;
                    chunk.push(cells);
                    if bytes >= budget.bytes {
                        stop = Stop::Budget;
                        break;
                    }
                    if chunk.len() >= chunk_rows
                        && on(Event::Rows(std::mem::take(&mut chunk))).is_break()
                    {
                        stop = Stop::Stopped;
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) if interrupted(&e) => {
                    stop = Stop::Stopped;
                    break;
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if !chunk.is_empty() {
            let _ = on(Event::Rows(chunk));
        }
        match failed {
            Some(e) => Err(query_error(e)),
            None => Ok(finish(total, bytes, stop)),
        }
    }
}

/// `sql` ordered by its result column `position` (1-based), so the order covers every
/// row the statement produces and not just those that were read. The position, not a
/// name, because a result may have two columns of the same name.
pub fn ordered_by(sql: &str, position: usize, ascending: bool) -> String {
    let inner = sql.trim().trim_end_matches(';').trim_end();
    // On its own lines, so a trailing `--` comment cannot swallow the parenthesis.
    format!(
        "SELECT * FROM (\n{inner}\n) ORDER BY {position} {}",
        if ascending { "ASC" } else { "DESC" }
    )
}

// ---------------------------------------------------------------------------------------
// A delimited file's rows, to query
// ---------------------------------------------------------------------------------------

/// What a column of a delimited file is read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    Real,
    Text,
}

impl ColumnType {
    /// The declared type in the table.
    pub fn sql(self) -> &'static str {
        match self {
            ColumnType::Integer => "INTEGER",
            ColumnType::Real => "REAL",
            ColumnType::Text => "TEXT",
        }
    }
}

/// An integer that reads back exactly as written: `007`, `+5` and ` 5` are text.
fn is_integer_text(cell: &str) -> bool {
    cell.parse::<i64>().is_ok_and(|n| n.to_string() == cell)
}

/// A plain decimal or exponent number: `-0.5`, `12.50`, `1e3`. No sign other than `-`, no
/// leading zeros, no spaces, no `nan` or `inf`.
fn is_real_text(cell: &str) -> bool {
    let rest = cell.strip_prefix('-').unwrap_or(cell);
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (rest, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (mantissa, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let whole_ok = whole == "0" || (digits(whole) && !whole.starts_with('0'));
    let fraction_ok = fraction.is_none_or(digits);
    let exponent_ok = exponent.is_none_or(|e| {
        digits(e.trim_start_matches(['+', '-']))
            && e.len() - e.trim_start_matches(['+', '-']).len() <= 1
    });
    // A number with neither a fraction nor an exponent is an integer, handled apart.
    whole_ok
        && fraction_ok
        && exponent_ok
        && (fraction.is_some() || exponent.is_some())
        && cell.parse::<f64>().is_ok_and(f64::is_finite)
}

/// The type a column's cells allow: integer if every non-empty one is, real if every one is a
/// number, otherwise text.
pub fn infer_type<'a>(cells: impl Iterator<Item = &'a str>) -> ColumnType {
    let (mut any, mut all_integer, mut all_number) = (false, true, true);
    for cell in cells.filter(|c| !c.is_empty()) {
        any = true;
        let integer = is_integer_text(cell);
        all_integer &= integer;
        all_number &= integer || is_real_text(cell);
        if !all_number {
            break;
        }
    }
    match (any, all_integer, all_number) {
        (false, _, _) | (_, _, false) => ColumnType::Text,
        (_, true, _) => ColumnType::Integer,
        _ => ColumnType::Real,
    }
}

/// Names a table can have: a repeat gets `_2`, `_3`, … so no column is lost.
fn unique_columns(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let mut candidate = name.clone();
        let mut n = 1;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&candidate)) {
            n += 1;
            candidate = format!("{name}_{n}");
        }
        out.push(candidate);
    }
    out
}

/// A delimited file's rows in a database of their own, and what they became.
pub struct TextTable {
    pub db: Database,
    pub table: String,
    /// The column names as the table has them.
    pub columns: Vec<String>,
    pub types: Vec<ColumnType>,
    pub rows: u64,
}

/// Put `rows` in a table `table` with `columns`, in a temporary database file that goes with
/// the returned [`Database`] and every connection reopened from it, so a sheet can be asked
/// questions in SQL ([ADR-0050]).
///
/// A column is `INTEGER` or `REAL` only when every non-empty cell reads that way exactly, so
/// an identifier like `007` stays text; an empty cell is NULL in such a column and an empty
/// string in a text one. A short row is padded, and a long one cut to `columns`.
///
/// [ADR-0050]: ../../../docs/adr/0050-the-table-viewer-is-a-grid-over-sources.md
pub fn load_text_table(table: &str, columns: &[String], rows: &[Vec<String>]) -> Result<TextTable> {
    if columns.is_empty() {
        return Err(Error::Invalid("a table needs at least one column".into()));
    }
    let width = columns.len();
    let names = unique_columns(columns);
    let types: Vec<ColumnType> = (0..width)
        .map(|c| infer_type(rows.iter().map(|r| r.get(c).map_or("", String::as_str))))
        .collect();

    let dir = tempfile::Builder::new()
        .prefix("reactor-sheet-")
        .tempdir()
        .map_err(|e| Error::Io(format!("a place to put the table: {e}")))?;
    let path = dir.path().join("sheet.sqlite");
    {
        let mut conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "OFF")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        let defs: Vec<String> = names
            .iter()
            .zip(&types)
            .map(|(n, t)| format!("{} {}", quote(n), t.sql()))
            .collect();
        conn.execute_batch(&format!(
            "CREATE TABLE {} ({})",
            quote(table),
            defs.join(", ")
        ))?;
        let tx = conn.transaction()?;
        {
            let marks = vec!["?"; width].join(", ");
            let mut insert =
                tx.prepare(&format!("INSERT INTO {} VALUES ({marks})", quote(table)))?;
            for row in rows {
                let cells = (0..width).map(|c| {
                    let cell = row.get(c).map_or("", String::as_str);
                    match types[c] {
                        ColumnType::Text => rusqlite::types::Value::Text(cell.to_string()),
                        _ if cell.is_empty() => rusqlite::types::Value::Null,
                        ColumnType::Integer => cell.parse().map_or(
                            rusqlite::types::Value::Null,
                            rusqlite::types::Value::Integer,
                        ),
                        ColumnType::Real => cell
                            .parse()
                            .map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Real),
                    }
                });
                insert.execute(rusqlite::params_from_iter(cells))?;
            }
        }
        tx.commit()?;
    }
    let db = Database::connect(&path, path.clone(), OpenMode::ReadOnly, Some(Arc::new(dir)))?;
    Ok(TextTable {
        db,
        table: table.to_string(),
        columns: names,
        types,
        rows: rows.len() as u64,
    })
}

// ---------------------------------------------------------------------------------------
// The one-call forms
// ---------------------------------------------------------------------------------------

/// The tables and views of a database file.
pub fn tables(path: &Path) -> Result<Vec<TableInfo>> {
    Database::open(path)?.tables()
}

/// The rows of one table, as text, the column names first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<Vec<String>>,
    /// The table has more rows than were read.
    pub truncated: bool,
}

/// Read a table: its column names, then up to `limit` rows in rowid order where it has
/// one.
pub fn read_table(path: &Path, table: &str, limit: usize) -> Result<Rows> {
    let db = Database::open(path)?;
    let mut stmt = db
        .conn
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
                .map(|i| row.get_ref(i).map(|v| Value::from_ref(v).display()))
                .collect::<std::result::Result<_, _>>()?,
        );
    }
    Ok(Rows { rows, truncated })
}
