//! What a person does to the graph by hand
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)):
//! the operations behind the graph view's buttons.
//!
//! Each is a small [`Batch`](super::Batch) recorded as the analyst's, through the same
//! write path the agent's tools use, so the rules are the same: a claim needs a source,
//! nothing is overwritten, saying what is already said writes nothing. Kept apart from
//! the window so that it is tested without one.

use serde_json::{Value, json};

use super::revert::{self, What};
use super::{Applied, Batch, Change, Content, Kind, Link, Put, Target, apply, query};
use crate::db::Db;
use crate::error::{Error, Result};
use crate::manifest::Author;

/// Where a person is in the case when they make a change: the session, and its latest
/// entry — which is what a relation they state themselves is sourced to.
#[derive(Debug, Clone)]
pub struct At {
    pub session: String,
    pub entry: u64,
}

impl At {
    pub fn new(session: &str, entry: u64) -> At {
        At {
            session: session.to_string(),
            entry,
        }
    }

    fn batch(&self) -> Batch {
        Batch {
            session: Some(self.session.clone()),
            entry: Some(self.entry),
            ..Batch::new(Author::analyst())
        }
    }

    /// The session entry as a source: "the analyst said so, here".
    fn source(&self) -> Link {
        Link::source(&format!("session:{}#entry={}", self.session, self.entry))
    }
}

/// The values of a scale, weakest first (`confidence` or `reliability`).
pub fn scale(db: &Db, name: &str) -> Result<Vec<String>> {
    db.read(|conn| {
        let mut stmt =
            conn.prepare("SELECT name FROM scale_value WHERE scale = ?1 ORDER BY ord")?;
        let rows = stmt
            .query_map([name], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

/// The types of a kind that the vocabulary knows, for suggestions.
pub fn known_types(db: &Db, kind: Kind) -> Result<Vec<String>> {
    db.read(|conn| {
        let mut stmt = conn.prepare("SELECT name FROM known_type WHERE kind = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map([kind.as_str()], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })
}

fn one(applied: Applied) -> Result<String> {
    applied
        .ids
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("nothing was added".into()))
}

/// Add an entity, or find the one that is already there: the same value is the same
/// node. Returns its id.
pub fn add_entity(db: &Db, at: &At, type_: &str, value: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(Error::Invalid("an entity needs a value".into()));
    }
    one(apply(db, at.batch().put(Put::entity(type_, value)))?)
}

/// State a relation between two elements: an observation of `type_` that links them by
/// the given roles, sourced to the session entry it was made in. Returns the claim's id.
pub fn relate(
    db: &Db,
    at: &At,
    from: &str,
    to: &str,
    type_: &str,
    from_role: &str,
    to_role: &str,
) -> Result<String> {
    if from == to {
        return Err(Error::Invalid("a relation needs two different ends".into()));
    }
    if type_.trim().is_empty() {
        return Err(Error::Invalid("a relation needs a type".into()));
    }
    let (from_role, to_role) = (
        Some(from_role.trim())
            .filter(|r| !r.is_empty())
            .unwrap_or("src"),
        Some(to_role.trim())
            .filter(|r| !r.is_empty())
            .unwrap_or("dst"),
    );
    if from_role.eq_ignore_ascii_case(to_role) {
        return Err(Error::Invalid(
            "the two ends need different roles, or the relation cannot say which is which".into(),
        ));
    }
    let claim = Put {
        links: vec![
            Link::new(from_role, Target::Id(from.to_string())),
            Link::new(to_role, Target::Id(to.to_string())),
            at.source(),
        ],
        ..Put::new(Kind::Observation, type_)
    };
    one(apply(db, at.batch().put(claim))?)
}

fn changes(db: &Db, at: &At, ids: &[String], make: impl Fn(&str) -> Change) -> Result<Applied> {
    if ids.is_empty() {
        return Err(Error::Invalid("nothing is selected".into()));
    }
    let mut batch = at.batch();
    for id in ids {
        batch.changes.push(make(id));
    }
    apply(db, batch)
}

/// Set (or, with `None`, clear) the confidence of each.
pub fn set_confidence(
    db: &Db,
    at: &At,
    ids: &[String],
    confidence: Option<&str>,
) -> Result<Applied> {
    let value = confidence
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    changes(db, at, ids, |id| Change {
        id: id.to_string(),
        confidence: Some(value.clone()),
        ..Change::default()
    })
}

/// Set the status of each: `active`, `conflicting` or `soft-deleted`.
pub fn set_status(db: &Db, at: &At, ids: &[String], status: &str) -> Result<Applied> {
    changes(db, at, ids, |id| Change {
        id: id.to_string(),
        status: Some(status.to_string()),
        ..Change::default()
    })
}

/// Add and remove tags on each.
pub fn tag(db: &Db, at: &At, ids: &[String], add: &[String], remove: &[String]) -> Result<Applied> {
    changes(db, at, ids, |id| Change {
        id: id.to_string(),
        add_tags: add.to_vec(),
        remove_tags: remove.to_vec(),
        ..Change::default()
    })
}

pub const HYPOTHESIS_STATUSES: [&str; 3] = ["open", "supported", "rejected"];

/// Move a hypothesis between open, supported and rejected. A status change is a change:
/// a new version, and the reason belongs in a note.
pub fn set_hypothesis_status(db: &Db, at: &At, id: &str, status: &str) -> Result<Applied> {
    if !HYPOTHESIS_STATUSES.contains(&status) {
        return Err(Error::Invalid(format!(
            "a hypothesis is open, supported or rejected -- not `{status}`"
        )));
    }
    match query::get(db, id)? {
        Some(e) if e.type_ == "analysis.hypothesis" => {}
        Some(e) => {
            return Err(Error::Invalid(format!(
                "{id} is a {}, not a hypothesis",
                e.type_
            )));
        }
        None => return Err(Error::Invalid(format!("no element {id}"))),
    }
    let mut set = serde_json::Map::new();
    set.insert("hypothesis_status".into(), json!(status));
    apply(
        db,
        at.batch().change(Change {
            id: id.to_string(),
            set_props: set,
            ..Change::default()
        }),
    )
}

/// What taking the latest version of an element back would do, without doing it.
pub fn undo_preview(db: &Db, id: &str) -> Result<revert::Plan> {
    revert::plan(db, &undo_target(db, id)?, false)
}

/// Take the latest version of an element back. Refused, saying why, if something builds
/// on it.
pub fn undo(db: &Db, at: &At, id: &str) -> Result<revert::Plan> {
    revert::apply(
        db,
        undo_target(db, id)?,
        false,
        &Author::analyst(),
        Some(&at.session),
    )
}

fn undo_target(db: &Db, id: &str) -> Result<What> {
    let e = query::get(db, id)?.ok_or_else(|| Error::Invalid(format!("no element {id}")))?;
    if e.ver <= 1 {
        return Err(Error::Invalid(format!(
            "{id} is still at its first version; to remove it, soft-delete it"
        )));
    }
    Ok(What::Element {
        id: id.to_string(),
        to: e.ver - 1,
    })
}

/// A note about what is selected, in the analyst's words.
pub fn add_note(db: &Db, at: &At, about: &[String], text: &str) -> Result<String> {
    if about.is_empty() {
        return Err(Error::Invalid("a note needs something to be about".into()));
    }
    note(db, at, about, text)
}

/// A note, about the given elements or — with none — about nothing in particular: a thing
/// to come back to, put where it was thought of.
pub fn note(db: &Db, at: &At, about: &[String], text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err(Error::Invalid("a note needs text".into()));
    }
    one(apply(
        db,
        at.batch().put(Put {
            content: Content::value(text.trim()),
            links: about
                .iter()
                .map(|id| Link::new("about", Target::Id(id.clone())))
                .collect(),
            ..Put::new(Kind::Annotation, "analysis.note")
        }),
    )?)
}

/// What kind of annotation a person can add by hand: the vocabulary's `analysis.*` types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    /// A candidate explanation; starts open.
    Hypothesis,
    /// An assessment of something, which may support or refute a hypothesis.
    Judgement,
    /// Commentary, a question, a to-do.
    Note,
    /// These belong together.
    Cluster,
    /// These two entities are one thing.
    SameAs,
    /// A search that found nothing, and where it looked.
    SearchNegative,
}

impl AnnotationKind {
    pub const ALL: [AnnotationKind; 6] = [
        AnnotationKind::Hypothesis,
        AnnotationKind::Judgement,
        AnnotationKind::Note,
        AnnotationKind::Cluster,
        AnnotationKind::SameAs,
        AnnotationKind::SearchNegative,
    ];

    pub fn type_name(self) -> &'static str {
        match self {
            AnnotationKind::Hypothesis => "analysis.hypothesis",
            AnnotationKind::Judgement => "analysis.judgement",
            AnnotationKind::Note => "analysis.note",
            AnnotationKind::Cluster => "analysis.cluster",
            AnnotationKind::SameAs => "analysis.same_as",
            AnnotationKind::SearchNegative => "analysis.search_negative",
        }
    }
}

/// An annotation to add: what it says, and what it is about. Which parts are needed depends
/// on the kind (see [`annotate`]).
#[derive(Debug, Clone, Default)]
pub struct Annotate<'a> {
    pub text: &'a str,
    /// What it comments on, or — for `same_as` — the two entities that are one.
    pub about: &'a [String],
    /// Hypotheses a judgement supports, and hypotheses it refutes.
    pub supports: &'a [String],
    pub refutes: &'a [String],
    /// The members of a cluster.
    pub members: &'a [String],
    /// Where a search looked (`search_negative`): URIs or names.
    pub sources: &'a [String],
    pub confidence: Option<&'a str>,
}

/// Add an annotation in the analyst's name, the same way the agent's `graph_annotate` does.
///
/// * **hypothesis**: needs text; starts `open`; about anything or nothing.
/// * **judgement**: needs text and something it is *about* (pinned to that version of it, so
///   it is flagged when it moves on); may `supports` or `refutes` hypotheses and carry a
///   confidence.
/// * **note**: needs text; about anything or nothing.
/// * **cluster**: needs two or more `members`; the text, if any, names it.
/// * **same_as**: needs exactly two entities in `about`; the text, if any, says why.
/// * **search_negative**: needs text (what was looked for); `sources` say where.
pub fn annotate(db: &Db, at: &At, kind: AnnotationKind, a: &Annotate) -> Result<String> {
    let text = a.text.trim();
    let invalid = |m: &str| Err(Error::Invalid(m.to_string()));
    let distinct = |ids: &[String]| ids.iter().collect::<std::collections::BTreeSet<_>>().len();
    match kind {
        AnnotationKind::Hypothesis | AnnotationKind::Note | AnnotationKind::SearchNegative
            if text.is_empty() =>
        {
            return invalid("it needs text: what it says");
        }
        AnnotationKind::Judgement if text.is_empty() => {
            return invalid("an assessment needs text: what you conclude");
        }
        AnnotationKind::Judgement if a.about.is_empty() => {
            return invalid("an assessment is about something: select what it is about first");
        }
        AnnotationKind::Cluster if distinct(a.members) < 2 => {
            return invalid("a cluster needs two or more members: select them first");
        }
        AnnotationKind::SameAs if distinct(a.about) != 2 => {
            return invalid("\"same as\" is said of exactly two entities: select the two");
        }
        _ => {}
    }
    let mut put = Put::new(Kind::Annotation, kind.type_name());
    put.content = Content {
        value: (!text.is_empty()).then(|| text.to_string()),
        confidence: a.confidence.map(str::to_string),
        ..Content::default()
    };
    if kind == AnnotationKind::Hypothesis {
        put.content
            .props
            .insert("hypothesis_status".into(), json!("open"));
    }
    let mut link = |role: &str, ids: &[String], pin: bool| {
        for id in ids {
            let l = Link::new(role, Target::Id(id.clone()));
            put.links.push(if pin { l.pinned() } else { l });
        }
    };
    link("about", a.about, kind == AnnotationKind::Judgement);
    link("supports", a.supports, false);
    link("refutes", a.refutes, false);
    link("member", a.members, false);
    for uri in a.sources.iter().filter(|u| !u.trim().is_empty()) {
        put.links.push(Link::source(uri.trim()));
    }
    one(apply(db, at.batch().put(put))?)
}

/// The value as JSON text, for showing a property.
pub fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One claim that links several elements at once, each by the same role: the way the graph
/// says "these belong together in this way" without choosing which two are related. With
/// exactly two elements use [`relate`], which gives each its own role. Sourced, like every
/// claim a person states, to the session entry it was made in.
pub fn relate_many(db: &Db, at: &At, ids: &[String], type_: &str, role: &str) -> Result<String> {
    let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
    if distinct.len() < 2 {
        return Err(Error::Invalid(
            "a connection needs at least two different elements".into(),
        ));
    }
    if type_.trim().is_empty() {
        return Err(Error::Invalid("a connection needs a type".into()));
    }
    let role = Some(role.trim())
        .filter(|r| !r.is_empty())
        .unwrap_or("member");
    let mut links: Vec<Link> = distinct
        .into_iter()
        .map(|id| Link::new(role, Target::Id(id.clone())))
        .collect();
    links.push(at.source());
    one(apply(
        db,
        at.batch().put(Put {
            links,
            ..Put::new(Kind::Observation, type_)
        }),
    )?)
}

/// A type the vocabulary or the case knows, for offering while one is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeChoice {
    pub name: String,
    /// How many elements of the case have it.
    pub uses: usize,
    /// Other spellings that mean it (`ip` for `net.ipv4`).
    pub aliases: Vec<String>,
}

/// The known types of these kinds, and the ones the case uses, with their aliases.
pub fn type_choices(db: &Db, kinds: &[Kind]) -> Result<Vec<TypeChoice>> {
    let mut by_name: std::collections::BTreeMap<String, TypeChoice> = Default::default();
    let fresh = |name: &str| TypeChoice {
        name: name.to_string(),
        uses: 0,
        aliases: Vec::new(),
    };
    for kind in kinds {
        for name in known_types(db, *kind)? {
            by_name.entry(name.clone()).or_insert_with(|| fresh(&name));
        }
    }
    for t in query::types_in_use(db)? {
        if kinds.contains(&t.kind) {
            by_name
                .entry(t.type_.clone())
                .or_insert_with(|| fresh(&t.type_))
                .uses += t.count;
        }
    }
    let aliases: Vec<(String, String)> = db.read(|conn| {
        let mut stmt = conn.prepare("SELECT alias, name FROM type_alias ORDER BY alias")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })?;
    for (alias, name) in aliases {
        if let Some(c) = by_name.get_mut(&name) {
            c.aliases.push(alias);
        }
    }
    Ok(by_name.into_values().collect())
}

/// What to offer for `typed`: the types it begins, then those with a part that begins with
/// it (`ipv4` finds `net.ipv4`), then those that contain it, then those with an alias it
/// matches. Within a group the types the case uses most come first. Nothing is offered once
/// `typed` is exactly one of them.
pub fn suggest_types(choices: &[TypeChoice], typed: &str, limit: usize) -> Vec<String> {
    let typed = typed.trim().to_lowercase();
    if choices.iter().any(|c| c.name == typed) {
        return Vec::new();
    }
    let mut ranked: Vec<(u8, &TypeChoice)> = choices
        .iter()
        .filter_map(|c| {
            let name = c.name.as_str();
            let rank = if typed.is_empty() || name.starts_with(&typed) {
                0
            } else if name
                .split(['.', '_', '-'])
                .any(|part| part.starts_with(&typed))
            {
                1
            } else if name.contains(&typed) {
                2
            } else if c.aliases.iter().any(|a| a.contains(&typed)) {
                3
            } else {
                return None;
            };
            Some((rank, c))
        })
        .collect();
    ranked.sort_by(|a, b| {
        (a.0, std::cmp::Reverse(a.1.uses), &a.1.name).cmp(&(
            b.0,
            std::cmp::Reverse(b.1.uses),
            &b.1.name,
        ))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, c)| c.name.clone())
        .collect()
}
