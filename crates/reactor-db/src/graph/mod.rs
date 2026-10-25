//! The analysis graph
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)):
//! what was seen, what is related, and what was concluded.
//!
//! Five kinds of element. An *entity* is a thing that can appear in many statements; an
//! *event* happened; an *observation* holds; an *annotation* is someone's assessment;
//! an *external* points at data outside the graph. **Claims are nodes**: an event or
//! observation is an element with role edges to what it involves, so it can be sourced,
//! dated, questioned and contradicted. An edge says nothing on its own.
//!
//! **Nothing is overwritten.** Every change is a new version on the element's chain,
//! with who wrote it and when. The current state is the top of each chain.
//!
//! [`apply`] is the one way in. A batch is one transaction and one row in the change
//! log; saying again what the graph already holds writes nothing at all, which is what
//! makes a pull safe to repeat.

pub mod edit;
pub mod fields;
pub mod identity;
pub mod layout;
pub mod pull;
pub mod query;
pub mod revert;
pub mod views;
pub mod vocab;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::manifest::{Author, author_id};
use crate::time::now_rfc3339;

pub use query::{Edge, Element, Filter, Relation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Entity,
    Event,
    Observation,
    Annotation,
    External,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Entity => "entity",
            Kind::Event => "event",
            Kind::Observation => "observation",
            Kind::Annotation => "annotation",
            Kind::External => "external",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_lowercase().as_str() {
            "entity" => Kind::Entity,
            "event" => Kind::Event,
            "observation" => Kind::Observation,
            "annotation" => Kind::Annotation,
            "external" => Kind::External,
            _ => return None,
        })
    }

    /// An event or an observation: a statement from a source.
    pub fn is_claim(self) -> bool {
        matches!(self, Kind::Event | Kind::Observation)
    }
}

pub const STATUSES: [&str; 3] = ["active", "conflicting", "soft-deleted"];

/// Domain time: when it happened or held. Never when it was recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Time {
    /// An event's occurrence, UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// An observation's validity, UTC. Either end may be open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    /// The zone the source used, and whether it was `stated`, `assumed` or `unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz_certainty: Option<String>,
    /// The value as the source wrote it, so a wrong assumption can be recomputed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<String>,
}

/// What a version holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Content {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub props: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default = "active")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default, flatten)]
    pub time: Time,
}

fn active() -> String {
    "active".into()
}

impl Default for Content {
    fn default() -> Self {
        Content {
            value: None,
            props: Map::new(),
            tags: Vec::new(),
            status: active(),
            confidence: None,
            time: Time::default(),
        }
    }
}

impl Content {
    pub fn value(v: impl Into<String>) -> Content {
        Content {
            value: Some(v.into()),
            ..Content::default()
        }
    }

    /// Tags as they are stored: trimmed, lowercase, sorted, each once.
    fn tidy(mut self) -> Content {
        let mut tags: Vec<String> = self
            .tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        tags.sort();
        tags.dedup();
        self.tags = tags;
        self
    }
}

/// What an edge points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An element that exists.
    Id(String),
    /// An entity by what it is: found, or made.
    Entity {
        type_: String,
        value: String,
        scope: Option<String>,
    },
    /// An external by its URI: found, or made.
    External { uri: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub role: String,
    pub to: Target,
    /// Pin the target's current version: this edge is about the target as it is now.
    pub pin: bool,
    /// On a `source` edge: where in the external, and what was there.
    pub locator: Option<String>,
    pub fingerprint: Option<String>,
    /// The edge's id, when it is being restored (an import) rather than made.
    pub id: Option<String>,
}

impl Link {
    pub fn new(role: &str, to: Target) -> Link {
        Link {
            role: role.to_string(),
            to,
            pin: false,
            locator: None,
            fingerprint: None,
            id: None,
        }
    }

    pub fn entity(role: &str, type_: &str, value: &str) -> Link {
        Link::new(
            role,
            Target::Entity {
                type_: type_.to_string(),
                value: value.to_string(),
                scope: None,
            },
        )
    }

    pub fn source(uri: &str) -> Link {
        Link::new(
            "source",
            Target::External {
                uri: uri.to_string(),
            },
        )
    }

    pub fn at(mut self, locator: &str, fingerprint: Option<&str>) -> Link {
        self.locator = Some(locator.to_string());
        self.fingerprint = fingerprint.map(str::to_string);
        self
    }

    pub fn pinned(mut self) -> Link {
        self.pin = true;
        self
    }
}

/// An element to add, or to bring up to date if it is already there.
#[derive(Debug, Clone, PartialEq)]
pub struct Put {
    pub kind: Kind,
    pub type_: String,
    /// A known id: an element to update, or one whose id the caller derived (a pulled
    /// row). `None` derives it for an entity or an external and makes one otherwise.
    pub id: Option<String>,
    /// For an entity whose value only identifies it within a context.
    pub scope: Option<String>,
    pub content: Content,
    pub links: Vec<Link>,
    /// Who wrote it and when, when that is not this batch's author and now: a restored
    /// element keeps what it had.
    pub author: Option<Author>,
    pub recorded_at: Option<String>,
}

impl Put {
    pub fn new(kind: Kind, type_: &str) -> Put {
        Put {
            kind,
            type_: type_.to_string(),
            id: None,
            scope: None,
            content: Content::default(),
            links: Vec::new(),
            author: None,
            recorded_at: None,
        }
    }

    pub fn entity(type_: &str, value: &str) -> Put {
        let mut p = Put::new(Kind::Entity, type_);
        p.content.value = Some(value.to_string());
        p
    }

    pub fn link(mut self, link: Link) -> Put {
        self.links.push(link);
        self
    }
}

/// A change to an element that exists. Absent fields are left as they are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub id: String,
    pub status: Option<String>,
    /// `Some(None)` clears it.
    pub confidence: Option<Option<String>>,
    pub value: Option<Option<String>>,
    pub set_props: Map<String, Value>,
    pub remove_props: Vec<String>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub time: Option<Time>,
    pub links: Vec<Link>,
    /// Edges to take away: `(role, target id)`.
    pub unlink: Vec<(String, String)>,
}

/// Everything written together: one transaction, one change-log row, one author.
#[derive(Debug, Clone)]
pub struct Batch {
    pub author: Author,
    /// Who asked for it, when that is not the author (an agent acting on a prompt).
    pub on_behalf_of: Option<Author>,
    pub session: Option<String>,
    pub entry: Option<u64>,
    /// The change-log type: `graph` for a manual change, `pull` for a pull.
    pub kind: String,
    pub payload: Value,
    pub puts: Vec<Put>,
    pub changes: Vec<Change>,
}

impl Batch {
    pub fn new(author: Author) -> Batch {
        Batch {
            author,
            on_behalf_of: None,
            session: None,
            entry: None,
            kind: "graph".into(),
            payload: Value::Object(Map::new()),
            puts: Vec::new(),
            changes: Vec::new(),
        }
    }

    pub fn put(mut self, put: Put) -> Batch {
        self.puts.push(put);
        self
    }

    pub fn change(mut self, change: Change) -> Batch {
        self.changes.push(change);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// The change-log row, or `None` when the batch changed nothing.
    pub event_seq: Option<i64>,
    /// The id of each put, in order.
    pub ids: Vec<String>,
    /// Elements that did not exist before.
    pub created: usize,
    /// New versions written besides: of elements that already existed, and of edges.
    pub versioned: usize,
}

/// Write a batch. All of it, or — if any part breaks a rule — none of it.
pub fn apply(db: &Db, batch: Batch) -> Result<Applied> {
    db.write(move |tx| {
        let mut w = Writer {
            tx,
            batch: &batch,
            event: None,
            authors: None,
            now: now_rfc3339(),
            own: None,
            applied: Applied::default(),
        };
        let mut claims = Vec::new();
        for put in &batch.puts {
            w.own = match (&put.author, &put.recorded_at) {
                (None, None) => None,
                (author, at) => {
                    let a = match author {
                        Some(a) => author_id(tx, a)?,
                        None => w.authors()?.0,
                    };
                    Some((a, at.clone().unwrap_or_else(|| w.now.clone())))
                }
            };
            let (id, kind) = w.put(put)?;
            for link in &put.links {
                w.link(&id, link)?;
            }
            w.own = None;
            if kind.is_claim() {
                claims.push(id.clone());
            }
            w.applied.ids.push(id);
        }
        for change in &batch.changes {
            if w.change(change)?.is_claim() {
                claims.push(change.id.clone());
            }
        }
        // The source rule: checked once the whole batch is in, so a claim and its
        // source can arrive in either order.
        for id in &claims {
            let sourced: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM edge_current WHERE from_id = ?1 AND role = 'source')",
                [id],
                |r| r.get(0),
            )?;
            if !sourced {
                return Err(Error::Invalid(format!(
                    "{id}: an event or an observation needs a source -- what an analyst or agent concludes is an annotation"
                )));
            }
        }
        w.applied.event_seq = w.event;
        Ok(w.applied)
    })
}

struct Writer<'a> {
    tx: &'a Transaction<'a>,
    batch: &'a Batch,
    /// The change-log row, written on the first thing that actually changes.
    event: Option<i64>,
    authors: Option<(i64, Option<i64>)>,
    now: String,
    /// While one `Put` is written: its own author and record time, if it has them.
    own: Option<(i64, String)>,
    applied: Applied,
}

/// Canonical JSON: keys sorted, so equal values are equal bytes.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn check_status(status: &str) -> Result<()> {
    if STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "`{status}` is not a status (active, conflicting, soft-deleted)"
        )))
    }
}

impl Writer<'_> {
    /// Who wrote what is being written now, and when.
    fn attribution(&mut self) -> Result<(i64, Option<i64>, String)> {
        let (author, behalf) = self.authors()?;
        Ok(match &self.own {
            Some((a, at)) => (*a, None, at.clone()),
            None => (author, behalf, self.now.clone()),
        })
    }

    fn authors(&mut self) -> Result<(i64, Option<i64>)> {
        if let Some(a) = self.authors {
            return Ok(a);
        }
        let a = (
            author_id(self.tx, &self.batch.author)?,
            match &self.batch.on_behalf_of {
                Some(b) => Some(author_id(self.tx, b)?),
                None => None,
            },
        );
        self.authors = Some(a);
        Ok(a)
    }

    fn event(&mut self) -> Result<i64> {
        if let Some(seq) = self.event {
            return Ok(seq);
        }
        let (author, _) = self.authors()?;
        self.tx.execute(
            "INSERT INTO event(id, ts, session_id, entry_id, author_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                uuid::Uuid::now_v7().to_string(),
                self.now,
                self.batch.session,
                self.batch.entry.map(|e| e as i64),
                author,
                self.batch.kind,
                canonical(&self.batch.payload).to_string()
            ],
        )?;
        let seq = self.tx.last_insert_rowid();
        self.event = Some(seq);
        Ok(seq)
    }

    fn insert_version(&mut self, id: &str, ver: i64, c: &Content) -> Result<()> {
        let event = self.event()?;
        let (author, behalf, at) = self.attribution()?;
        self.tx.execute(
            "INSERT INTO element_version(element_id, ver, recorded_at, author_id, on_behalf_of,
               event_seq, status, confidence, value, props, tags, occurred_at, duration_ms,
               valid_from, valid_to, tz, tz_certainty, time_raw, time_precision)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                id,
                ver,
                at,
                author,
                behalf,
                event,
                c.status,
                c.confidence,
                c.value,
                canonical(&Value::Object(c.props.clone())).to_string(),
                serde_json::to_string(&c.tags).unwrap(),
                c.time.occurred_at,
                c.time.duration_ms,
                c.time.valid_from,
                c.time.valid_to,
                c.time.tz,
                c.time.tz_certainty,
                c.time.raw,
                c.time.precision
            ],
        )?;
        Ok(())
    }

    /// Resolve what an entity put means: its stored type, its normalized content, and
    /// the id its key gives it.
    fn entity(
        &self,
        type_: &str,
        mut content: Content,
        scope: Option<&str>,
        id: Option<&str>,
    ) -> Result<(String, String, Content)> {
        let mut type_ = identity::canonical_type(self.tx, type_)?;
        let rule = identity::type_rule(self.tx, &type_)?;
        if let Some(scope) = scope.map(str::trim).filter(|s| !s.is_empty()) {
            content
                .props
                .insert("scope".into(), Value::String(scope.to_string()));
        }
        let scope = content
            .props
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(v) = content.value.take() {
            let (t, v) = identity::normalize(rule.normalizer.as_deref(), &type_, &v);
            type_ = t;
            content.value = Some(v).filter(|v| !v.is_empty());
        }
        let mut parts: Vec<String> = vec![type_.clone()];
        let mut keyed = true;
        for field in &rule.key_fields {
            let part = if field == "value" {
                content.value.clone()
            } else {
                content.props.get(field).map(|v| match v {
                    Value::String(s) => s.trim().to_string(),
                    other => other.to_string(),
                })
            };
            match part.filter(|p| !p.is_empty()) {
                Some(p) => parts.push(format!("{field}={p}")),
                None => keyed = false,
            }
        }
        parts.push(format!("scope={scope}"));
        let derived = keyed.then(|| {
            identity::derived_id(
                Kind::Entity,
                &parts.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        });
        let id = match (id, derived) {
            // Naming an entity by id while giving it a different key would move it.
            (Some(given), Some(d)) if given != d => {
                return Err(Error::Invalid(format!(
                    "{given}: an entity's key fields never change -- a different value is a different entity"
                )));
            }
            (Some(given), _) => given.to_string(),
            (None, Some(d)) => d,
            // Typed in by hand with nothing to key it on.
            (None, None) => identity::random_id(Kind::Entity),
        };
        Ok((id, type_, content))
    }

    fn put(&mut self, put: &Put) -> Result<(String, Kind)> {
        check_status(&put.content.status)?;
        let mut content = put.content.clone().tidy();
        fields::fold(self.tx, &mut content.props)?;
        let (id, type_, content) = match put.kind {
            Kind::Entity => {
                self.entity(&put.type_, content, put.scope.as_deref(), put.id.as_deref())?
            }
            Kind::External => {
                let uri = identity::canonical_uri(content.value.as_deref().unwrap_or(""))?;
                let id = identity::derived_id(Kind::External, &[&uri]);
                let type_ = identity::canonical_type(self.tx, &put.type_)?;
                (
                    id,
                    type_,
                    Content {
                        value: Some(uri),
                        ..content
                    },
                )
            }
            kind => (
                put.id.clone().unwrap_or_else(|| identity::random_id(kind)),
                identity::canonical_type(self.tx, &put.type_)?,
                content,
            ),
        };
        self.upsert(&id, put.kind, &type_, &content)?;
        Ok((id, put.kind))
    }

    /// Make the element, or give it a new version, or — if it already says this — do
    /// nothing.
    fn upsert(&mut self, id: &str, kind: Kind, type_: &str, content: &Content) -> Result<()> {
        let existing: Option<(String, String)> = self
            .tx
            .query_row("SELECT kind, type FROM element WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        match existing {
            None => {
                self.tx.execute(
                    "INSERT INTO element(id, kind, type) VALUES (?1, ?2, ?3)",
                    params![id, kind.as_str(), type_],
                )?;
                self.insert_version(id, 1, content)?;
                self.applied.created += 1;
            }
            Some((k, t)) => {
                if k != kind.as_str() || t != type_ {
                    return Err(Error::Invalid(format!(
                        "{id} is a {k} of type {t}; it cannot become a {} of type {type_}",
                        kind.as_str()
                    )));
                }
                let (ver, current) = query::current_content(self.tx, id)?;
                if current != *content {
                    self.insert_version(id, ver + 1, content)?;
                    self.applied.versioned += 1;
                }
            }
        }
        Ok(())
    }

    /// The id a target stands for, creating the entity or external it names.
    fn resolve(&mut self, target: &Target) -> Result<String> {
        match target {
            Target::Id(id) => {
                let exists: bool = self.tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM element WHERE id = ?1)",
                    [id],
                    |r| r.get(0),
                )?;
                if !exists {
                    return Err(Error::Invalid(format!("no element {id}")));
                }
                Ok(id.clone())
            }
            Target::Entity {
                type_,
                value,
                scope,
            } => {
                let (id, type_, content) =
                    self.entity(type_, Content::value(value), scope.as_deref(), None)?;
                if content.value.is_none() {
                    return Err(Error::Invalid("an entity needs a value".into()));
                }
                // Named in passing: make it if it is new, and leave it alone if not.
                let exists: bool = self.tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM element WHERE id = ?1)",
                    [&id],
                    |r| r.get(0),
                )?;
                if !exists {
                    self.upsert(&id, Kind::Entity, &type_, &content)?;
                }
                Ok(id)
            }
            Target::External { uri } => {
                let uri = identity::canonical_uri(uri)?;
                let id = identity::derived_id(Kind::External, &[&uri]);
                let exists: bool = self.tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM element WHERE id = ?1)",
                    [&id],
                    |r| r.get(0),
                )?;
                if !exists {
                    let scheme = match uri.split_once(':') {
                        Some((s, _)) if s.chars().all(|c| c.is_ascii_alphanumeric()) => s,
                        _ => "file",
                    };
                    let type_ = format!("external.{}", scheme.to_lowercase());
                    self.upsert(&id, Kind::External, &type_, &Content::value(uri))?;
                }
                Ok(id)
            }
        }
    }

    fn link(&mut self, from: &str, link: &Link) -> Result<()> {
        let role = link.role.trim().to_lowercase();
        if role.is_empty() {
            return Err(Error::Invalid("an edge needs a role".into()));
        }
        let to = self.resolve(&link.to)?;
        let pinned: Option<i64> = if link.pin {
            Some(query::current_content(self.tx, &to)?.0)
        } else {
            None
        };
        // The edge between these two with this role, if there is one.
        let found: Option<EdgeTop> = self
            .tx
            .query_row(
                "SELECT g.id, v.ver, v.status, v.pinned_ver, v.locator, v.fingerprint
                       FROM edge g JOIN edge_version v ON v.edge_id = g.id
                      WHERE g.from_id = ?1 AND g.to_id = ?2 AND v.role = ?3
                        AND v.ver = (SELECT max(ver) FROM edge_version WHERE edge_id = g.id)",
                params![from, to, role],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let (edge, ver) = match found {
            Some((_, _, status, pin, loc, fp))
                if status == "active"
                    && pin == pinned
                    && loc == link.locator
                    && fp == link.fingerprint =>
            {
                return Ok(());
            }
            Some((id, ver, ..)) => {
                self.applied.versioned += 1;
                (id, ver + 1)
            }
            None => {
                let n: i64 = self.tx.query_row(
                    "SELECT count(*) FROM edge WHERE from_id = ?1 AND to_id = ?2",
                    params![from, to],
                    |r| r.get(0),
                )?;
                let id = link
                    .id
                    .clone()
                    .unwrap_or_else(|| edge_id(from, &to, &role, n));
                self.tx.execute(
                    "INSERT INTO edge(id, from_id, to_id) VALUES (?1, ?2, ?3)",
                    params![id, from, to],
                )?;
                self.applied.versioned += 1;
                (id, 1)
            }
        };
        self.edge_version(&edge, ver, &role, "active", pinned, link)
    }

    fn edge_version(
        &mut self,
        edge: &str,
        ver: i64,
        role: &str,
        status: &str,
        pinned: Option<i64>,
        link: &Link,
    ) -> Result<()> {
        let event = self.event()?;
        let (author, _, at) = self.attribution()?;
        self.tx.execute(
            "INSERT INTO edge_version(edge_id, ver, role, status, pinned_ver, locator,
               fingerprint, recorded_at, author_id, event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                edge,
                ver,
                role,
                status,
                pinned,
                link.locator,
                link.fingerprint,
                at,
                author,
                event
            ],
        )?;
        Ok(())
    }

    fn change(&mut self, change: &Change) -> Result<Kind> {
        let kind: Option<String> = self
            .tx
            .query_row(
                "SELECT kind FROM element WHERE id = ?1",
                [&change.id],
                |r| r.get(0),
            )
            .optional()?;
        let kind = kind
            .as_deref()
            .and_then(Kind::parse)
            .ok_or_else(|| Error::Invalid(format!("no element {}", change.id)))?;
        let (ver, current) = query::current_content(self.tx, &change.id)?;
        let mut next = current.clone();
        if let Some(status) = &change.status {
            check_status(status)?;
            next.status = status.clone();
        }
        if let Some(c) = &change.confidence {
            next.confidence = c.clone();
        }
        if let Some(v) = &change.value {
            if matches!(kind, Kind::Entity | Kind::External) && *v != current.value {
                return Err(Error::Invalid(format!(
                    "{}: an {}'s value is its identity and never changes -- a different value is a different element",
                    change.id,
                    kind.as_str()
                )));
            }
            next.value = v.clone();
        }
        if kind == Kind::Entity {
            let type_: String = self.tx.query_row(
                "SELECT type FROM element WHERE id = ?1",
                [&change.id],
                |r| r.get(0),
            )?;
            let keys = identity::type_rule(self.tx, &type_)?.key_fields;
            let touched = change
                .set_props
                .keys()
                .chain(change.remove_props.iter())
                .find(|k| *k == "scope" || keys.contains(k));
            if let Some(k) = touched {
                return Err(Error::Invalid(format!(
                    "{}: `{k}` is a key field and never changes -- a different key is a different entity",
                    change.id
                )));
            }
        }
        let mut set = change.set_props.clone();
        fields::fold(self.tx, &mut set)?;
        for (k, v) in set {
            next.props.insert(k, v);
        }
        for k in &change.remove_props {
            next.props.remove(&fields::fold_name(self.tx, k)?);
        }
        next.tags.extend(change.add_tags.iter().cloned());
        let gone: Vec<String> = change
            .remove_tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .collect();
        next.tags
            .retain(|t| !gone.contains(&t.trim().to_lowercase()));
        if let Some(t) = &change.time {
            next.time = t.clone();
        }
        let next = next.tidy();
        if next != current {
            self.insert_version(&change.id, ver + 1, &next)?;
            self.applied.versioned += 1;
        }
        for link in &change.links {
            self.link(&change.id, link)?;
        }
        for (role, to) in &change.unlink {
            let role = role.trim().to_lowercase();
            let found: Option<(String, i64)> = self
                .tx
                .query_row(
                    "SELECT id, ver FROM edge_current WHERE from_id = ?1 AND to_id = ?2 AND role = ?3",
                    params![change.id, to, role],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((edge, ver)) = found else {
                return Err(Error::Invalid(format!(
                    "{}: no `{role}` edge to {to}",
                    change.id
                )));
            };
            let blank = Link::new(&role, Target::Id(to.clone()));
            self.edge_version(&edge, ver + 1, &role, "removed", None, &blank)?;
            self.applied.versioned += 1;
        }
        Ok(kind)
    }
}

/// The top of an edge's chain: id, version, status, pin, locator, fingerprint.
type EdgeTop = (
    String,
    i64,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// An edge's id: the same two ends and role always give the same edge. `n` tells apart
/// a second edge between the same pair.
fn edge_id(from: &str, to: &str, role: &str, n: i64) -> String {
    let n = n.to_string();
    let id = identity::derived_id(Kind::Entity, &["edge", from, to, role, &n]);
    id.replacen("entity--", "edge--", 1)
}
