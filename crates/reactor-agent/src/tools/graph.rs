//! The case's graph, for the agent
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! Five tools over `reactor-db`: add what a source says, annotate with what you
//! conclude, query, pull rows of a source in, and register a file. Each is a thin skin:
//! the rules — a claim needs a source, an entity's key never changes, nothing is
//! overwritten — are the database's, and a refusal comes back as the text to act on.
//!
//! They are advertised only while the session is attached to the case's graph. With the
//! *Case graph* switch off the session neither reads nor writes it, and a stale call is
//! told why (ADR-0030).

use reactor_context::settings::Settings;
use reactor_db::artifact;
use reactor_db::graph::pull::{self, Request};
use reactor_db::graph::revert::{self, What};
use reactor_db::graph::{
    self, Batch, Change, Content, Element, Filter, Kind, Link, Put, Target, query,
};
use reactor_db::manifest::Author;
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
fn target(v: &Value) -> Result<Target, String> {
    match v {
        Value::String(id) if !id.trim().is_empty() => Ok(Target::Id(id.trim().to_string())),
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

/// `"uri"` or `{uri, locator?}`.
fn source(v: &Value) -> Result<Link, String> {
    match v {
        Value::String(uri) => Ok(Link::source(uri)),
        Value::Object(o) => {
            let uri = o
                .get("uri")
                .and_then(Value::as_str)
                .ok_or("a source needs a `uri`")?;
            let link = Link::source(uri);
            Ok(
                match o.get("locator").map(text_of).filter(|l| !l.is_empty()) {
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
    let mut out = format!("{} {}", e.id, e.type_);
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
            "  <-{}- {} {}",
            g.role,
            name(db, &g.from_id),
            g.from_id
        ));
    }
    Ok(out.join("\n"))
}

// -- graph_add ------------------------------------------------------------------------------

pub struct GraphAdd;

impl Tool for GraphAdd {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_add".into(),
            description: "Add to the case's graph what a source says. An `entity` is a thing that can show up in many places (an address, a host, a hash): give its type and value, and the same value always lands on the same node. An `event` happened and an `observation` holds: each links the entities it involves by role (src/dst, actor/target, parent/child, subject/object, query/answer) and MUST name its `source` -- a file or table in the case, or a session entry as `session:<id>#entry=<n>`. What you conclude yourself is not for this tool: use graph_annotate. For many rows, load them into reactor_data.sqlite and use graph_pull instead.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "elements": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": { "type": "string", "enum": ["entity", "event", "observation"] },
                                "type": { "type": "string", "description": "Dot-namespaced, e.g. net.ipv4, net.domain, file.hash.sha256, host.name, account.user, auth.logon, net.connection, net.resolution. `graph_query what=types` lists the ones in use." },
                                "value": { "type": "string", "description": "An entity's value; optional label for a claim." },
                                "scope": { "type": "string", "description": "Entity only: the context a value is unique in (a private address at site-a)." },
                                "time": { "type": "string", "description": "Event: when it happened, as the source wrote it." },
                                "valid_from": { "type": "string" },
                                "valid_to": { "type": "string" },
                                "confidence": { "type": "string" },
                                "tags": { "type": "array", "items": { "type": "string" } },
                                "props": { "type": "object" },
                                "links": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "role": { "type": "string" },
                                            "to": { "description": "An element id, or {type, value} for an entity." }
                                        },
                                        "required": ["role", "to"]
                                    }
                                },
                                "source": { "description": "A URI, or {uri, locator}. Required for an event or observation." }
                            },
                            "required": ["kind", "type"]
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
            let mut batch = s.batch("graph");
            for (i, el) in list(args.get("elements")).iter().enumerate() {
                let Some(o) = el.as_object() else {
                    return ToolOutput::err(format!("element {}: not an object", i + 1));
                };
                let kind = match o.get("kind").and_then(Value::as_str).and_then(Kind::parse) {
                    Some(k @ (Kind::Entity | Kind::Event | Kind::Observation)) => k,
                    _ => {
                        return ToolOutput::err(format!(
                            "element {}: `kind` is entity, event or observation (a conclusion is an annotation: use graph_annotate)",
                            i + 1
                        ));
                    }
                };
                let Some(type_) = o.get("type").and_then(Value::as_str) else {
                    return ToolOutput::err(format!("element {}: needs a `type`", i + 1));
                };
                let mut put = Put {
                    scope: o.get("scope").and_then(Value::as_str).map(str::to_string),
                    content: content(o),
                    ..Put::new(kind, type_)
                };
                for l in list(o.get("links")) {
                    let role = l.get("role").and_then(Value::as_str).unwrap_or("");
                    match l.get("to").map(target) {
                        Some(Ok(to)) => put.links.push(Link::new(role, to)),
                        Some(Err(e)) => return ToolOutput::err(format!("element {}: {e}", i + 1)),
                        None => {
                            return ToolOutput::err(format!(
                                "element {}: a link needs `to`",
                                i + 1
                            ));
                        }
                    }
                }
                if let Some(src) = o.get("source") {
                    match source(src) {
                        Ok(link) => put.links.push(link),
                        Err(e) => return ToolOutput::err(format!("element {}: {e}", i + 1)),
                    }
                }
                batch.puts.push(put);
            }
            if batch.puts.is_empty() {
                return ToolOutput::err("graph_add needs `elements`");
            }
            match graph::apply(s.case.db(), batch) {
                Ok(done) => {
                    let mut out: Vec<String> = Vec::new();
                    for id in &done.ids {
                        if let Ok(Some(e)) = query::get(s.case.db(), id) {
                            out.push(line(&e));
                        }
                    }
                    out.push(match done.event_seq {
                        Some(seq) => format!(
                            "{} new, {} updated (change #{seq})",
                            done.created, done.versioned
                        ),
                        None => "nothing new: the graph already says this".into(),
                    });
                    ToolOutput::ok(out.join("\n"))
                }
                Err(e) => fail(e),
            }
        })
    }
}

// -- graph_annotate -------------------------------------------------------------------------

pub struct GraphAnnotate;

const ANNOTATIONS: [&str; 6] = [
    "hypothesis",
    "judgement",
    "note",
    "cluster",
    "same_as",
    "search_negative",
];

impl Tool for GraphAnnotate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_annotate".into(),
            description: "Record what you or the user conclude, in the case's graph. Add an annotation: a `hypothesis` (a candidate explanation), a `judgement` (your assessment of a claim, with a confidence; link it with `about`, and `supports` or `refutes` a hypothesis), a `note` (commentary, a question, a to-do), a `cluster` (these belong together: `members`), `same_as` (two entities are one: `about` both) or `search_negative` (you looked and found nothing: say where with `sources` and what for with `about`). Or `change` an element: its status (active, conflicting, soft-deleted), confidence, tags or properties -- a new version, nothing is overwritten. Or `revert` a mistake nothing builds on: one change by its number, or one element back to a version; preview first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "type": { "type": "string", "enum": ANNOTATIONS },
                    "text": { "type": "string", "description": "The hypothesis, assessment or note itself. Plain text when it is a sentence or two. Only a long one -- several paragraphs -- is written as Markdown (a short heading, lists, code in backticks), because the graph shows it rendered; a short one with markup in it just looks cluttered." },
                    "confidence": { "type": "string", "description": "almost no chance | very unlikely | unlikely | roughly even | likely | very likely | almost certain" },
                    "about": { "type": "array", "description": "Element ids, or {type, value} entities.", "items": {} },
                    "supports": { "type": "array", "items": {} },
                    "refutes": { "type": "array", "items": {} },
                    "members": { "type": "array", "items": {} },
                    "sources": { "type": "array", "description": "search_negative: the URIs that were searched.", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "props": { "type": "object", "description": "E.g. for search_negative: {query, window_from, window_to}." },
                    "change": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "status": { "type": "string", "enum": ["active", "conflicting", "soft-deleted"] },
                            "confidence": { "type": "string" },
                            "text": { "type": "string" },
                            "add_tags": { "type": "array", "items": { "type": "string" } },
                            "remove_tags": { "type": "array", "items": { "type": "string" } },
                            "props": { "type": "object" },
                            "reason": { "type": "string", "description": "Why; kept as a note on the element." }
                        },
                        "required": ["id"]
                    },
                    "revert": {
                        "type": "object",
                        "properties": {
                            "change": { "type": "integer", "description": "A change number, as tools report it." },
                            "element": { "type": "string" },
                            "to": { "type": "integer", "description": "The version to return to; 0 removes the element." },
                            "force": { "type": "boolean" },
                            "preview": { "type": "boolean", "description": "Say what would go, and do nothing." }
                        }
                    }
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
            let db = s.case.db();
            if let Some(r) = args.get("revert").and_then(Value::as_object) {
                return revert_call(&s, r);
            }
            if let Some(c) = args.get("change").and_then(Value::as_object) {
                let Some(id) = c.get("id").and_then(Value::as_str) else {
                    return ToolOutput::err("`change` needs the element's `id`");
                };
                let change = Change {
                    id: id.to_string(),
                    status: c.get("status").and_then(Value::as_str).map(str::to_string),
                    confidence: c
                        .get("confidence")
                        .map(|v| Some(text_of(v)).filter(|t| !t.is_empty())),
                    value: c
                        .get("text")
                        .map(|v| Some(text_of(v)).filter(|t| !t.is_empty())),
                    set_props: c
                        .get("props")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default(),
                    add_tags: strings(c.get("add_tags")),
                    remove_tags: strings(c.get("remove_tags")),
                    ..Change::default()
                };
                let mut batch = s.batch("graph").change(change);
                if let Some(reason) = c.get("reason").map(text_of).filter(|r| !r.is_empty()) {
                    batch.puts.push(Put {
                        content: Content::value(reason),
                        links: vec![Link::new("about", Target::Id(id.to_string()))],
                        ..Put::new(Kind::Annotation, "analysis.note")
                    });
                }
                return match graph::apply(db, batch) {
                    Ok(done) => match query::get(db, id) {
                        Ok(Some(e)) => ToolOutput::ok(format!(
                            "{}\n{}",
                            line(&e),
                            match done.event_seq {
                                Some(seq) => format!("now at version {} (change #{seq})", e.ver),
                                None => "unchanged: it already says this".into(),
                            }
                        )),
                        _ => ToolOutput::ok("changed"),
                    },
                    Err(e) => fail(e),
                };
            }

            let Some(kind) = str_arg(&args, "type")
                .map(|t| t.trim().trim_start_matches("analysis.").to_string())
                .filter(|t| ANNOTATIONS.contains(&t.as_str()))
            else {
                return ToolOutput::err(format!(
                    "graph_annotate needs a `type` ({}), or `change`, or `revert`",
                    ANNOTATIONS.join(", ")
                ));
            };
            let Some(o) = args.as_object() else {
                return ToolOutput::err("graph_annotate needs an object");
            };
            let mut put = Put {
                content: content(o),
                ..Put::new(Kind::Annotation, &format!("analysis.{kind}"))
            };
            if put.content.value.is_none() && kind != "cluster" && kind != "same_as" {
                return ToolOutput::err(format!("a {kind} needs `text`"));
            }
            if kind == "hypothesis" {
                put.content
                    .props
                    .entry("hypothesis_status")
                    .or_insert_with(|| json!("open"));
            }
            for (key, role) in [
                ("about", "about"),
                ("supports", "supports"),
                ("refutes", "refutes"),
                ("members", "member"),
            ] {
                for t in list(args.get(key)) {
                    match target(&t) {
                        // A judgement is about a claim as it stood: pin that version, so
                        // it is flagged when the claim moves on.
                        Ok(to) if kind == "judgement" && role == "about" => {
                            put.links.push(Link::new(role, to).pinned())
                        }
                        Ok(to) => put.links.push(Link::new(role, to)),
                        Err(e) => return ToolOutput::err(format!("`{key}`: {e}")),
                    }
                }
            }
            for uri in strings(args.get("sources")) {
                put.links.push(Link::source(&uri));
            }
            match graph::apply(db, s.batch("graph").put(put)) {
                Ok(done) => match query::get(db, &done.ids[0]) {
                    Ok(Some(e)) => match detail(db, &e) {
                        Ok(d) => ToolOutput::ok(format!(
                            "{d}\n(change #{})",
                            done.event_seq.unwrap_or_default()
                        )),
                        Err(e) => fail(e),
                    },
                    _ => ToolOutput::ok("recorded"),
                },
                Err(e) => fail(e),
            }
        })
    }
}

fn revert_call(s: &Scope, r: &Map<String, Value>) -> ToolOutput {
    let what = match (
        r.get("change").and_then(Value::as_i64),
        r.get("element").and_then(Value::as_str),
    ) {
        (Some(seq), None) => What::Event(seq),
        (None, Some(id)) => What::Element {
            id: id.to_string(),
            to: r.get("to").and_then(Value::as_u64).unwrap_or(0) as u32,
        },
        _ => {
            return ToolOutput::err(
                "`revert` names one `change` number, or one `element` with `to`",
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
                    "blocked -- something builds on it: {}. Use `change` to revise instead, or `force` to take those along.",
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
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_query".into(),
            description: "Read the case's graph. `find` lists elements (filter by kind, type prefix such as `net.` or `analysis.hypothesis`, text, tag). `element` shows one in full with everything it links to and everything that points at it -- for an entity that is where else it shows up; for a hypothesis, what supports and refutes it -- optionally `as_of` a moment or with its `history`. `relations` lists claims between two things as `from --type--> to`. `stale` lists what may no longer hold, `orphans` entities nothing mentions, `types` the types in use, `files` the registered files and whether they changed. Absence here means not recorded, not that it does not exist.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "what": { "type": "string", "enum": ["find", "element", "relations", "stale", "orphans", "types", "files"] },
                    "id": { "description": "element, relations: an element id, or {type, value} for an entity." },
                    "kind": { "type": "string", "enum": ["entity", "event", "observation", "annotation", "external"] },
                    "type": { "type": "string", "description": "find: a type or its prefix." },
                    "text": { "type": "string" },
                    "tag": { "type": "string" },
                    "include_deleted": { "type": "boolean" },
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
fn resolve_id(db: &Db, v: Option<&Value>) -> Result<String, String> {
    match v.map(target) {
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
                    kind: str_arg(args, "kind").and_then(Kind::parse),
                    type_prefix: str_arg(args, "type").map(str::to_string),
                    text: str_arg(args, "text").map(str::to_string),
                    tag: str_arg(args, "tag").map(str::to_string),
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
            let id = resolve_id(db, args.get("id"))?;
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
                Some(v) => Some(resolve_id(db, Some(v))?),
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
                                artifact::State::Missing => "  MISSING",
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        other => Err(format!(
            "`what` is find, element, relations, stale, orphans, types or files -- not `{other}`"
        )),
    }
}

// -- graph_pull -----------------------------------------------------------------------------

pub struct GraphPull;

impl Tool for GraphPull {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "graph_pull".into(),
            description: "Turn rows of a source in the case into graph claims, each linked back to its row. `source` is `sqlite:reactor_data.sqlite?table=<t>`, or a .csv, .jsonl or .json file in the case (`json:path#/pointer` for an array inside). `query` is plain SQL over the table `src` (a whole SQLite file is also attached as `data`), whose column names say what each value is: \"@type\" (required: the claim's type), \"@time\" (an event) or \"@valid_from\"/\"@valid_to\" (an observation), \"@row\" (a stable row id, so pulling again changes nothing), \"<type>@<role>\" for each entity (e.g. src_ip AS \"net.ipv4@src\"), and any other column becomes a property. Pull what is relevant, not everything: aggregate large sets (count(*) ... GROUP BY) and leave the bulk in the source. `preview` shows what the first rows would become and writes nothing.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "query": { "type": "string" },
                    "preview": { "type": "boolean" }
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
                        "{} claim(s); {} element(s) new, {} updated (change #{seq})",
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

pub struct ArtifactRegister;

impl Tool for ArtifactRegister {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "artifact_register".into(),
            description: "Register a file in the case as evidence or as something you produced: its path, hashes and kind are recorded and the file stays where it is. Do this for a sample, a capture, a dump, and for anything you unpack, extract or write that later work relies on (`parent` names the registered file it came out of). A file that changes afterwards is reported by `graph_query what=files`.".into(),
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
