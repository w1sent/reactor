//! The case's graph, for the agent
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! Six tools over `reactor-db`: write elements (what a source says, and what you conclude),
//! query, pull rows of a source in, take a mistake back, define a type, and register a file.
//! Reading by reference is `case_get`. Each is a thin skin:
//! the rules — a claim needs a source, an entity's key never changes, nothing is
//! overwritten — are the database's, and a refusal comes back as the text to act on.
//!
//! They are advertised only while the session is attached to the case's graph. With the
//! *Case graph* switch off the session neither reads nor writes it, and a stale call is
//! told why (ADR-0030).

use reactor_context::settings::Settings;
use reactor_db::artifact;
use reactor_db::graph::fields;
use reactor_db::graph::pull::{self, Request};
use reactor_db::graph::revert::{self, What};
use reactor_db::graph::vocab::{self, Define};
use reactor_db::graph::{
    self, Batch, Change, Content, Element, Filter, Kind, Link, Put, Target, query,
};
use reactor_db::manifest::Author;
use reactor_db::reference;
use reactor_db::time::parse_source_time;
use reactor_db::{Case, Db};
use serde_json::{Map, Value, json};

use super::{BoxFut, Tool, ToolCtx, ToolOutput, str_arg, usize_arg};
use crate::context::SessionState;
use crate::llm::ToolSpec;

const OFF: &str = "this session is detached from the case's graph (the case-graph switch is off): it neither reads nor writes it. Work from the files and data tables in the case, and say what you conclude in your reply.";

/// The case, and who is writing: this agent, on the user's behalf, from this entry.
struct Scope {
    case: Case,
    author: Author,
    session: String,
    entry: u64,
}

fn scope(ctx: &ToolCtx) -> Result<Scope, ToolOutput> {
    let global = Settings::load(&ctx.paths).memory;
    if !ctx.state().memory.over(&global).case_graph() {
        return Err(ToolOutput::err(OFF));
    }
    let store = ctx.store.lock().unwrap();
    Ok(Scope {
        case: store.case().clone(),
        author: Author::agent(&ctx.model),
        session: store.session().to_string(),
        entry: store.head(),
    })
}

impl Scope {
    fn batch(&self, kind: &str) -> Batch {
        Batch {
            on_behalf_of: Some(Author::analyst()),
            session: Some(self.session.clone()),
            entry: Some(self.entry),
            kind: kind.to_string(),
            ..Batch::new(self.author.clone())
        }
    }
}

fn attached(state: &SessionState) -> bool {
    state.memory.case_graph()
}

fn fail(e: impl std::fmt::Display) -> ToolOutput {
    ToolOutput::err(e.to_string())
}

// -- reading arguments ----------------------------------------------------------------------

/// `"<id>"`, or `{type, value, scope?}` for an entity, or `{uri}` for an external.
fn target(v: &Value, case: &Case) -> Result<Target, String> {
    match v {
        Value::String(id) if !id.trim().is_empty() => {
            let id = id.trim();
            if reference::looks_like(id) {
                return from_reference(id, case);
            }
            Ok(Target::Id(id.to_string()))
        }
        Value::Object(o) => {
            if let Some(id) = o.get("id").and_then(Value::as_str) {
                return Ok(Target::Id(id.to_string()));
            }
            if let Some(uri) = o.get("uri").and_then(Value::as_str) {
                return Ok(Target::External {
                    uri: uri.to_string(),
                });
            }
            match (
                o.get("type").and_then(Value::as_str),
                o.get("value").map(text_of),
            ) {
                (Some(t), Some(v)) if !v.is_empty() => Ok(Target::Entity {
                    type_: t.to_string(),
                    value: v,
                    scope: o.get("scope").and_then(Value::as_str).map(str::to_string),
                }),
                _ => Err("a target is an element id, or {type, value} for an entity".into()),
            }
        }
        _ => Err("a target is an element id, or {type, value} for an entity".into()),
    }
}

/// What a reference names, as the graph takes it: an element by id, an entity by its key,
/// anything else outside the graph as an external.
fn from_reference(raw: &str, case: &Case) -> Result<Target, String> {
    let mut r = reference::parse(raw).map_err(|e| e.to_string())?;
    // Written with this case's name: the same thing as without it.
    if let Some(named) = &r.case {
        if !case.is_named(named) {
            return Ok(Target::External { uri: r.to_string() });
        }
        r.case = None;
    }
    match r.target {
        reference::Target::Graph {
            id: reference::GraphId::Id(id),
            ..
        } => Ok(Target::Id(id)),
        reference::Target::Graph {
            id:
                reference::GraphId::Entity {
                    type_,
                    value,
                    scope,
                },
            ..
        } => Ok(Target::Entity {
            type_,
            value,
            scope,
        }),
        reference::Target::Set { .. } => Err(format!("`{raw}` is a set, not one thing")),
        _ => Ok(Target::External { uri: r.short() }),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(text_of).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn list(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(one) => vec![one.clone()],
    }
}

/// A `session:` source must name an entry of a session this case has: a citation that
/// points nowhere (the size column of a listing taken for the id) is refused, with the
/// sessions there are.
fn session_exists(case: &Case, uri: &str) -> Result<(), String> {
    let Ok(r) = reference::parse(uri) else {
        return Ok(());
    };
    if r.case.is_some() || !matches!(r.target, reference::Target::Session { .. }) {
        return Ok(());
    }
    if reference::lookup::exists(case, &r.target).map_err(|e| e.to_string())? {
        return Ok(());
    }
    let known = reactor_db::sessions::list(case.db())
        .map(|l| {
            l.iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    Err(format!(
        "`{uri}` names no entry of this case; its sessions: {known}"
    ))
}

/// `"uri"` or `{uri, locator?}`.
fn source(v: &Value, case: &Case) -> Result<Link, String> {
    match v {
        Value::String(uri) => {
            session_exists(case, uri)?;
            let (uri, place) = reference::split_source(&reference::localize(case, uri));
            Ok(match place {
                Some(p) => Link::source(&uri).at(&p, None),
                None => Link::source(&uri),
            })
        }
        Value::Object(o) => {
            let uri = o
                .get("uri")
                .and_then(Value::as_str)
                .ok_or("a source needs a `uri`")?;
            session_exists(case, uri)?;
            let (uri, place) = reference::split_source(&reference::localize(case, uri));
            let link = Link::source(&uri);
            Ok(
                match o
                    .get("locator")
                    .map(text_of)
                    .filter(|l| !l.is_empty())
                    .or(place)
                {
                    Some(l) => link.at(&l, None),
                    None => link,
                },
            )
        }
        _ => Err("a source is a URI, or {uri, locator}".into()),
    }
}

/// A domain timestamp as the model wrote it: read as UTC, with what was assumed, and
/// the raw text kept.
fn time_into(raw: Option<String>, content: &mut Content, set: fn(&mut Content, String)) {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return;
    };
    match parse_source_time(&raw) {
        Some(p) => {
            set(content, p.utc);
            content.time.tz_certainty =
                Some(if p.zone_stated { "stated" } else { "assumed" }.into());
            content.time.precision = Some(p.precision.into());
        }
        None => content.time.tz_certainty = Some("unknown".into()),
    }
    let kept = content.time.raw.get_or_insert_with(String::new);
    if !kept.is_empty() {
        kept.push_str(" | ");
    }
    kept.push_str(&raw);
}

fn content(o: &Map<String, Value>) -> Content {
    let mut c = Content {
        value: o
            .get("value")
            .or(o.get("text"))
            .map(text_of)
            .filter(|v| !v.is_empty()),
        props: o
            .get("props")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        tags: strings(o.get("tags")),
        confidence: o.get("confidence").map(text_of).filter(|v| !v.is_empty()),
        ..Content::default()
    };
    let get = |k: &str| o.get(k).map(text_of);
    time_into(get("time"), &mut c, |c, t| c.time.occurred_at = Some(t));
    time_into(get("valid_from"), &mut c, |c, t| {
        c.time.valid_from = Some(t)
    });
    time_into(get("valid_to"), &mut c, |c, t| c.time.valid_to = Some(t));
    c
}

// -- rendering ------------------------------------------------------------------------------

fn short(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() <= n {
        line.to_string()
    } else {
        format!(
            "{}…",
            line.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    }
}

/// One line for an element: what it is, what it says, and its state where that is not
/// the ordinary one.
fn line(e: &Element) -> String {
    let mut out = format!("graph:{} {}", e.id, e.type_);
    if let Some(v) = &e.content.value {
        out.push_str(&format!(" \"{}\"", short(v, 100)));
    }
    if let Some(t) = e
        .content
        .time
        .occurred_at
        .as_ref()
        .or(e.content.time.valid_from.as_ref())
    {
        out.push_str(&format!(" at {t}"));
    }
    if e.content.status != "active" {
        out.push_str(&format!(" [{}]", e.content.status));
    }
    if let Some(c) = &e.content.confidence {
        out.push_str(&format!(" ({c})"));
    }
    if !e.content.tags.is_empty() {
        out.push_str(&format!(" #{}", e.content.tags.join(" #")));
    }
    out
}

/// A short name for an element in a relation: `net.ipv4:198.51.100.7`, or its id.
pub fn name(db: &Db, id: &str) -> String {
    match query::get(db, id) {
        Ok(Some(e)) => match &e.content.value {
            Some(v) => format!("{}:{}", e.type_, short(v, 60)),
            None => format!("{} {}", e.type_, e.id),
        },
        _ => id.to_string(),
    }
}

/// An element in full: what it says, what it points at, and what points at it.
pub fn detail(db: &Db, e: &Element) -> reactor_db::Result<String> {
    let mut out = vec![
        line(e),
        format!(
            "  {} · version {} · recorded {} by {}",
            e.kind.as_str(),
            e.ver,
            e.recorded_at,
            e.author
        ),
    ];
    if !e.content.props.is_empty() {
        out.push(format!(
            "  props {}",
            Value::Object(e.content.props.clone())
        ));
    }
    if let Some(raw) = &e.content.time.raw {
        out.push(format!(
            "  time as written: {raw} (zone {})",
            e.content.time.tz_certainty.as_deref().unwrap_or("unknown")
        ));
    }
    for g in query::edges_from(db, &e.id)? {
        let at = g.locator.map(|l| format!(" @{l}")).unwrap_or_default();
        let pin = g
            .pinned_ver
            .map(|v| format!(" (judged its version {v})"))
            .unwrap_or_default();
        out.push(format!("  -{}-> {}{at}{pin}", g.role, name(db, &g.to_id)));
    }
    for g in query::edges_to(db, &e.id)? {
        out.push(format!(
            "  <-{}- {} graph:{}",
            g.role,
            name(db, &g.from_id),
            g.from_id
        ));
    }
    Ok(out.join("\n"))
}

// -- graph_write ----------------------------------------------------------------------------

pub struct GraphWrite;

const ANNOTATIONS: [&str; 6] = [
    "hypothesis",
    "judgement",
    "note",
    "cluster",
    "same_as",
    "search_negative",
];

const CONFIDENCE: [&str; 7] = [
    "almost no chance",
    "very unlikely",
    "unlikely",
    "roughly even",
    "likely",
    "very likely",
    "almost certain",
];

impl Tool for GraphWrite {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_write".into(),
            description: "Write to the case's graph. An element is new, or, with an `id`, a new version of one that exists: nothing is overwritten. An `entity`, `event` or `observation` records what a source says and an event or observation needs its `source`; an `annotation` records what you or the user conclude. Returns each element with its reference and the change number.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "elements": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Revise this element (a reference or id) instead of adding one; then only the fields below that you give change." },
                                "kind": { "type": "string", "enum": ["entity", "event", "observation", "annotation"], "description": "entity: a thing that can show up in many places (an address, a host, a hash): the same type and value is always the same node. event: it happened. observation: it holds. annotation: a conclusion of yours or the user's. Not needed when `type` is an annotation type." },
                                "type": { "type": "string", "description": "Dot-namespaced, e.g. net.ipv4, file.hash.sha256, host.name, auth.logon, net.connection (`graph_query` with `what: vocabulary` lists the known ones). For an annotation: hypothesis (a candidate explanation), judgement (an assessment of a claim, `about` it, with a confidence; `supports` or `refutes` a hypothesis), note (commentary, a question, a to-do), cluster (these belong together: `member` links), same_as (two entities are one: `about` both), search_negative (you looked and found nothing: `source` where, `about` what for)." },
                                "value": { "type": "string", "description": "An entity's value, or a label for an event or observation. For an annotation, its text. Plain text; Markdown only for a long one (several paragraphs), because the graph shows it rendered." },
                                "scope": { "type": "string", "description": "Entities only: the context the value is unique in (a private address at site-a)." },
                                "time": { "type": "string", "description": "Events: when it happened, as the source wrote it." },
                                "valid_from": { "type": "string", "description": "Observations: when it started to hold." },
                                "valid_to": { "type": "string", "description": "Observations: when it stopped." },
                                "confidence": { "type": "string", "description": format!("Of an annotation: {}.", CONFIDENCE.join(", ")) },
                                "tags": { "type": "array", "items": { "type": "string" } },
                                "props": { "type": "object", "description": "Details, each named `<namespace>.<name>` (`graph_query` with `what: fields` lists them). The namespace is how a value is drawn: time, size, ref, ..." },
                                "links": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "role": { "type": "string", "description": "src/dst, actor/target, parent/child, subject/object, query/answer, ...; for an annotation about, supports, refutes or member." },
                                            "to": { "description": "A reference (graph:<id>, graph:entity/<type>/<value>) or an element id, or {type, value} for an entity." }
                                        },
                                        "required": ["role", "to"]
                                    }
                                },
                                "source": { "description": "A reference: artifact:<path>[#locator], session:<id>#entry=<n>, doc:<id>, or a URL; or {uri, locator}; or a list of them. Required for an event or observation." },
                                "status": { "type": "string", "enum": ["active", "conflicting", "soft-deleted"], "description": "Revising: the new status." },
                                "add_tags": { "type": "array", "items": { "type": "string" }, "description": "Revising." },
                                "remove_tags": { "type": "array", "items": { "type": "string" }, "description": "Revising." },
                                "reason": { "type": "string", "description": "Revising: why; kept as a note on the element." }
                            }
                        }
                    }
                },
                "required": ["elements"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            let db = s.case.db();
            let mut batch = s.batch("graph");
            let mut revised: Vec<String> = Vec::new();
            for (i, el) in list(args.get("elements")).iter().enumerate() {
                let n = i + 1;
                let Some(o) = el.as_object() else {
                    return ToolOutput::err(format!("element {n}: not an object"));
                };
                if let Some(id) = o.get("id").filter(|v| !v.is_null()) {
                    match revision(&s, id, o, &mut batch) {
                        Ok(id) => revised.push(id),
                        Err(e) => return ToolOutput::err(format!("element {n}: {e}")),
                    }
                    continue;
                }
                match addition(o, &s.case) {
                    Ok(put) => batch.puts.push(put),
                    Err(e) => return ToolOutput::err(format!("element {n}: {e}")),
                }
            }
            if batch.puts.is_empty() && batch.changes.is_empty() {
                return ToolOutput::err("graph_write needs `elements`");
            }
            let unnamed = unnamespaced(db, &batch);
            match graph::apply(db, batch) {
                Ok(done) => {
                    let mut out: Vec<String> = Vec::new();
                    // One new element is shown in full, with what it links to; several, a
                    // line each.
                    let alone = done.ids.len() == 1 && revised.is_empty();
                    for id in &done.ids {
                        if let Ok(Some(e)) = query::get(db, id) {
                            match alone {
                                true => out.push(detail(db, &e).unwrap_or_else(|_| line(&e))),
                                false => out.push(line(&e)),
                            }
                        }
                    }
                    for id in &revised {
                        if let Ok(Some(e)) = query::get(db, id) {
                            out.push(format!("{} -- now at version {}", line(&e), e.ver));
                        }
                    }
                    out.push(match done.event_seq {
                        Some(seq) => format!(
                            "{} new, {} updated (change:{seq})",
                            done.created, done.versioned
                        ),
                        None => "nothing new: the graph already says this".into(),
                    });
                    if !unnamed.is_empty() {
                        out.push(format!(
                            "{} {} no namespace and {} drawn as text; name a property `<namespace>.<name>` (`graph_query` with `what: fields`)",
                            unnamed.join(", "),
                            if unnamed.len() == 1 { "has" } else { "have" },
                            if unnamed.len() == 1 { "is" } else { "are" },
                        ));
                    }
                    ToolOutput::ok(out.join("\n"))
                }
                Err(e) => fail(e),
            }
        })
    }
}

/// The property names of a batch that no known namespace speaks for, once aliases are
/// folded: what the write says is drawn as text (ADR-0067).
fn unnamespaced(db: &reactor_db::Db, batch: &Batch) -> Vec<String> {
    let Ok(fields) = fields::load(db) else {
        return Vec::new();
    };
    let names = batch
        .puts
        .iter()
        .flat_map(|p| p.content.props.keys())
        .chain(batch.changes.iter().flat_map(|c| c.set_props.keys()));
    fields.without_namespace(names)
}

/// The element id a reference or id argument names.
fn element_id(case: &Case, v: &Value) -> Result<String, String> {
    let db = case.db();
    match target(v, case)? {
        Target::Id(id) => Ok(id),
        Target::Entity {
            type_,
            value,
            scope,
        } => query::entity_id(db, &type_, &value, scope.as_deref()).map_err(|e| e.to_string()),
        Target::External { .. } => Err("name an element by its id or a graph reference".into()),
    }
}

/// An element with an `id`: a new version of it.
fn revision(
    s: &Scope,
    id: &Value,
    o: &Map<String, Value>,
    batch: &mut Batch,
) -> Result<String, String> {
    let id = element_id(&s.case, id)?;
    let change = Change {
        id: id.clone(),
        status: o.get("status").and_then(Value::as_str).map(str::to_string),
        confidence: o
            .get("confidence")
            .map(|v| Some(text_of(v)).filter(|t| !t.is_empty())),
        value: o
            .get("value")
            .or(o.get("text"))
            .map(|v| Some(text_of(v)).filter(|t| !t.is_empty())),
        set_props: o
            .get("props")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        add_tags: strings(o.get("add_tags")),
        remove_tags: strings(o.get("remove_tags")),
        ..Change::default()
    };
    batch.changes.push(change);
    if let Some(reason) = o.get("reason").map(text_of).filter(|r| !r.is_empty()) {
        batch.puts.push(Put {
            content: Content::value(reason),
            links: vec![Link::new("about", Target::Id(id.clone()))],
            ..Put::new(Kind::Annotation, "analysis.note")
        });
    }
    Ok(id)
}

/// An element that is new: its kind, its content, what it links to and where it came from.
fn addition(o: &Map<String, Value>, case: &Case) -> Result<Put, String> {
    let given = o.get("type").and_then(Value::as_str).map(str::trim);
    let annotation_type = given
        .map(|t| t.trim_start_matches("analysis."))
        .filter(|t| ANNOTATIONS.contains(t));
    let kind = match o.get("kind").and_then(Value::as_str).and_then(Kind::parse) {
        Some(k @ (Kind::Entity | Kind::Event | Kind::Observation | Kind::Annotation)) => k,
        Some(Kind::External) => {
            return Err("an external is made by naming it as a `source` or a link".into());
        }
        None if annotation_type.is_some() => Kind::Annotation,
        None => {
            return Err(
                "`kind` is entity, event, observation or annotation (or give an annotation `type`)"
                    .into(),
            );
        }
    };
    let Some(type_) = given.filter(|t| !t.is_empty()) else {
        return Err("needs a `type`".into());
    };
    let mut put = if kind == Kind::Annotation {
        let Some(a) = annotation_type else {
            return Err(format!(
                "an annotation's `type` is one of {}",
                ANNOTATIONS.join(", ")
            ));
        };
        let mut put = Put {
            scope: None,
            content: content(o),
            ..Put::new(Kind::Annotation, &format!("analysis.{a}"))
        };
        if put.content.value.is_none() && a != "cluster" && a != "same_as" {
            return Err(format!("a {a} needs its text in `value`"));
        }
        if a == "hypothesis" {
            put.content
                .props
                .entry("hypothesis_status")
                .or_insert_with(|| json!("open"));
        }
        put
    } else {
        Put {
            scope: o.get("scope").and_then(Value::as_str).map(str::to_string),
            content: content(o),
            ..Put::new(kind, type_)
        }
    };
    let judgement = kind == Kind::Annotation && put.type_ == "analysis.judgement";
    for l in list(o.get("links")) {
        let role = l.get("role").and_then(Value::as_str).unwrap_or("");
        let to = match l.get("to").map(|t| target(t, case)) {
            Some(Ok(to)) => to,
            Some(Err(e)) => return Err(e),
            None => return Err("a link needs `to`".into()),
        };
        // A judgement is about a claim as it stood: pin that version, so it is flagged when
        // the claim moves on.
        if judgement && role == "about" {
            put.links.push(Link::new(role, to).pinned());
        } else {
            put.links.push(Link::new(role, to));
        }
    }
    if let Some(src) = o.get("source").filter(|v| !v.is_null()) {
        let sources = match src {
            Value::Array(a) => a.clone(),
            one => vec![one.clone()],
        };
        for s in &sources {
            put.links.push(source(s, case)?);
        }
    }
    Ok(put)
}

// -- graph_revert and define_type -----------------------------------------------------------

pub struct GraphRevert;

impl Tool for GraphRevert {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_revert".into(),
            description: "Take back a mistake in the graph that nothing builds on: one change, or one element back to a version. Preview first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "change": { "type": "string", "description": "A change, as tools report it: change:<n> or the number." },
                    "element": { "type": "string", "description": "An element, as a reference or id." },
                    "to": { "type": "integer", "description": "With `element`: the version to return to; 0 removes it." },
                    "force": { "type": "boolean", "description": "Also take back what builds on it." },
                    "preview": { "type": "boolean", "description": "Say what would go, and do nothing." }
                }
            }),
        }
    }
    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            match args.as_object() {
                Some(o) => revert_call(&s, o),
                None => ToolOutput::err("graph_revert needs an object"),
            }
        })
    }
}

pub struct DefineType;

impl Tool for DefineType {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "define_type".into(),
            description: "Make a recurring type of your own known to the graph, or say more about one that is. With `property` instead of `type`, the same for a property name.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "type": { "type": "string" },
                    "kind": { "type": "string", "enum": ["entity", "event", "observation"] },
                    "property": { "type": "string", "description": "A property name, `<namespace>.<name>`, in place of `type`." },
                    "aliases": { "type": "array", "items": { "type": "string" }, "description": "property: spellings that become this name when written." },
                    "definition": { "type": "string", "description": "One line." },
                    "icon": { "type": "string", "description": "A Lucide icon name." },
                    "colour": { "type": "string", "description": "How it is drawn." }
                }
            }),
        }
    }
    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            match args.as_object() {
                Some(o) => define_type_call(&s, o),
                None => ToolOutput::err("define_type needs an object"),
            }
        })
    }
}

/// Make a type known, or say more about one that is.
fn define_type_call(s: &Scope, d: &Map<String, Value>) -> ToolOutput {
    let text = |key: &str| {
        d.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    if let Some(name) = text("property") {
        let aliases = d
            .get("aliases")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        return match fields::define_field(
            s.case.db(),
            fields::Define {
                name,
                definition: text("definition"),
                icon: text("icon"),
                colour: text("colour"),
                aliases,
                ..fields::Define::default()
            },
        ) {
            Ok(f) => ToolOutput::ok(format!(
                "{} -- {} (drawn as {})",
                f.name,
                f.definition,
                fields::load(s.case.db())
                    .map(|v| v.shape(&f.name).as_str())
                    .unwrap_or("text")
            )),
            Err(e) => fail(e),
        };
    }
    let Some(name) = text("type") else {
        return ToolOutput::err("`define_type` needs the `type` or a `property`");
    };
    let kind = match text("kind") {
        Some(k) => match Kind::parse(&k) {
            Some(kind @ (Kind::Entity | Kind::Event | Kind::Observation)) => Some(kind),
            _ => return ToolOutput::err("`kind` is entity, event or observation"),
        },
        None => None,
    };
    let define = Define {
        name,
        kind,
        definition: text("definition"),
        icon: text("icon"),
        colour: text("colour"),
    };
    match vocab::define_type(s.case.db(), define) {
        Ok(t) => {
            let look = vocab::load(s.case.db())
                .map(|v| v.look(&t.name))
                .unwrap_or_default();
            ToolOutput::ok(format!(
                "{} ({}) {} -- drawn with icon {}, colour {}",
                t.name,
                t.kind.map(Kind::as_str).unwrap_or("?"),
                t.definition,
                look.icon.as_deref().unwrap_or("of its kind"),
                look.colour.as_deref().unwrap_or("neutral"),
            ))
        }
        Err(e) => fail(e),
    }
}

fn revert_call(s: &Scope, r: &Map<String, Value>) -> ToolOutput {
    // A change is `change:<n>` or the number; an element is a reference or an id.
    let change = r.get("change").and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(t) => match reference::parse(t) {
            Ok(reference::Reference {
                target: reference::Target::Change { seq },
                ..
            }) => Some(seq as i64),
            _ => t.trim().parse().ok(),
        },
        _ => None,
    });
    let element = match r.get("element") {
        Some(v) => match element_id(&s.case, v) {
            Ok(id) => Some(id),
            Err(e) => return ToolOutput::err(e),
        },
        None => None,
    };
    let what = match (change, element) {
        (Some(seq), None) => What::Event(seq),
        (None, Some(id)) => What::Element {
            id,
            to: r.get("to").and_then(Value::as_u64).unwrap_or(0) as u32,
        },
        _ => {
            return ToolOutput::err(
                "`graph_revert` names one `change` number, or one `element` with `to`",
            );
        }
    };
    let flag = |k: &str| r.get(k).and_then(Value::as_bool).unwrap_or(false);
    let (force, preview) = (flag("force"), flag("preview"));
    let plan = if preview {
        revert::plan(s.case.db(), &what, force)
    } else {
        revert::apply(s.case.db(), what, force, &s.author, Some(&s.session))
    };
    match plan {
        Ok(plan) => {
            let mut out = vec![if preview {
                "would take back:".to_string()
            } else {
                "taken back:".to_string()
            }];
            for e in &plan.elements {
                out.push(format!(
                    "- {} {} {} (by {}; {})",
                    e.id,
                    e.type_,
                    if e.back_to == 0 {
                        "leaves the graph".to_string()
                    } else {
                        format!("returns to version {}", e.back_to)
                    },
                    e.authors.join(", "),
                    e.why
                ));
            }
            if !plan.blocked_by.is_empty() {
                out.push(format!(
                    "blocked -- something builds on it: {}. Use `graph_write` with the element's `id` to revise instead, or `force` to take those along.",
                    plan.blocked_by.join("; ")
                ));
            }
            ToolOutput::ok(out.join("\n"))
        }
        Err(e) => fail(e),
    }
}

// -- graph_query ----------------------------------------------------------------------------

pub struct GraphQuery;

impl Tool for GraphQuery {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn prints_much(&self) -> bool {
        true
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_query".into(),
            description: "Read the case's graph. Absence here means not recorded, not that it does not exist.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "what": { "type": "string", "enum": ["find", "element", "relations", "stale", "orphans", "types", "vocabulary", "fields", "files"], "description": "find: list elements (filter by `kind`, `type` prefix such as `net.` or `analysis.hypothesis`, `text`, `tag`, `prop`). element: one in full, with what it links to and what links to it (`as_of`, `history`). relations: claims between two things, as `from --type--> to`. stale: what may no longer hold. orphans: entities nothing mentions. types: the types in use. vocabulary: the known types by namespace (`type` narrows it). fields: the property namespaces and the known names under them (`under` narrows it). files: the registered files and whether they changed." },
                    "id": { "description": "element, relations: an element id, or {type, value} for an entity." },
                    "kind": { "type": "string", "enum": ["entity", "event", "observation", "annotation", "external"] },
                    "type": { "type": "string", "description": "find: a type or its prefix." },
                    "text": { "type": "string", "description": "find: text the element contains." },
                    "tag": { "type": "string", "description": "find: a tag." },
                    "prop": { "type": "string", "description": "find: a property it has: `name`, `name=value`, `name>value` or `name<value`; `time.` takes any name under it. Compared by its namespace: `size>1MiB`, `time.first_seen<2026-03-01`." },
                    "under": { "type": "string", "description": "fields: a namespace or name prefix." },
                    "include_deleted": { "type": "boolean", "description": "Also soft-deleted elements (default false)." },
                    "limit": { "type": "integer", "description": "find: default 40." },
                    "as_of": { "type": "string", "description": "element: a UTC timestamp; what was recorded by then." },
                    "history": { "type": "boolean", "description": "element: every version." }
                },
                "required": ["what"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            match answer(&s, &args) {
                Ok(text) => ToolOutput::ok(text),
                Err(e) => ToolOutput::err(e),
            }
        })
    }
}

/// The id an `id` argument stands for. An entity named by type and value need not exist.
fn resolve_id(case: &Case, v: Option<&Value>) -> Result<String, String> {
    let db = case.db();
    match v.map(|v| target(v, case)) {
        Some(Ok(Target::Id(id))) => Ok(id),
        Some(Ok(Target::Entity {
            type_,
            value,
            scope,
        })) => query::entity_id(db, &type_, &value, scope.as_deref()).map_err(|e| e.to_string()),
        Some(Ok(Target::External { .. })) => Err("name an external by its id".into()),
        Some(Err(e)) => Err(e),
        None => Err("needs `id`".into()),
    }
}

fn answer(s: &Scope, args: &Value) -> Result<String, String> {
    let db = s.case.db();
    let err = |e: reactor_db::Error| e.to_string();
    match str_arg(args, "what").unwrap_or("") {
        "find" => {
            let limit = usize_arg(args, "limit").unwrap_or(40);
            let found = query::find(
                db,
                &Filter {
                    offset: 0,
                    kind: str_arg(args, "kind").and_then(Kind::parse),
                    type_prefix: str_arg(args, "type").map(str::to_string),
                    text: str_arg(args, "text").map(str::to_string),
                    tag: str_arg(args, "tag").map(str::to_string),
                    props: str_arg(args, "prop")
                        .map(|p| vec![p.to_string()])
                        .unwrap_or_default(),
                    include_deleted: args
                        .get("include_deleted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    limit: limit + 1,
                },
            )
            .map_err(err)?;
            if found.is_empty() {
                return Ok("nothing recorded matches".into());
            }
            let mut out: Vec<String> = found.iter().take(limit).map(line).collect();
            if found.len() > limit {
                out.push(format!("… more than {limit}; narrow it or raise `limit`"));
            }
            Ok(out.join("\n"))
        }
        "element" => {
            let id = resolve_id(&s.case, args.get("id"))?;
            if args
                .get("history")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let versions = query::history(db, &id).map_err(err)?;
                if versions.is_empty() {
                    return Ok(format!("{id}: not in the graph"));
                }
                return Ok(versions
                    .iter()
                    .map(|e| format!("v{} {} by {}: {}", e.ver, e.recorded_at, e.author, line(e)))
                    .collect::<Vec<_>>()
                    .join("\n"));
            }
            let found = match str_arg(args, "as_of") {
                Some(at) => {
                    let at = parse_source_time(at)
                        .map(|p| p.utc)
                        .ok_or("`as_of` is a UTC timestamp, e.g. 2026-10-04T12:00:00Z")?;
                    query::get_as_of(db, &id, &at).map_err(err)?
                }
                None => query::get(db, &id).map_err(err)?,
            };
            match found {
                Some(e) => detail(db, &e).map_err(err),
                None => Ok(format!(
                    "{id}: not in the graph (not recorded -- which is not the same as not existing)"
                )),
            }
        }
        "relations" => {
            let about = match args.get("id") {
                Some(v) => Some(resolve_id(&s.case, Some(v))?),
                None => None,
            };
            let rel = query::relations(db, about.as_deref()).map_err(err)?;
            if rel.is_empty() {
                return Ok("no relations recorded".into());
            }
            Ok(rel
                .iter()
                .map(|r| {
                    format!(
                        "{} --{}--> {}  ({})",
                        name(db, &r.from_id),
                        r.type_,
                        name(db, &r.to_id),
                        r.claim_id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "stale" => {
            let mut out: Vec<String> = query::stale(db)
                .map_err(err)?
                .iter()
                .map(|st| format!("{} {}: {}", st.id, name(db, &st.id), st.reason))
                .collect();
            for uri in pull::changed_sources(&s.case).map_err(err)? {
                out.push(format!("{uri}: changed since it was last pulled from"));
            }
            Ok(if out.is_empty() {
                "nothing is flagged".into()
            } else {
                out.join("\n")
            })
        }
        "orphans" => {
            let found = query::orphans(db).map_err(err)?;
            Ok(if found.is_empty() {
                "every entity is mentioned by something".into()
            } else {
                found.iter().map(line).collect::<Vec<_>>().join("\n")
            })
        }
        "types" => Ok(query::types_in_use(db)
            .map_err(err)?
            .iter()
            .map(|t| {
                format!(
                    "{} {} ×{}{}",
                    t.kind.as_str(),
                    t.type_,
                    t.count,
                    if t.known { "" } else { "  (not a known type)" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")),
        "vocabulary" => vocab::describe(db, str_arg(args, "type")).map_err(err),
        "fields" => fields::describe(db, str_arg(args, "under")).map_err(err),
        "files" => {
            let files = artifact::verify(&s.case).map_err(err)?;
            Ok(if files.is_empty() {
                "no files registered".into()
            } else {
                files
                    .iter()
                    .map(|(a, state)| {
                        format!(
                            "{} {} sha256:{} {}B{}",
                            a.path,
                            a.kind.as_deref().unwrap_or("-"),
                            a.sha256,
                            a.size,
                            match state {
                                artifact::State::Unchanged => "",
                                artifact::State::Changed => "  CHANGED since it was registered",
                                artifact::State::Missing =>
                                    "  MISSING (artifact_unregister withdraws the record)",
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        other => Err(format!(
            "`what` is find, element, relations, stale, orphans, types, vocabulary, fields or files -- not `{other}`"
        )),
    }
}

// -- graph_pull -----------------------------------------------------------------------------

pub struct GraphPull;

impl Tool for GraphPull {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_pull".into(),
            description: "Turn rows of a table or file in the case into graph claims, each linked back to its row. `query` is SQL over the table `src` (a whole SQLite file is also attached as `data`); its column names say what each value is: \"@type\" (required: the claim's type), \"@time\" (an event) or \"@valid_from\"/\"@valid_to\" (an observation), \"@row\" (a stable row id, so pulling again changes nothing), \"<type>@<role>\" for each entity (e.g. src_ip AS \"net.ipv4@src\"); any other column becomes a property.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "A file of the case as a reference: `artifact:reactor_data.sqlite#table=<t>`, or `artifact:<path>` for a .csv, .tsv, .jsonl or .json file (`artifact:<path>#/pointer` for an array inside a JSON file)." },
                    "query": { "type": "string", "description": "SQL over `src`." },
                    "preview": { "type": "boolean", "description": "Show what the first rows would become and write nothing (default false)." }
                },
                "required": ["source", "query"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            let (Some(src), Some(sql)) = (str_arg(&args, "source"), str_arg(&args, "query")) else {
                return ToolOutput::err("graph_pull needs `source` and `query`");
            };
            let preview = args
                .get("preview")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let req = Request {
                source: src.to_string(),
                query: sql.to_string(),
                author: s.author.clone(),
                on_behalf_of: Some(Author::analyst()),
                session: Some(s.session.clone()),
                entry: Some(s.entry),
                max_rows: 0,
                preview,
            };
            let case = s.case.clone();
            // A pull reads files and runs a query: off the async threads.
            let done = tokio::task::spawn_blocking(move || pull::pull(&case, req)).await;
            let r = match done {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return fail(e),
                Err(e) => return fail(e),
            };
            let mut out = vec![format!("{}: {} row(s)", r.source, r.rows)];
            if preview {
                out.push("would become (first rows):".into());
                out.extend(r.sample.iter().map(|l| format!("- {l}")));
            } else {
                out.push(match r.applied.event_seq {
                    Some(seq) => format!(
                        "{} claim(s); {} element(s) new, {} updated (change:{seq})",
                        r.claims.len(),
                        r.applied.created,
                        r.applied.versioned
                    ),
                    None => format!(
                        "{} claim(s), all already in the graph: nothing written",
                        r.claims.len()
                    ),
                });
                out.extend(r.sample.iter().map(|l| format!("- {l}")));
            }
            for line in &r.skipped {
                out.push(format!("skipped {line}"));
            }
            if !r.overlaps.is_empty() {
                out.push(format!(
                    "note: {} earlier pull(s) from this source used another query; their rows may overlap these (an aggregate and its individual rows count twice)",
                    r.overlaps.len()
                ));
            }
            ToolOutput::ok(out.join("\n"))
        })
    }
}

// -- artifact_register ----------------------------------------------------------------------

pub struct ArtifactUnregister;

impl Tool for ArtifactUnregister {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "artifact_unregister".into(),
            description: "Withdraw a file's record from the case's registry, e.g. a file registered by mistake or since deleted. The file itself is left alone, and the log keeps that it was registered and withdrawn. Graph elements that cite it keep their references.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The registered path, as `graph_query` with `what: files` lists it." }
                },
                "required": ["path"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            let Some(path) = str_arg(&args, "path") else {
                return ToolOutput::err("artifact_unregister needs a `path`");
            };
            match artifact::unregister(&s.case, path, &s.author, Some(&s.session)) {
                Ok(a) => ToolOutput::ok(format!("{} is no longer registered", a.path)),
                Err(e) => fail(e),
            }
        })
    }
}

pub struct ArtifactRegister;

impl Tool for ArtifactRegister {
    fn group(&self) -> Option<&'static str> {
        Some(super::GRAPH_GROUP)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "artifact_register".into(),
            description: "Register a file in the case as evidence or as something you produced: its path, hashes and kind are recorded, and the file stays where it is. `graph_query` with `what: files` reports one that changed afterwards.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Inside the case; relative to its root." },
                    "kind": { "type": "string", "description": "sample, extracted_file, pcap, memdump, disasm, decomp, script, report, log, ..." },
                    "parent": { "type": "string", "description": "The registered file this one came out of." }
                },
                "required": ["path"]
            }),
        }
    }

    fn available(&self, state: &SessionState) -> bool {
        attached(state)
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolCtx) -> BoxFut<'a, ToolOutput> {
        Box::pin(async move {
            let s = match scope(ctx) {
                Ok(s) => s,
                Err(refusal) => return refusal,
            };
            let Some(path) = str_arg(&args, "path") else {
                return ToolOutput::err("artifact_register needs a `path`");
            };
            // The model's shell may have changed directory; a relative path is the
            // case's, as every path in the registry is.
            match artifact::register(
                &s.case,
                path,
                str_arg(&args, "kind"),
                str_arg(&args, "parent"),
                &s.author,
                Some(&s.session),
            ) {
                Ok(a) => ToolOutput::ok(format!(
                    "{} {} {}B\nsha256 {}\nsha1   {}\nmd5    {}{}{}",
                    a.path,
                    a.kind.as_deref().unwrap_or("-"),
                    a.size,
                    a.sha256,
                    a.sha1,
                    a.md5,
                    a.mime.map(|m| format!("\n{m}")).unwrap_or_default(),
                    a.parent
                        .map(|p| format!("\ncame out of {p}"))
                        .unwrap_or_default()
                )),
                Err(e) => fail(e),
            }
        })
    }
}
