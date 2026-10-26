//! Pulling: turning rows of an external into graph elements
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! Data enters the graph only when it is relevant, and a pull is how: plain SQL over a
//! source in the case, whose **column names say what each value is**.
//!
//! ```sql
//! SELECT ts AS "@time", record_id AS "@row", 'auth.logon' AS "@type",
//!        src_ip AS "net.ipv4@src", username AS "account.user@actor"
//!   FROM src WHERE src_ip = '198.51.100.7'
//! ```
//!
//! | column | means |
//! |---|---|
//! | `@type` | the type of the claim made per row (required) |
//! | `@kind` | `event` or `observation`; else an event if there is a `@time`, an observation if not |
//! | `@time` | when it happened (event) |
//! | `@valid_from`, `@valid_to` | when it held (observation) |
//! | `@row` | a stable id for the row: its locator, and what makes a re-pull land on the same claim |
//! | `@value`, `@confidence` | the claim's own value and confidence |
//! | `<type>@<role>` | an entity of that type, linked with that role |
//! | anything else | a property of the claim |
//!
//! One source table is always there as `src`: the table named in an `artifact:...#table=<t>` reference, or
//! the rows of a CSV, JSON or JSONL file. A whole SQLite file is attached as `data`.
//!
//! The query runs on a connection of its own that can only read: no writes, no
//! `ATTACH`, no pragmas, and a time limit. Pulling the same rows again changes nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{Applied, Batch, Content, Kind, Link, Put, Target, identity};
use crate::artifact;
use crate::case::Case;
use crate::error::{Error, Result};
use crate::manifest::Author;
use crate::time::parse_source_time;

/// More rows than this is not a pull: aggregate it, or narrow it. Bulk stays in the
/// external.
pub const DEFAULT_MAX_ROWS: usize = 5_000;
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Request {
    /// The external to pull from: `artifact:reactor_data.sqlite#table=fw_conn`,
    /// `artifact:data/pdns.csv`, `artifact:out/edr.jsonl`, `artifact:out/report.json#/hits`, or a
    /// bare path whose extension says which.
    pub source: String,
    pub query: String,
    pub author: Author,
    pub on_behalf_of: Option<Author>,
    pub session: Option<String>,
    pub entry: Option<u64>,
    pub max_rows: usize,
    /// Run the query and say what it would make, without writing.
    pub preview: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// The external's URI as it is stored.
    pub source: String,
    pub rows: usize,
    /// The claims the rows became (empty in a preview).
    pub claims: Vec<String>,
    pub applied: Applied,
    /// Earlier pulls from the same external with another query: their rows may overlap
    /// these. Reported, never resolved.
    pub overlaps: Vec<String>,
    /// The first rows as they would be read, one line each.
    pub sample: Vec<String>,
    /// Rows that could not become a claim, and why.
    pub skipped: Vec<String>,
}

enum Source {
    Sqlite {
        file: PathBuf,
        table: Option<String>,
    },
    Csv(PathBuf),
    Jsonl(PathBuf),
    Json {
        file: PathBuf,
        pointer: String,
    },
}

/// What a source string means: the URI the external is stored under, and how to read it. A
/// source is a file of the case, `artifact:<path>`, and what is read from it follows the
/// file: a table of a SQLite file (`#table=<t>`), the array a JSON pointer names
/// (`#/events`), or every row of a CSV or JSON lines file.
fn resolve(case: &Case, raw: &str) -> Result<(String, Source, PathBuf)> {
    let raw = identity::canonical_uri(raw)?;
    let target = match crate::reference::parse(&raw) {
        Ok(r) => r.target,
        Err(_) => {
            return Err(Error::Invalid(format!(
                "`{raw}`: a pull reads a file of the case: `artifact:<path>`"
            )));
        }
    };
    let crate::reference::Target::Artifact { path, locator } = target else {
        return Err(Error::Invalid(format!(
            "`{raw}`: a pull reads a file of the case: `artifact:<path>`"
        )));
    };
    // The case's own database is read, and only read.
    let (stored, file) = if path == ".reactor/analysis.db" {
        (path.clone(), case.dir().join("analysis.db"))
    } else {
        artifact::locate(case, &path)?
    };
    let ext = Path::new(&stored)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let place = locator
        .as_deref()
        .and_then(crate::reference::Locator::parse);
    let uri = |loc: Option<String>| {
        let mut r = crate::reference::Reference::artifact(&stored);
        if let crate::reference::Target::Artifact { locator, .. } = &mut r.target {
            *locator = loc;
        }
        r.short()
    };
    match ext.as_str() {
        "sqlite" | "sqlite3" | "db" => {
            let table = match place {
                Some(crate::reference::Locator::Table { table, .. }) => Some(table),
                _ => None,
            };
            Ok((
                uri(table.as_ref().map(|t| format!("table={t}"))),
                Source::Sqlite {
                    file: file.clone(),
                    table,
                },
                file,
            ))
        }
        "csv" | "tsv" => Ok((uri(None), Source::Csv(file.clone()), file)),
        "jsonl" | "ndjson" => Ok((uri(None), Source::Jsonl(file.clone()), file)),
        "json" => {
            let pointer = match place {
                Some(crate::reference::Locator::JsonPointer(p)) => p,
                _ => String::new(),
            };
            Ok((
                uri((!pointer.is_empty()).then(|| pointer.clone())),
                Source::Json {
                    file: file.clone(),
                    pointer,
                },
                file,
            ))
        }
        _ => Err(Error::Invalid(format!(
            "`{raw}`: cannot tell what this file is -- a pull reads .sqlite, .csv, .tsv, .jsonl and .json"
        ))),
    }
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Load rows of JSON objects into a temporary table `src`: one column per top-level
/// key, nested values kept as JSON text, plus `_row` and the whole object as `_json`.
fn load_objects(conn: &Connection, objects: &[Value]) -> Result<()> {
    let mut columns: Vec<String> = Vec::new();
    for o in objects {
        if let Some(m) = o.as_object() {
            for k in m.keys() {
                if !k.starts_with('_') && !columns.contains(k) {
                    columns.push(k.clone());
                }
            }
        }
    }
    let defs: String = columns.iter().map(|c| format!(", {}", quote(c))).collect();
    conn.execute_batch(&format!(
        "CREATE TEMP TABLE src (_row INTEGER, _json TEXT{defs})"
    ))?;
    let marks: String = columns.iter().map(|_| ", ?").collect();
    let mut stmt = conn.prepare(&format!("INSERT INTO src VALUES (?, ?{marks})"))?;
    for (i, o) in objects.iter().enumerate() {
        let mut params: Vec<rusqlite::types::Value> = vec![
            rusqlite::types::Value::Integer(i as i64 + 1),
            rusqlite::types::Value::Text(o.to_string()),
        ];
        for c in &columns {
            params.push(match o.get(c) {
                None | Some(Value::Null) => rusqlite::types::Value::Null,
                Some(Value::String(s)) => rusqlite::types::Value::Text(s.clone()),
                Some(Value::Bool(b)) => rusqlite::types::Value::Integer(*b as i64),
                Some(Value::Number(n)) => match n.as_i64() {
                    Some(i) => rusqlite::types::Value::Integer(i),
                    None => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
                },
                Some(nested) => rusqlite::types::Value::Text(nested.to_string()),
            });
        }
        stmt.execute(rusqlite::params_from_iter(params))?;
    }
    Ok(())
}

fn load(conn: &Connection, source: &Source) -> Result<()> {
    match source {
        Source::Sqlite { file, table } => {
            let uri = format!(
                "file:{}?mode=ro",
                file.to_string_lossy()
                    .replace('%', "%25")
                    .replace('?', "%3f")
                    .replace('#', "%23")
            );
            conn.execute("ATTACH DATABASE ?1 AS data", [&uri])?;
            if let Some(t) = table {
                let with_row = format!(
                    "CREATE TEMP VIEW src AS SELECT rowid AS _row, * FROM data.{}",
                    quote(t)
                );
                // A view, or a table without a rowid, has none to offer.
                if conn.execute_batch(&with_row).is_err()
                    || conn.prepare("SELECT * FROM src LIMIT 0").is_err()
                {
                    conn.execute_batch("DROP VIEW IF EXISTS src")?;
                    conn.execute_batch(&format!(
                        "CREATE TEMP VIEW src AS SELECT * FROM data.{}",
                        quote(t)
                    ))?;
                    conn.prepare("SELECT * FROM src LIMIT 0").map_err(|e| {
                        Error::Invalid(format!("no table `{t}` in that database ({e})"))
                    })?;
                }
            }
            Ok(())
        }
        Source::Csv(file) => {
            let tab = file.extension().is_some_and(|e| e == "tsv");
            let mut reader = csv::ReaderBuilder::new()
                .flexible(true)
                .delimiter(if tab { b'\t' } else { b',' })
                .from_path(file)
                .map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
            let headers: Vec<String> = reader
                .headers()
                .map_err(|e| Error::Io(format!("{}: {e}", file.display())))?
                .iter()
                .map(|h| h.trim().trim_start_matches('\u{feff}').to_string())
                .collect();
            let mut objects = Vec::new();
            for record in reader.records() {
                let record = record.map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
                let mut o = Map::new();
                for (i, h) in headers.iter().enumerate() {
                    if !h.is_empty() {
                        o.insert(h.clone(), record.get(i).map_or(Value::Null, |v| json!(v)));
                    }
                }
                objects.push(Value::Object(o));
            }
            // Keep the header order, which `load_objects` takes from the first rows.
            if objects.is_empty() {
                let empty: Map<String, Value> =
                    headers.iter().map(|h| (h.clone(), Value::Null)).collect();
                load_objects(conn, &[Value::Object(empty)])?;
                conn.execute_batch("DELETE FROM src")?;
                return Ok(());
            }
            load_objects(conn, &objects)
        }
        Source::Jsonl(file) => {
            let text = std::fs::read_to_string(file).map_err(|e| Error::io(file, e))?;
            let mut objects = Vec::new();
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                objects.push(serde_json::from_str::<Value>(line).map_err(|e| {
                    Error::Invalid(format!(
                        "{}: line {} is not JSON ({e})",
                        file.display(),
                        i + 1
                    ))
                })?);
            }
            load_objects(conn, &objects)
        }
        Source::Json { file, pointer } => {
            let text = std::fs::read_to_string(file).map_err(|e| Error::io(file, e))?;
            let doc: Value = serde_json::from_str(&text)
                .map_err(|e| Error::Invalid(format!("{}: not JSON ({e})", file.display())))?;
            let at = doc.pointer(pointer).ok_or_else(|| {
                Error::Invalid(format!("{}: nothing at `{pointer}`", file.display()))
            })?;
            match at {
                Value::Array(items) => load_objects(conn, items),
                one => load_objects(conn, std::slice::from_ref(one)),
            }
        }
    }
}

/// From here on the connection can only read.
fn lock_down(conn: &Connection) -> Result<()> {
    conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Function { .. }
        | AuthAction::Recursive => Authorization::Allow,
        _ => Authorization::Deny,
    }))?;
    let started = Instant::now();
    conn.progress_handler(10_000, Some(move || started.elapsed() > TIMEOUT))?;
    Ok(())
}

fn cell(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
    }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        other => Some(other.to_string()),
    }
}

/// One row as a claim, or why it cannot be one.
fn claim(
    columns: &[String],
    values: &[Value],
    external: &str,
    uri: &str,
) -> std::result::Result<Put, String> {
    let get = |name: &str| {
        columns
            .iter()
            .position(|c| c == name)
            .and_then(|i| text(&values[i]))
    };
    let type_ = get("@type").ok_or("no @type")?;
    let (time, from, to) = (get("@time"), get("@valid_from"), get("@valid_to"));
    let kind = match get("@kind").as_deref().map(Kind::parse) {
        Some(Some(k)) if k.is_claim() => k,
        Some(_) => return Err("@kind must be event or observation".into()),
        None if time.is_some() => Kind::Event,
        None => Kind::Observation,
    };
    // What the row was, for telling later whether it still is.
    let fingerprint = {
        let pairs: Vec<Value> = columns
            .iter()
            .zip(values)
            .map(|(c, v)| json!([c, v]))
            .collect();
        let digest = Sha256::digest(Value::Array(pairs).to_string());
        digest
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let locator = get("@row").unwrap_or_else(|| format!("sha={fingerprint}"));

    let mut content = Content {
        value: get("@value"),
        confidence: get("@confidence"),
        ..Content::default()
    };
    let mut read = |raw: Option<String>, into: fn(&mut Content) -> &mut Option<String>| {
        let Some(raw) = raw else { return };
        match parse_source_time(&raw) {
            Some(p) => {
                *into(&mut content) = Some(p.utc);
                content.time.tz_certainty =
                    Some(if p.zone_stated { "stated" } else { "assumed" }.to_string());
                content.time.tz = (!p.zone_stated).then(|| "UTC".to_string());
                content.time.precision = Some(p.precision.to_string());
            }
            None => content.time.tz_certainty = Some("unknown".into()),
        }
        let kept = content.time.raw.get_or_insert_with(String::new);
        if !kept.is_empty() {
            kept.push_str(" | ");
        }
        kept.push_str(&raw);
    };
    read(time, |c| &mut c.time.occurred_at);
    read(from, |c| &mut c.time.valid_from);
    read(to, |c| &mut c.time.valid_to);

    let mut links = Vec::new();
    for (column, value) in columns.iter().zip(values) {
        if column.starts_with('@') || column.starts_with('_') {
            continue;
        }
        match column.rsplit_once('@') {
            Some((type_, role)) if !type_.is_empty() && !role.is_empty() => {
                if let Some(v) = text(value) {
                    links.push(Link::entity(role, type_, &v));
                }
            }
            _ => {
                if !value.is_null() {
                    content.props.insert(column.clone(), value.clone());
                }
            }
        }
    }
    links.push(Link::source(uri).at(&format!("row={locator}"), Some(&fingerprint)));
    Ok(Put {
        kind,
        id: Some(identity::derived_id(
            kind,
            &["pull", external, &type_, &locator],
        )),
        type_,
        scope: None,
        content,
        links,
        label: None,
        author: None,
        recorded_at: None,
    })
}

fn describe(put: &Put) -> String {
    let mut parts = vec![format!("{} {}", put.kind.as_str(), put.type_)];
    if let Some(t) = put
        .content
        .time
        .occurred_at
        .as_ref()
        .or(put.content.time.valid_from.as_ref())
    {
        parts.push(format!("at {t}"));
    }
    for l in &put.links {
        if let Target::Entity { type_, value, .. } = &l.to {
            parts.push(format!("{}={}:{}", l.role, type_, value));
        }
    }
    parts.join(" ")
}

/// Pull from a source in the case.
pub fn pull(case: &Case, req: Request) -> Result<Report> {
    let (uri, source, file) = resolve(case, &req.source)?;
    let external = identity::derived_id(Kind::External, &[&uri]);

    let conn = Connection::open_with_flags(
        ":memory:",
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    load(&conn, &source)?;
    lock_down(&conn)?;

    let max = if req.max_rows == 0 {
        DEFAULT_MAX_ROWS
    } else {
        req.max_rows
    };
    let mut stmt = conn
        .prepare(&req.query)
        .map_err(|e| Error::Invalid(format!("the pull query was refused: {e}")))?;
    if !stmt.readonly() {
        return Err(Error::Invalid("a pull query only reads".into()));
    }
    let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    if !columns.iter().any(|c| c == "@type") {
        return Err(Error::Invalid(
            "a pull query names what each row is: select the type as \"@type\"".into(),
        ));
    }
    let mut report = Report {
        source: uri.clone(),
        ..Report::default()
    };
    let mut puts = Vec::new();
    let mut rows = stmt
        .query([])
        .map_err(|e| Error::Invalid(format!("the pull query failed: {e}")))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| Error::Invalid(format!("the pull query failed: {e}")))?
    {
        report.rows += 1;
        if report.rows > max {
            return Err(Error::Invalid(format!(
                "more than {max} rows: narrow the query or aggregate -- bulk data stays in the external"
            )));
        }
        let values: Vec<Value> = (0..columns.len())
            .map(|i| row.get_ref(i).map(cell))
            .collect::<std::result::Result<_, _>>()?;
        match claim(&columns, &values, &external, &uri) {
            Ok(put) => {
                if report.sample.len() < 5 {
                    report.sample.push(describe(&put));
                }
                puts.push(put);
            }
            Err(why) => {
                if report.skipped.len() < 20 {
                    report.skipped.push(format!("row {}: {why}", report.rows));
                }
            }
        }
    }
    drop(rows);
    drop(stmt);
    drop(conn);

    report.overlaps = earlier_pulls(case, &external, &req.query)?;
    if req.preview {
        return Ok(report);
    }

    // The pull itself is something that happened, with its query, so it can be
    // understood and repeated.
    let record = Put {
        id: Some(identity::derived_id(
            Kind::Event,
            &["analysis.pull", &external, &req.query],
        )),
        content: Content {
            props: json!({
                "query": req.query,
                "rows": puts.len(),
                "source_sha256": artifact::sha256_of(&file)?,
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
            ..Content::default()
        },
        links: vec![Link::source(&uri)],
        ..Put::new(Kind::Event, "analysis.pull")
    };
    let n = puts.len();
    puts.push(record);
    let batch = Batch {
        author: req.author,
        on_behalf_of: req.on_behalf_of,
        session: req.session,
        entry: req.entry,
        kind: "pull".into(),
        payload: json!({ "source": uri, "query": req.query }),
        puts,
        changes: Vec::new(),
    };
    let applied = super::apply(case.db(), batch)?;
    report.claims = applied.ids[..n].to_vec();
    report.applied = applied;
    Ok(report)
}

/// The queries of earlier pulls from the same external, other than this one.
fn earlier_pulls(case: &Case, external: &str, query: &str) -> Result<Vec<String>> {
    case.db().read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT json_extract(c.props, '$.query') FROM element_current c
               JOIN edge_current g ON g.from_id = c.id
              WHERE c.type = 'analysis.pull' AND c.status <> 'soft-deleted'
                AND g.role = 'source' AND g.to_id = ?1
              ORDER BY c.recorded_at",
        )?;
        let rows: Vec<Option<String>> = stmt
            .query_map([external], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows.into_iter().flatten().filter(|q| q != query).collect())
    })
}

/// Externals pulled from whose file is no longer what it was at the last pull.
pub fn changed_sources(case: &Case) -> Result<Vec<String>> {
    let pulls: Vec<(String, Option<String>)> = case.db().read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT x.value, json_extract(c.props, '$.source_sha256')
               FROM element_current c
               JOIN edge_current g ON g.from_id = c.id AND g.role = 'source'
               JOIN element_current x ON x.id = g.to_id
              WHERE c.type = 'analysis.pull' AND c.status <> 'soft-deleted'
              ORDER BY c.recorded_at",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })?;
    // The latest pull of each external is the one to compare with.
    let mut latest: std::collections::BTreeMap<String, Option<String>> = Default::default();
    for (uri, sha) in pulls {
        latest.insert(uri, sha);
    }
    let mut changed = Vec::new();
    for (uri, sha) in latest {
        let now = resolve(case, &uri)
            .ok()
            .and_then(|(_, _, file)| artifact::sha256_of(&file).ok());
        if now != sha {
            changed.push(uri);
        }
    }
    Ok(changed)
}
