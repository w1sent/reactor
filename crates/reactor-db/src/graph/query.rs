//! Reading the graph: the current state, the state as of a moment, and the views a
//! person or an agent asks for.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Map, Value};

use super::{Content, Kind, Time, identity};
use crate::db::Db;
use crate::error::{Error, Result};

/// One version of an element.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Element {
    pub id: String,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub type_: String,
    pub ver: u32,
    /// Record time: when this version was written.
    pub recorded_at: String,
    pub author: String,
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_ver: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// A binary claim as a person means it: `from` is related to `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relation {
    pub from_id: String,
    pub to_id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub claim_id: String,
    pub from_role: String,
    pub to_role: String,
}

const COLUMNS: &str = "e.id, e.kind, e.type, v.ver, v.recorded_at,
    a.kind || ':' || a.identity || CASE WHEN a.version = '' THEN '' ELSE '@' || a.version END,
    v.status, v.confidence, v.value, v.props, v.tags, v.occurred_at, v.duration_ms,
    v.valid_from, v.valid_to, v.tz, v.tz_certainty, v.time_raw, v.time_precision";

const FROM: &str = "element e JOIN element_version v ON v.element_id = e.id
    JOIN author a ON a.id = v.author_id";

fn content(r: &Row, at: usize) -> rusqlite::Result<Content> {
    let props: String = r.get(at + 3)?;
    let tags: String = r.get(at + 4)?;
    Ok(Content {
        status: r.get(at)?,
        confidence: r.get(at + 1)?,
        value: r.get(at + 2)?,
        props: serde_json::from_str::<Map<String, Value>>(&props).unwrap_or_default(),
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        time: Time {
            occurred_at: r.get(at + 5)?,
            duration_ms: r.get(at + 6)?,
            valid_from: r.get(at + 7)?,
            valid_to: r.get(at + 8)?,
            tz: r.get(at + 9)?,
            tz_certainty: r.get(at + 10)?,
            raw: r.get(at + 11)?,
            precision: r.get(at + 12)?,
        },
    })
}

fn element(r: &Row) -> rusqlite::Result<Element> {
    let kind: String = r.get(1)?;
    Ok(Element {
        id: r.get(0)?,
        kind: Kind::parse(&kind).unwrap_or(Kind::Annotation),
        type_: r.get(2)?,
        ver: r.get(3)?,
        recorded_at: r.get(4)?,
        author: r.get(5)?,
        content: content(r, 6)?,
    })
}

/// The version at the top of an element's chain, and what it holds.
pub(super) fn current_content(conn: &Connection, id: &str) -> Result<(i64, Content)> {
    conn.query_row(
        "SELECT ver, status, confidence, value, props, tags, occurred_at, duration_ms,
                valid_from, valid_to, tz, tz_certainty, time_raw, time_precision
           FROM element_version WHERE element_id = ?1 ORDER BY ver DESC LIMIT 1",
        [id],
        |r| Ok((r.get(0)?, content(r, 1)?)),
    )
    .optional()?
    .ok_or_else(|| Error::Invalid(format!("no element {id}")))
}

/// An element as it stands.
pub fn get(db: &Db, id: &str) -> Result<Option<Element>> {
    db.read(|conn| {
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM {FROM} WHERE e.id = ?1 ORDER BY v.ver DESC LIMIT 1"
                ),
                [id],
                element,
            )
            .optional()?)
    })
}

/// An element as it stood at `at` (RFC 3339 UTC, record time): what was known then.
/// `None` if it did not exist yet.
pub fn get_as_of(db: &Db, id: &str, at: &str) -> Result<Option<Element>> {
    db.read(|conn| {
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM {FROM} WHERE e.id = ?1 AND v.recorded_at <= ?2
                     ORDER BY v.ver DESC LIMIT 1"
                ),
                params![id, at],
                element,
            )
            .optional()?)
    })
}

/// Every version of an element, oldest first.
pub fn history(db: &Db, id: &str) -> Result<Vec<Element>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM {FROM} WHERE e.id = ?1 ORDER BY v.ver"
        ))?;
        let rows = stmt
            .query_map([id], element)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

fn edge(r: &Row) -> rusqlite::Result<Edge> {
    Ok(Edge {
        id: r.get(0)?,
        from_id: r.get(1)?,
        to_id: r.get(2)?,
        role: r.get(3)?,
        pinned_ver: r.get(4)?,
        locator: r.get(5)?,
        fingerprint: r.get(6)?,
    })
}

const EDGE: &str =
    "SELECT id, from_id, to_id, role, pinned_ver, locator, fingerprint FROM edge_current";

fn edges(db: &Db, where_: &str, id: &str) -> Result<Vec<Edge>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(&format!("{EDGE} WHERE {where_} = ?1 ORDER BY role, id"))?;
        let rows = stmt
            .query_map([id], edge)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// What an element points at: what a claim involves, what an annotation is about.
pub fn edges_from(db: &Db, id: &str) -> Result<Vec<Edge>> {
    edges(db, "from_id", id)
}

/// What points at an element: the claims an entity takes part in, the annotations on it.
pub fn edges_to(db: &Db, id: &str) -> Result<Vec<Edge>> {
    edges(db, "to_id", id)
}

/// Where a claim came from: an external it links to as `source`, and where in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRef {
    /// A relative path or a URL, or `session:<id>#entry=<n>` for what was said in a session.
    pub uri: String,
    /// Where in the source: `row=…`, a line, an offset.
    pub locator: Option<String>,
}

/// The sources of an element, in the order they were recorded: what to open to see where it
/// came from. An entity has none of its own.
pub fn sources_of(db: &Db, id: &str) -> Result<Vec<SourceRef>> {
    let mut out = Vec::new();
    for edge in edges_from(db, id)?
        .into_iter()
        .filter(|e| e.role == "source")
    {
        if let Some(external) = get(db, &edge.to_id)?
            && let Some(uri) = external.content.value
        {
            out.push(SourceRef {
                uri,
                locator: edge.locator,
            });
        }
    }
    Ok(out)
}

/// Which elements to list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub kind: Option<Kind>,
    /// `net.` matches `net.ipv4` and `net.domain`.
    pub type_prefix: Option<String>,
    /// A substring of the value or of the properties.
    pub text: Option<String>,
    pub tag: Option<String>,
    /// Properties it must have: `size>1MiB`, `time.first_seen<2026-03-01`, or a name (a
    /// namespace such as `time.` takes every name under it) alone (ADR-0067).
    pub props: Vec<String>,
    /// Soft-deleted elements are hidden unless asked for.
    pub include_deleted: bool,
    /// 0 means no limit.
    pub limit: usize,
    /// How many of the newest to skip: the start of a later page.
    pub offset: usize,
}

/// Elements as they stand, newest first.
pub fn find(db: &Db, f: &Filter) -> Result<Vec<Element>> {
    if f.props.is_empty() {
        return find_where(db, f);
    }
    // A property is compared by its shape, which SQL does not know: read every match of the
    // rest of the filter, keep those whose properties hold, and then cut the page.
    let by = f
        .props
        .iter()
        .map(|p| super::fields::PropFilter::parse(p))
        .collect::<Result<Vec<_>>>()?;
    let fields = super::fields::load(db)?;
    let all = find_where(
        db,
        &Filter {
            limit: 0,
            offset: 0,
            ..f.clone()
        },
    )?;
    let kept = all
        .into_iter()
        .filter(|e| by.iter().all(|p| p.matches(&fields, &e.content.props)))
        .skip(f.offset);
    Ok(if f.limit == 0 {
        kept.collect()
    } else {
        kept.take(f.limit).collect()
    })
}

fn find_where(db: &Db, f: &Filter) -> Result<Vec<Element>> {
    let like = |s: &str| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    };
    let prefix = f.type_prefix.as_ref().map(|p| {
        format!(
            "{}%",
            p.trim()
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let (kind, text) = (f.kind.map(Kind::as_str), f.text.as_deref().map(like));
    let tag = f.tag.as_ref().map(|t| t.trim().to_lowercase());
    let limit = if f.limit == 0 { -1 } else { f.limit as i64 };
    db.read(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM {FROM}
              WHERE v.ver = (SELECT max(ver) FROM element_version WHERE element_id = e.id)
                AND (?1 IS NULL OR e.kind = ?1)
                AND (?2 IS NULL OR e.type LIKE ?2 ESCAPE '\\')
                AND (?3 IS NULL OR v.value LIKE ?3 ESCAPE '\\' OR v.props LIKE ?3 ESCAPE '\\')
                AND (?4 IS NULL OR EXISTS (SELECT 1 FROM json_each(v.tags) WHERE value = ?4))
                AND (?5 OR v.status <> 'soft-deleted')
              ORDER BY v.recorded_at DESC, e.id LIMIT ?6 OFFSET ?7"
        ))?;
        let rows = stmt
            .query_map(
                params![
                    kind,
                    prefix,
                    text,
                    tag,
                    f.include_deleted,
                    limit,
                    f.offset as i64
                ],
                element,
            )?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// The binary claims, as `(from, to, type)` rows — all of them, or those touching one
/// element.
pub fn relations(db: &Db, about: Option<&str>) -> Result<Vec<Relation>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT from_id, to_id, type, claim_id, from_role, to_role FROM relations
              WHERE ?1 IS NULL OR from_id = ?1 OR to_id = ?1
              ORDER BY claim_id",
        )?;
        let rows = stmt
            .query_map([about], |r| {
                Ok(Relation {
                    from_id: r.get(0)?,
                    to_id: r.get(1)?,
                    type_: r.get(2)?,
                    claim_id: r.get(3)?,
                    from_role: r.get(4)?,
                    to_role: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// The id an entity of this type and value has, whether or not it is in the graph: the
/// answer to "where else does this show up?" starts here.
pub fn entity_id(db: &Db, type_: &str, value: &str, scope: Option<&str>) -> Result<String> {
    db.read(|conn| {
        let type_ = identity::canonical_type(conn, type_)?;
        let rule = identity::type_rule(conn, &type_)?;
        let (type_, value) = identity::normalize(rule.normalizer.as_deref(), &type_, value);
        let scope = scope.map(str::trim).unwrap_or("");
        Ok(identity::derived_id(
            Kind::Entity,
            &[&type_, &format!("value={value}"), &format!("scope={scope}")],
        ))
    })
}

/// Entities nothing says anything about: no active claim or annotation points at them.
pub fn orphans(db: &Db) -> Result<Vec<Element>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM {FROM}
              WHERE v.ver = (SELECT max(ver) FROM element_version WHERE element_id = e.id)
                AND e.kind = 'entity' AND v.status <> 'soft-deleted'
                AND NOT EXISTS (
                  SELECT 1 FROM edge_current g JOIN element_current c ON c.id = g.from_id
                   WHERE g.to_id = e.id AND c.status <> 'soft-deleted')
              ORDER BY e.id"
        ))?;
        let rows = stmt
            .query_map([], element)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// A type in use, how often, and whether the vocabulary knows it — the view that shows
/// spelling drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeUse {
    pub kind: Kind,
    #[serde(rename = "type")]
    pub type_: String,
    pub count: usize,
    pub known: bool,
}

pub fn types_in_use(db: &Db) -> Result<Vec<TypeUse>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT e.kind, e.type, count(*),
                    EXISTS(SELECT 1 FROM known_type k WHERE k.name = e.type)
               FROM element e GROUP BY e.kind, e.type ORDER BY e.kind, e.type",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let kind: String = r.get(0)?;
                Ok(TypeUse {
                    kind: Kind::parse(&kind).unwrap_or(Kind::Annotation),
                    type_: r.get(1)?,
                    count: r.get::<_, i64>(2)? as usize,
                    known: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// Something that may no longer hold. Flagged, never resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stale {
    pub id: String,
    pub reason: String,
}

/// What may be out of date: an assessment of a version that has since moved on, one
/// that rests on a claim now rejected or disputed, and anything older than its type
/// allows.
pub fn stale(db: &Db) -> Result<Vec<Stale>> {
    db.read(|conn| {
        let mut out = Vec::new();
        let mut collect = |sql: &str| -> Result<()> {
            let mut stmt = conn.prepare(sql)?;
            for row in stmt.query_map([], |r| {
                Ok(Stale {
                    id: r.get(0)?,
                    reason: r.get(1)?,
                })
            })? {
                out.push(row?);
            }
            Ok(())
        };
        collect(
            "SELECT g.from_id, 'judged version ' || g.pinned_ver || ' of ' || g.to_id ||
                    ', which is now at version ' || c.ver
               FROM edge_current g JOIN element_current c ON c.id = g.to_id
               JOIN element_current f ON f.id = g.from_id
              WHERE g.pinned_ver IS NOT NULL AND g.pinned_ver < c.ver
                AND f.status <> 'soft-deleted'
              ORDER BY g.from_id",
        )?;
        collect(
            "SELECT g.from_id, 'rests on ' || g.to_id || ' (' || g.role || '), which is now ' || c.status
               FROM edge_current g JOIN element_current c ON c.id = g.to_id
               JOIN element_current f ON f.id = g.from_id
              WHERE g.role IN ('about', 'supports', 'refutes', 'basis')
                AND c.status IN ('conflicting', 'soft-deleted')
                AND f.status <> 'soft-deleted'
              ORDER BY g.from_id",
        )?;
        collect(
            "SELECT c.id, 'recorded ' || cast(julianday('now') - julianday(c.recorded_at) AS INTEGER) ||
                    ' days ago; a ' || c.type || ' is checked again after ' || k.stale_after_days
               FROM element_current c JOIN known_type k ON k.name = c.type
              WHERE k.stale_after_days IS NOT NULL AND c.status <> 'soft-deleted'
                AND julianday('now') - julianday(c.recorded_at) > k.stale_after_days
              ORDER BY c.id",
        )?;
        Ok(out)
    })
}

/// The whole graph as it stands, for drawing: every element that is not soft-deleted,
/// and the edges between them.
pub fn snapshot(db: &Db, include_deleted: bool) -> Result<(Vec<Element>, Vec<Edge>)> {
    let elements = find(
        db,
        &Filter {
            include_deleted,
            ..Filter::default()
        },
    )?;
    let ids: std::collections::HashSet<&str> = elements.iter().map(|e| e.id.as_str()).collect();
    let all: Vec<Edge> = db.read(|conn| {
        let mut stmt = conn.prepare(&format!("{EDGE} ORDER BY id"))?;
        let rows = stmt
            .query_map([], edge)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })?;
    let edges = all
        .into_iter()
        .filter(|g| ids.contains(g.from_id.as_str()) && ids.contains(g.to_id.as_str()))
        .collect();
    Ok((elements, edges))
}
