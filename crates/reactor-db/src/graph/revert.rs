//! Revert: taking versions off the top of a chain
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! A change is how a belief is revised: it keeps the trace. A revert is for a mistake
//! nothing builds on yet — a typo, a pull of the wrong rows — and leaves no trace of
//! what was popped beyond a row in the change log.
//!
//! Only the top of a chain can go, so returning to an earlier version pops everything
//! above it. What stands in the way is something that **builds on** a version to be
//! popped: an edge that pinned it. Then the revert is refused, unless it is forced — and
//! a forced revert takes what built on it along, transitively. [`plan`] says exactly
//! what would go, and whose it is, before [`apply`] does it.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::json;

use crate::db::Db;
use crate::error::{Error, Result};
use crate::manifest::{Author, author_id};
use crate::time::now_rfc3339;

/// What to take back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// Everything one change-log row wrote: a pull, a batch of edits.
    Event(i64),
    /// One element, back to version `to` — or out of the graph, with `to: 0`.
    Element { id: String, to: u32 },
}

/// One element a revert touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Affected {
    pub id: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub type_: String,
    /// The version it returns to; 0 means it leaves the graph.
    pub back_to: u32,
    /// Who wrote the versions that go.
    pub authors: Vec<String>,
    /// Why it is part of this: asked for, or what it built on.
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub elements: Vec<Affected>,
    /// Edges that go entirely, and edges that return to an earlier version.
    pub edges: usize,
    /// What builds on the versions to be popped. Not empty means the revert is refused
    /// unless forced; forced, these are in `elements` instead.
    pub blocked_by: Vec<String>,
}

#[derive(Default)]
struct Work {
    /// Element → the version it returns to, and why.
    elements: BTreeMap<String, (u32, String)>,
    /// Edge → the version it returns to.
    edges: BTreeMap<String, u32>,
    blocked: Vec<String>,
}

impl Work {
    fn element(&mut self, id: &str, back: u32, why: &str) -> bool {
        match self.elements.get(id) {
            Some((b, _)) if *b <= back => false,
            _ => {
                self.elements
                    .insert(id.to_string(), (back, why.to_string()));
                true
            }
        }
    }

    fn edge(&mut self, id: &str, back: u32) -> bool {
        match self.edges.get(id) {
            Some(b) if *b <= back => false,
            _ => {
                self.edges.insert(id.to_string(), back);
                true
            }
        }
    }
}

fn label(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT kind || ' ' || type || coalesce(' \"' || substr(value, 1, 60) || '\"', '')
           FROM element_current WHERE id = ?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_else(|_| id.to_string())
}

/// Work out everything a revert takes with it.
fn compute(conn: &Connection, what: &What, force: bool) -> Result<Work> {
    let mut w = Work::default();
    match what {
        What::Event(seq) => {
            let mut stmt = conn.prepare(
                "SELECT element_id, min(ver) - 1 FROM element_version WHERE event_seq = ?1
                  GROUP BY element_id",
            )?;
            for row in stmt.query_map([seq], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                let (id, back) = row?;
                w.element(&id, back, "asked for");
            }
            let mut stmt = conn.prepare(
                "SELECT edge_id, min(ver) - 1 FROM edge_version WHERE event_seq = ?1
                  GROUP BY edge_id",
            )?;
            for row in stmt.query_map([seq], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
                let (id, back) = row?;
                w.edge(&id, back);
            }
            if w.elements.is_empty() && w.edges.is_empty() {
                return Err(Error::Invalid(format!(
                    "change #{seq} wrote nothing to the graph that is still there"
                )));
            }
        }
        What::Element { id, to } => {
            let top: Option<u32> = conn.query_row(
                "SELECT max(ver) FROM element_version WHERE element_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            let top = top.ok_or_else(|| Error::Invalid(format!("no element {id}")))?;
            if *to >= top {
                return Err(Error::Invalid(format!(
                    "{id} is at version {top}; there is nothing above version {to} to take back"
                )));
            }
            w.element(id, *to, "asked for");
        }
    }

    // Until nothing more follows from what is already going.
    loop {
        let mut grew = false;
        let going: Vec<(String, u32)> = w
            .elements
            .iter()
            .map(|(id, (back, _))| (id.clone(), *back))
            .collect();
        for (id, back) in going {
            let name = label(conn, &id);
            if back == 0 {
                // It leaves the graph: so does every edge at it, and whatever was made
                // pointing at it.
                let mut stmt = conn.prepare(
                    "SELECT g.id, g.from_id,
                            (SELECT event_seq FROM edge_version WHERE edge_id = g.id AND ver = 1)
                              = (SELECT event_seq FROM element_version
                                  WHERE element_id = g.from_id AND ver = 1)
                       FROM edge g WHERE g.from_id = ?1 OR g.to_id = ?1",
                )?;
                let rows: Vec<(String, String, Option<bool>)> = stmt
                    .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<std::result::Result<_, _>>()?;
                for (edge, from, born_together) in rows {
                    grew |= w.edge(&edge, 0);
                    if from != id && born_together == Some(true) {
                        grew |= w.element(&from, 0, &format!("was made about {name}"));
                    }
                }
            }
            // Edges that pinned a version that is going: something builds on it.
            let mut stmt = conn.prepare(
                "SELECT v.edge_id, min(v.ver), g.from_id, v.event_seq
                   FROM edge_version v JOIN edge g ON g.id = v.edge_id
                  WHERE g.to_id = ?1 AND v.pinned_ver > ?2 AND v.status = 'active'
                  GROUP BY v.edge_id",
            )?;
            let pins: Vec<(String, u32, String, Option<i64>)> = stmt
                .query_map(params![id, back], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<std::result::Result<_, _>>()?;
            for (edge, ver, from, event) in pins {
                if w.edges.get(&edge).is_some_and(|b| *b < ver) {
                    continue;
                }
                if !force {
                    let line = format!("{} builds on {name}", label(conn, &from));
                    if !w.blocked.contains(&line) {
                        w.blocked.push(line);
                    }
                    continue;
                }
                grew |= w.edge(&edge, ver - 1);
                // The versions of the pinning element written with that edge go too.
                let first: Option<u32> = conn.query_row(
                    "SELECT min(ver) FROM element_version WHERE element_id = ?1 AND event_seq = ?2",
                    params![from, event],
                    |r| r.get(0),
                )?;
                if let Some(first) = first {
                    grew |= w.element(&from, first - 1, &format!("built on {name}"));
                }
            }
        }
        // A claim left without a source cannot stay.
        let leaving: Vec<String> = w
            .edges
            .iter()
            .filter(|(_, back)| **back == 0)
            .map(|(id, _)| id.clone())
            .collect();
        for edge in leaving {
            let found: Option<(String, String)> = conn
                .query_row(
                    "SELECT g.from_id, e.kind FROM edge g JOIN element e ON e.id = g.from_id
                       JOIN edge_current c ON c.id = g.id
                      WHERE g.id = ?1 AND c.role = 'source'",
                    [&edge],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            let Some((from, kind)) = found else { continue };
            if kind != "event" && kind != "observation" {
                continue;
            }
            if w.elements.get(&from).is_some_and(|(b, _)| *b == 0) {
                continue;
            }
            let mut stmt =
                conn.prepare("SELECT id FROM edge_current WHERE from_id = ?1 AND role = 'source'")?;
            let sources: Vec<String> = stmt
                .query_map([&from], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            if sources.iter().all(|s| w.edges.get(s) == Some(&0)) {
                grew |= w.element(&from, 0, "would be left without a source");
            }
        }
        if !grew {
            break;
        }
    }
    Ok(w)
}

fn describe(conn: &Connection, w: &Work) -> Result<Plan> {
    let mut elements = Vec::new();
    for (id, (back, why)) in &w.elements {
        let (kind, type_): (String, String) =
            conn.query_row("SELECT kind, type FROM element WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT a.kind || ':' || a.identity ||
                    CASE WHEN a.version = '' THEN '' ELSE '@' || a.version END
               FROM element_version v JOIN author a ON a.id = v.author_id
              WHERE v.element_id = ?1 AND v.ver > ?2 ORDER BY 1",
        )?;
        let authors = stmt
            .query_map(params![id, back], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        elements.push(Affected {
            id: id.clone(),
            kind,
            type_,
            back_to: *back,
            authors,
            why: why.clone(),
        });
    }
    Ok(Plan {
        elements,
        edges: w.edges.len(),
        blocked_by: w.blocked.clone(),
    })
}

/// What a revert would take, without taking it.
pub fn plan(db: &Db, what: &What, force: bool) -> Result<Plan> {
    db.read(|conn| describe(conn, &compute(conn, what, force)?))
}

/// Take it back. Refused, with nothing changed, if something builds on what would go
/// and `force` is not set.
pub fn apply(
    db: &Db,
    what: What,
    force: bool,
    author: &Author,
    session: Option<&str>,
) -> Result<Plan> {
    let (author, session) = (author.clone(), session.map(str::to_string));
    db.write(move |tx| {
        let work = compute(tx, &what, force)?;
        let plan = describe(tx, &work)?;
        if !plan.blocked_by.is_empty() {
            return Err(Error::Invalid(format!(
                "not reverted: {}. Change it instead, or force the revert to take those along.",
                plan.blocked_by.join("; ")
            )));
        }
        for (edge, back) in &work.edges {
            tx.execute(
                "DELETE FROM edge_version WHERE edge_id = ?1 AND ver > ?2",
                params![edge, back],
            )?;
            if *back == 0 {
                tx.execute("DELETE FROM edge WHERE id = ?1", [edge])?;
            }
        }
        for (id, (back, _)) in &work.elements {
            tx.execute(
                "DELETE FROM element_version WHERE element_id = ?1 AND ver > ?2",
                params![id, back],
            )?;
            if *back == 0 {
                tx.execute("DELETE FROM element WHERE id = ?1", [id])?;
            }
        }
        let author_id = author_id(tx, &author)?;
        tx.execute(
            "INSERT INTO event(id, ts, session_id, author_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, 'revert', ?5)",
            params![
                uuid::Uuid::now_v7().to_string(),
                now_rfc3339(),
                session,
                author_id,
                json!({
                    "what": match &what {
                        What::Event(seq) => json!({ "event": seq }),
                        What::Element { id, to } => json!({ "element": id, "to": to }),
                    },
                    "forced": force,
                    "elements": plan.elements.iter().map(|e| json!([e.id, e.back_to])).collect::<Vec<_>>(),
                })
                .to_string()
            ],
        )?;
        Ok(plan)
    })
}
