//! What a reference names in an open case: whether it exists, and what a set of them holds
//! ([ADR-0066](../../../../docs/adr/0066-the-case-is-addressed-by-references.md)).
//!
//! Both read the database and nothing else. A file's bytes are not here: a reference to a
//! file says where it is, and `read` reads it.

use rusqlite::OptionalExtension;

use super::{GraphId, Kind, Reference, Target, param};
use crate::artifact;
use crate::case::Case;
use crate::corpus;
use crate::error::{Error, Result};
use crate::graph::{self, Filter, Kind as ElementKind, query};
use crate::sessions;

/// The size of a page of a set. The default is the one `graph_query find` has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub limit: usize,
    pub offset: usize,
}

impl Default for Page {
    fn default() -> Self {
        Page {
            limit: 40,
            offset: 0,
        }
    }
}

/// One member of a set: its reference, and a line saying what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub reference: Reference,
    pub preview: String,
}

/// A page of a set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    pub items: Vec<Item>,
    /// There are more after this page.
    pub more: bool,
}

fn flag(params: &[(String, String)], name: &str) -> bool {
    param(params, name).is_some_and(|v| matches!(v, "" | "1" | "true" | "yes"))
}

fn one_line(s: &str, max: usize) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        line
    } else {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// The id a graph reference stands for. An entity named by what it is has the id its type
/// and value derive, whether or not it was ever recorded.
pub fn graph_id(case: &Case, id: &GraphId) -> Result<String> {
    match id {
        GraphId::Id(id) => Ok(id.clone()),
        GraphId::Entity {
            type_,
            value,
            scope,
        } => query::entity_id(case.db(), type_, value, scope.as_deref()),
    }
}

/// Whether `target` names something this case has. A set always does (it may be empty).
pub fn exists(case: &Case, target: &Target) -> Result<bool> {
    let db = case.db();
    let one = |sql: &str, p: &dyn rusqlite::ToSql| -> Result<bool> {
        db.read(|conn| Ok(conn.query_row(&format!("SELECT EXISTS({sql})"), [p], |r| r.get(0))?))
    };
    Ok(match target {
        Target::Session { session, entry } => match entry {
            None => sessions::exists(db, session)?,
            Some(n) => db.read(|conn| {
                Ok(conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM entry WHERE session_id = ?1 AND id = ?2)",
                    rusqlite::params![session, *n as i64],
                    |r| r.get(0),
                )?)
            })?,
        },
        Target::Graph { id, ver } => {
            let id = graph_id(case, id)?;
            match ver {
                None => query::get(db, &id)?.is_some(),
                Some(v) => query::history(db, &id)?.iter().any(|e| e.ver == *v),
            }
        }
        Target::Artifact { path, .. } => artifact::locate(case, path).is_ok(),
        Target::Doc { id, passage } => match passage {
            None => one("SELECT 1 FROM document WHERE id = ?1", id)?,
            Some(n) => db.read(|conn| {
                Ok(conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM chunk WHERE document_id = ?1 AND ord = ?2)",
                    rusqlite::params![id, *n as i64],
                    |r| r.get(0),
                )?)
            })?,
        },
        Target::Blob { sha256 } => one("SELECT 1 FROM blob WHERE sha256 = ?1", sha256)?,
        Target::Change { seq } => one("SELECT 1 FROM event WHERE seq = ?1", &(*seq as i64))?,
        Target::Vocab { name } => {
            one("SELECT 1 FROM known_type WHERE name = ?1", name)?
                || one("SELECT 1 FROM element WHERE type = ?1", name)?
        }
        Target::Set { .. } => true,
    })
}

/// A line that says what an element is.
pub fn element_preview(e: &graph::Element) -> String {
    let value = e.content.value.as_deref().unwrap_or("");
    let status = if e.content.status == "active" {
        String::new()
    } else {
        format!(" [{}]", e.content.status)
    };
    let value = if value.is_empty() {
        String::new()
    } else {
        format!(" {}", one_line(value, 90))
    };
    format!("{} {}{value}{status}", e.kind.as_str(), e.type_)
}

/// A reference to an element as it stands now.
pub fn element_reference(e: &graph::Element) -> Reference {
    Reference::graph(&e.id)
}

/// A page of the set `kind:?params`.
pub fn list(case: &Case, kind: Kind, params: &[(String, String)], page: Page) -> Result<Listing> {
    let db = case.db();
    // One more than a page, to know whether there is another.
    let want = page.limit.max(1) + 1;
    let text = param(params, "text").map(|t| t.to_lowercase());
    let mut items: Vec<Item> = match kind {
        Kind::Graph => {
            if flag(params, "orphans") {
                query::orphans(db)?
                    .iter()
                    .map(|e| Item {
                        reference: element_reference(e),
                        preview: element_preview(e),
                    })
                    .skip(page.offset)
                    .take(want)
                    .collect()
            } else if flag(params, "stale") {
                query::stale(db)?
                    .into_iter()
                    .skip(page.offset)
                    .take(want)
                    .map(|s| Item {
                        reference: Reference::graph(&s.id),
                        preview: one_line(&s.reason, 120),
                    })
                    .collect()
            } else {
                let kind = match param(params, "kind") {
                    None | Some("") => None,
                    Some(k) => Some(ElementKind::parse(k).ok_or_else(|| {
                        Error::Invalid(format!(
                            "`kind={k}`: entity, event, observation, annotation or external"
                        ))
                    })?),
                };
                query::find(
                    db,
                    &Filter {
                        kind,
                        type_prefix: param(params, "type").map(str::to_string),
                        text: param(params, "text").map(str::to_string),
                        tag: param(params, "tag").map(str::to_string),
                        props: param(params, "prop")
                            .map(|p| vec![p.to_string()])
                            .unwrap_or_default(),
                        include_deleted: flag(params, "include_deleted"),
                        limit: want,
                        offset: page.offset,
                    },
                )?
                .iter()
                .map(|e| Item {
                    reference: element_reference(e),
                    preview: element_preview(e),
                })
                .collect()
            }
        }
        Kind::Session => sessions::list(db)?
            .into_iter()
            .filter(|s| {
                text.as_ref().is_none_or(|t| {
                    s.id.to_lowercase().contains(t)
                        || s.title.as_deref().unwrap_or("").to_lowercase().contains(t)
                        || s.first_prompt
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(t)
                })
            })
            .skip(page.offset)
            .take(want)
            .map(|s| {
                let what = s
                    .title
                    .as_deref()
                    .or(s.first_prompt.as_deref())
                    .unwrap_or("");
                Item {
                    reference: Reference::new(Target::Session {
                        session: s.id.clone(),
                        entry: None,
                    }),
                    preview: format!("{} entries. {}", s.entries, one_line(what, 80)),
                }
            })
            .collect(),
        Kind::Artifact => {
            let changed = flag(params, "changed");
            let rows: Vec<artifact::Artifact> = if changed {
                artifact::verify(case)?
                    .into_iter()
                    .filter(|(_, s)| *s != artifact::State::Unchanged)
                    .map(|(a, _)| a)
                    .collect()
            } else {
                artifact::list(case)?
            };
            rows.into_iter()
                .filter(|a| {
                    text.as_ref()
                        .is_none_or(|t| a.path.to_lowercase().contains(t))
                })
                .skip(page.offset)
                .take(want)
                .map(|a| Item {
                    preview: format!(
                        "{} bytes{}",
                        a.size,
                        a.kind
                            .as_deref()
                            .map(|k| format!(", {k}"))
                            .unwrap_or_default()
                    ),
                    reference: Reference::artifact(&a.path),
                })
                .collect()
        }
        Kind::Doc => corpus::documents(db)?
            .into_iter()
            .filter(|d| {
                text.as_ref().is_none_or(|t| {
                    d.title.as_deref().unwrap_or("").to_lowercase().contains(t)
                        || d.source_uri.to_lowercase().contains(t)
                })
            })
            .skip(page.offset)
            .take(want)
            .map(|d| Item {
                preview: format!(
                    "{}, {} passages",
                    one_line(d.title.as_deref().unwrap_or(&d.source_uri), 80),
                    d.chunks
                ),
                reference: Reference::new(Target::Doc {
                    id: d.id,
                    passage: None,
                }),
            })
            .collect(),
        Kind::Vocab => {
            let prefix = param(params, "type").map(str::to_lowercase);
            let vocab = graph::vocab::load(db)?;
            vocab
                .types
                .values()
                .filter(|t| {
                    prefix
                        .as_ref()
                        .is_none_or(|p| t.name.starts_with(p.as_str()))
                })
                .filter(|t| {
                    text.as_ref().is_none_or(|x| {
                        t.name.contains(x.as_str()) || t.definition.to_lowercase().contains(x)
                    })
                })
                .skip(page.offset)
                .take(want)
                .map(|t| Item {
                    reference: Reference::new(Target::Vocab {
                        name: t.name.clone(),
                    }),
                    preview: one_line(&t.definition, 100),
                })
                .collect()
        }
        Kind::Change => db.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT seq, ts, type FROM event ORDER BY seq DESC LIMIT ?1 OFFSET ?2")?;
            let rows = stmt
                .query_map(rusqlite::params![want as i64, page.offset as i64], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows
                .into_iter()
                .map(|(seq, ts, ty)| Item {
                    reference: Reference::new(Target::Change { seq: seq as u64 }),
                    preview: format!("{ty} at {ts}"),
                })
                .collect())
        })?,
        Kind::Blob => {
            return Err(Error::Invalid(
                "blobs are not listed: name one by its SHA-256".into(),
            ));
        }
    };
    let more = items.len() > page.limit.max(1);
    items.truncate(page.limit.max(1));
    Ok(Listing { items, more })
}

/// The ordinal of a passage in its document.
pub fn passage_ord(case: &Case, chunk_id: i64) -> Result<Option<usize>> {
    case.db().read(|conn| {
        Ok(conn
            .query_row("SELECT ord FROM chunk WHERE id = ?1", [chunk_id], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .map(|n| n as usize))
    })
}

/// One write batch of the change log, and what it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeInfo {
    pub seq: u64,
    pub ts: String,
    pub type_: String,
    pub session: Option<String>,
    pub entry: Option<u64>,
    /// `kind:identity`, as the author is recorded.
    pub author: String,
    pub payload: String,
    /// The element versions the batch wrote, as `(element id, version)`.
    pub versions: Vec<(String, u32)>,
}

/// The change numbered `seq`, if there is one.
pub fn change_info(case: &Case, seq: u64) -> Result<Option<ChangeInfo>> {
    case.db().read(|conn| {
        let head = conn
            .query_row(
                "SELECT e.ts, e.type, e.session_id, e.entry_id, e.payload,
                        coalesce(a.kind || ':' || a.identity, '')
                   FROM event e LEFT JOIN author a ON a.id = e.author_id
                  WHERE e.seq = ?1",
                [seq as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((ts, type_, session, entry, payload, author)) = head else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT element_id, ver FROM element_version WHERE event_seq = ?1
              ORDER BY element_id, ver",
        )?;
        let versions = stmt
            .query_map([seq as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some(ChangeInfo {
            seq,
            ts,
            type_,
            session,
            entry: entry.map(|e| e as u64),
            author,
            payload,
            versions,
        }))
    })
}
