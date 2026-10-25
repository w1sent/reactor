//! STIX 2.1: sharing finished intelligence.
//!
//! The mapping follows the graph concept's table, and it is lossy by nature — STIX has no
//! versions, no record time apart from domain time, no way to say a claim is being
//! questioned — so every object also carries an `x_reactor` property holding what REactor
//! knows about it, edges included. A bundle that has it is **restored exactly**: the same
//! ids, the same edges, who wrote what and when, as of the version exported (history is not
//! exported). A bundle from elsewhere is imported as best it maps, every claim sourced to
//! the bundle.
//!
//! | REactor | STIX 2.1 |
//! |---|---|
//! | entity | a cyber observable: `ipv4-addr`, `domain-name`, `file`, `user-account`, …; `x-reactor-entity` where STIX has none (a host, a process) |
//! | event | `observed-data` |
//! | observation of two entities | `relationship` |
//! | other observation | `observed-data` |
//! | judgement | `opinion` |
//! | cluster | `grouping` |
//! | note, hypothesis, … | `note` |
//! | external | an `external_references` entry on what came from it |
//!
//! The case itself is an `identity`, which everything is `created_by`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::case::Case;
use crate::error::{Error, Result};
use crate::graph::query::{self, Element};
use crate::graph::{self, Batch, Content, Kind, Link, Put, Target, Time};
use crate::manifest::Author;

pub const SPEC_VERSION: &str = "2.1";

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub include_deleted: bool,
}

/// A UUID (version 5 in form) made from `seed`: the same seed, the same id, so exporting
/// twice gives the same bundle and an id can be found again from what it came from.
fn uuid_of(seed: &str) -> String {
    let d = Sha256::digest(seed.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn stix_id(kind: &str, original: &str) -> String {
    format!("{kind}--{}", uuid_of(&format!("{kind}|{original}")))
}

/// How sure, as STIX's 0 to 100: the words of estimative probability.
fn confidence_number(words: &str) -> Option<u32> {
    Some(match words {
        "almost no chance" => 5,
        "very unlikely" => 15,
        "unlikely" => 30,
        "roughly even" => 50,
        "likely" => 70,
        "very likely" => 85,
        "almost certain" => 95,
        _ => return None,
    })
}

fn opinion_of(words: Option<&str>) -> &'static str {
    match words {
        Some("almost certain" | "very likely") => "strongly-agree",
        Some("likely") => "agree",
        Some("unlikely") => "disagree",
        Some("very unlikely" | "almost no chance") => "strongly-disagree",
        _ => "neutral",
    }
}

fn words_of_opinion(opinion: &str) -> Option<&'static str> {
    Some(match opinion {
        "strongly-agree" => "very likely",
        "agree" => "likely",
        "neutral" => "roughly even",
        "disagree" => "unlikely",
        "strongly-disagree" => "very unlikely",
        _ => return None,
    })
}

/// A relationship type is lowercase words joined by hyphens.
fn relationship_type(type_: &str) -> String {
    let t: String = type_
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let t = t.trim_matches('-').to_string();
    if t.len() >= 3 { t } else { "related-to".into() }
}

/// The STIX object type and properties an entity becomes.
fn entity_object(type_: &str, value: &str) -> (&'static str, Map<String, Value>) {
    let mut p = Map::new();
    let kind = match type_ {
        "net.ipv4" => ("ipv4-addr", "value"),
        "net.ipv6" => ("ipv6-addr", "value"),
        "net.domain" => ("domain-name", "value"),
        "net.url" => ("url", "value"),
        "net.email" => ("email-addr", "value"),
        "account.user" => ("user-account", "user_id"),
        "registry.key" => ("windows-registry-key", "key"),
        "file.path" => ("directory", "path"),
        "file.name" => ("file", "name"),
        "file.hash.md5" | "file.hash.sha1" | "file.hash.sha256" => {
            // STIX takes a hash only when it is one: the right number of hex digits. A
            // truncated or mistyped value is still an entity, just not a `file`.
            let (algorithm, digits) = match type_ {
                "file.hash.md5" => ("MD5", 32),
                "file.hash.sha1" => ("SHA-1", 40),
                _ => ("SHA-256", 64),
            };
            if value.len() == digits && value.bytes().all(|b| b.is_ascii_hexdigit()) {
                p.insert("hashes".into(), json!({ algorithm: value }));
                return ("file", p);
            }
            p.insert("entity_type".into(), json!(type_));
            p.insert("value".into(), json!(value));
            return ("x-reactor-entity", p);
        }
        _ => {
            p.insert("entity_type".into(), json!(type_));
            p.insert("value".into(), json!(value));
            return ("x-reactor-entity", p);
        }
    };
    p.insert(kind.1.into(), json!(value));
    (kind.0, p)
}

fn time_json(t: &Time) -> Value {
    serde_json::to_value(t).unwrap_or_default()
}

/// What REactor knows about an element that STIX cannot hold.
struct Record<'a> {
    e: &'a Element,
    created: String,
    links: Vec<Value>,
}

fn x_reactor(r: &Record) -> Value {
    let c = &r.e.content;
    json!({
        "id": r.e.id,
        "kind": r.e.kind.as_str(),
        "type": r.e.type_,
        "version": r.e.ver,
        "status": c.status,
        "confidence": c.confidence,
        "value": c.value,
        "props": c.props,
        "tags": c.tags,
        "time": time_json(&c.time),
        "author": r.e.author,
        "recorded_at": r.e.recorded_at,
        "created_at": r.created,
        "links": r.links,
    })
}

/// Export the case's graph as a STIX 2.1 bundle.
pub fn export(case: &Case, opts: &ExportOptions) -> Result<Value> {
    let db = case.db();
    let (elements, edges) = query::snapshot(db, opts.include_deleted)?;
    let created: HashMap<String, String> = db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT element_id, min(recorded_at) FROM element_version GROUP BY element_id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })?;
    let directions: HashMap<String, String> = db.read(|conn| {
        let mut stmt =
            conn.prepare("SELECT name, direction FROM known_role WHERE direction IS NOT NULL")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    })?;
    let by_id: HashMap<&str, &Element> = elements.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut out_edges: BTreeMap<&str, Vec<&query::Edge>> = BTreeMap::new();
    for g in &edges {
        out_edges.entry(g.from_id.as_str()).or_default().push(g);
    }

    let authority = case.authority();
    // Which STIX object each element becomes, and so its id.
    let identity_id = stix_id("identity", &case.meta().id);
    let mut kind_of: HashMap<&str, String> = HashMap::new();
    let two_entities = |e: &Element| -> Option<(&query::Edge, &query::Edge)> {
        if e.kind != Kind::Observation {
            return None;
        }
        let parts: Vec<&query::Edge> = out_edges
            .get(e.id.as_str())?
            .iter()
            .copied()
            .filter(|g| g.role != "source")
            .collect();
        (parts.len() == 2
            && parts.iter().all(|g| {
                by_id
                    .get(g.to_id.as_str())
                    .is_some_and(|t| t.kind == Kind::Entity)
            }))
        .then(|| (parts[0], parts[1]))
    };
    for e in &elements {
        let k = match e.kind {
            Kind::External => continue,
            Kind::Entity => entity_object(&e.type_, e.content.value.as_deref().unwrap_or(""))
                .0
                .to_string(),
            Kind::Event => "observed-data".into(),
            Kind::Observation if two_entities(e).is_some() => "relationship".into(),
            Kind::Observation => "observed-data".into(),
            Kind::Annotation => match e.type_.as_str() {
                "analysis.judgement" => "opinion".into(),
                "analysis.cluster" => "grouping".into(),
                _ => "note".into(),
            },
        };
        kind_of.insert(e.id.as_str(), k);
    }
    let sid = |id: &str| -> Option<String> { kind_of.get(id).map(|k| stix_id(k, id)) };

    let mut objects: Vec<Value> = Vec::new();
    let meta = case.meta();
    objects.push(json!({
        // Both names of the case: the alias in the references, the id to tell two cases that
        // chose the same alias apart (ADR-0066).
        "x_reactor_case": { "id": meta.id, "alias": meta.alias },
        "type": "identity",
        "spec_version": SPEC_VERSION,
        "id": identity_id,
        "created": meta.created,
        "modified": meta.created,
        "name": meta.title.clone().unwrap_or_else(|| "REactor case".into()),
        "identity_class": "system",
        "description": "The REactor case this graph was exported from.",
    }));

    let mut sorted: Vec<&Element> = elements
        .iter()
        .filter(|e| e.kind != Kind::External)
        .collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    for e in sorted {
        let links: Vec<Value> = out_edges
            .get(e.id.as_str())
            .map(|v| {
                v.iter()
                    .map(|g| {
                        let target = by_id.get(g.to_id.as_str());
                        json!({
                            "id": g.id,
                            "role": g.role,
                            "to": {
                                "id": g.to_id,
                                "kind": target.map(|t| t.kind.as_str()),
                                "uri": target
                                    .filter(|t| t.kind == Kind::External)
                                    .and_then(|t| t.content.value.as_deref())
                                    .map(|u| crate::reference::canonicalize_text(u, &authority)),
                            },
                            "pinned_ver": g.pinned_ver,
                            "locator": g.locator,
                            "fingerprint": g.fingerprint,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rec = Record {
            e,
            created: created
                .get(&e.id)
                .cloned()
                .unwrap_or_else(|| e.recorded_at.clone()),
            links,
        };
        let id = sid(&e.id).expect("every exported element has a kind");
        let kind = kind_of[e.id.as_str()].as_str();
        let c = &e.content;
        let mut o = Map::new();
        o.insert("type".into(), json!(kind));
        o.insert("spec_version".into(), json!(SPEC_VERSION));
        o.insert("id".into(), json!(id));

        // Where it came from: what its source edges point at.
        let references: Vec<Value> = out_edges
            .get(e.id.as_str())
            .into_iter()
            .flatten()
            .filter(|g| g.role == "source")
            .filter_map(|g| by_id.get(g.to_id.as_str()))
            .filter_map(|x| x.content.value.clone())
            .map(|uri| crate::reference::canonicalize_text(&uri, &authority))
            .map(|uri| {
                if uri.starts_with("http://") || uri.starts_with("https://") {
                    json!({ "source_name": "reactor", "url": uri })
                } else {
                    json!({ "source_name": "reactor-case", "external_id": uri })
                }
            })
            .collect();
        let participants: Vec<String> = out_edges
            .get(e.id.as_str())
            .into_iter()
            .flatten()
            .filter(|g| g.role != "source")
            .filter_map(|g| sid(&g.to_id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let refs_or_identity = if participants.is_empty() {
            vec![identity_id.clone()]
        } else {
            participants.clone()
        };
        let common = |o: &mut Map<String, Value>| {
            o.insert("created".into(), json!(rec.created));
            o.insert("modified".into(), json!(e.recorded_at));
            o.insert("created_by_ref".into(), json!(identity_id));
            if let Some(n) = c.confidence.as_deref().and_then(confidence_number) {
                o.insert("confidence".into(), json!(n));
            }
            if !references.is_empty() {
                o.insert("external_references".into(), json!(references));
            }
        };

        match e.kind {
            Kind::Entity => {
                let (_, props) = entity_object(&e.type_, c.value.as_deref().unwrap_or(""));
                if kind == "x-reactor-entity" {
                    o.insert("created".into(), json!(rec.created));
                    o.insert("modified".into(), json!(e.recorded_at));
                    o.insert("created_by_ref".into(), json!(identity_id));
                }
                o.extend(props);
            }
            Kind::Event | Kind::Observation if kind == "observed-data" => {
                common(&mut o);
                let first = c
                    .time
                    .occurred_at
                    .clone()
                    .or_else(|| c.time.valid_from.clone())
                    .unwrap_or_else(|| e.recorded_at.clone());
                let last = match (&c.time.occurred_at, c.time.duration_ms, &c.time.valid_to) {
                    (Some(at), Some(ms), _) => add_ms(at, ms).unwrap_or_else(|| first.clone()),
                    (_, _, Some(to)) if *to >= first => to.clone(),
                    _ => first.clone(),
                };
                o.insert("first_observed".into(), json!(first));
                o.insert("last_observed".into(), json!(last));
                o.insert("number_observed".into(), json!(1));
                o.insert("object_refs".into(), json!(refs_or_identity));
                if let Some(v) = &c.value {
                    o.insert("x_description".into(), json!(v));
                }
            }
            Kind::Event | Kind::Observation => {
                common(&mut o);
                let (a, b) = two_entities(e).expect("classified as a relationship");
                let from_first = match (
                    directions.get(&a.role).map(String::as_str),
                    directions.get(&b.role).map(String::as_str),
                ) {
                    (Some("from"), _) | (_, Some("to")) => true,
                    (Some("to"), _) | (_, Some("from")) => false,
                    _ => a.to_id <= b.to_id,
                };
                let (from, to) = if from_first { (a, b) } else { (b, a) };
                o.insert(
                    "relationship_type".into(),
                    json!(relationship_type(&e.type_)),
                );
                o.insert("source_ref".into(), json!(sid(&from.to_id)));
                o.insert("target_ref".into(), json!(sid(&to.to_id)));
                if let Some(v) = &c.time.valid_from {
                    o.insert("start_time".into(), json!(v));
                }
                if let Some(v) = c
                    .time
                    .valid_to
                    .as_ref()
                    .filter(|to| c.time.valid_from.as_ref().is_none_or(|f| *to > f))
                {
                    o.insert("stop_time".into(), json!(v));
                }
                if let Some(v) = &c.value {
                    o.insert("description".into(), json!(v));
                }
                if !c.tags.is_empty() {
                    o.insert("labels".into(), json!(c.tags));
                }
            }
            Kind::Annotation => {
                common(&mut o);
                let text = c.value.clone().unwrap_or_else(|| e.type_.clone());
                match kind {
                    "opinion" => {
                        o.insert("opinion".into(), json!(opinion_of(c.confidence.as_deref())));
                        o.insert("explanation".into(), json!(text));
                    }
                    "grouping" => {
                        o.insert("context".into(), json!("unspecified"));
                        o.insert("name".into(), json!(text));
                    }
                    _ => {
                        o.insert("content".into(), json!(text));
                        o.insert("authors".into(), json!([e.author]));
                    }
                }
                o.insert("object_refs".into(), json!(refs_or_identity));
                let mut labels = c.tags.clone();
                labels.push(e.type_.trim_start_matches("analysis.").to_string());
                o.insert("labels".into(), json!(labels));
            }
            Kind::External => {}
        }
        o.insert("x_reactor".into(), x_reactor(&rec));
        objects.push(Value::Object(o));
    }
    let bundle_seed: String = objects
        .iter()
        .filter_map(|o| o["id"].as_str())
        .collect::<Vec<_>>()
        .join(",");
    Ok(json!({
        "type": "bundle",
        "id": format!("bundle--{}", uuid_of(&bundle_seed)),
        "objects": objects,
    }))
}

/// A timestamp moved on by some milliseconds, in the same format.
fn add_ms(at: &str, ms: i64) -> Option<String> {
    Some(crate::time::rfc3339(epoch_ms(at)? + ms.max(0) as u64))
}

fn epoch_ms(utc: &str) -> Option<u64> {
    // `YYYY-MM-DDTHH:MM:SS.mmmZ`, which is what this crate writes.
    let (date, rest) = utc.split_once('T')?;
    let rest = rest.trim_end_matches('Z');
    let (clock, ms) = rest.split_once('.').unwrap_or((rest, "0"));
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = clock.split(':').map(|x| x.parse::<i64>().ok());
    let (h, mi, s) = (t.next()??, t.next()??, t.next()??);
    let (yy, mm) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy.rem_euclid(400);
    let doy = (153 * mm + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = (days * 86_400 + h * 3600 + mi * 60 + s) * 1000 + ms.parse::<i64>().ok()?;
    u64::try_from(total).ok()
}

// -- validation ----------------------------------------------------------------------------------

fn is_id(s: &str, type_: &str) -> bool {
    let Some((t, u)) = s.split_once("--") else {
        return false;
    };
    let hex = |p: &str, n: usize| {
        p.len() == n
            && p.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    };
    let parts: Vec<&str> = u.split('-').collect();
    t == type_
        && parts.len() == 5
        && hex(parts[0], 8)
        && hex(parts[1], 4)
        && hex(parts[2], 4)
        && hex(parts[3], 4)
        && hex(parts[4], 12)
        && matches!(parts[2].as_bytes()[0], b'1'..=b'5')
        && matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b')
}

fn is_timestamp(s: &str) -> bool {
    // YYYY-MM-DDTHH:MM:SS(.fff...)Z
    let b = s.as_bytes();
    s.ends_with('Z')
        && s.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && s[..19]
            .bytes()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16) || c.is_ascii_digit())
}

/// Structural problems in a bundle: what the specification requires of the object types
/// this module writes. This is **not** the OASIS JSON Schema; it is what can be checked
/// without it, and it catches the ways an export can be wrong. Empty means none were found.
pub fn validate(bundle: &Value) -> Vec<String> {
    let mut bad = Vec::new();
    if bundle["type"] != "bundle" || !is_id(bundle["id"].as_str().unwrap_or(""), "bundle") {
        bad.push("a bundle needs type `bundle` and an id".into());
    }
    let Some(objects) = bundle["objects"].as_array() else {
        bad.push("a bundle needs `objects`".into());
        return bad;
    };
    let ids: BTreeSet<&str> = objects.iter().filter_map(|o| o["id"].as_str()).collect();
    if ids.len() != objects.len() {
        bad.push("two objects share an id".into());
    }
    for o in objects {
        let (t, id) = (
            o["type"].as_str().unwrap_or(""),
            o["id"].as_str().unwrap_or(""),
        );
        let at = |msg: &str| format!("{id}: {msg}");
        if !is_id(id, t) {
            bad.push(at("the id is not `<type>--<uuid>`"));
        }
        let sco = matches!(
            t,
            "ipv4-addr"
                | "ipv6-addr"
                | "domain-name"
                | "url"
                | "email-addr"
                | "file"
                | "user-account"
                | "windows-registry-key"
                | "directory"
        );
        if o["spec_version"] != SPEC_VERSION {
            bad.push(at("`spec_version` must be 2.1"));
        }
        if !sco {
            for f in ["created", "modified"] {
                if !is_timestamp(o[f].as_str().unwrap_or("")) {
                    bad.push(at(&format!("`{f}` is not a STIX timestamp")));
                }
            }
            if o["modified"].as_str() < o["created"].as_str() {
                bad.push(at("`modified` is before `created`"));
            }
        }
        let need = |names: &[&str], bad: &mut Vec<String>| {
            for n in names {
                if o.get(*n).is_none_or(Value::is_null) {
                    bad.push(at(&format!("`{n}` is required")));
                }
            }
        };
        let refs = |name: &str, bad: &mut Vec<String>| {
            let list = o[name]
                .as_array()
                .cloned()
                .unwrap_or_else(|| o.get(name).cloned().into_iter().collect());
            for r in &list {
                match r.as_str() {
                    Some(r) if ids.contains(r) => {}
                    other => bad.push(at(&format!(
                        "`{name}` names {other:?}, which is not in the bundle"
                    ))),
                }
            }
            if name == "object_refs" && list.is_empty() {
                bad.push(at("`object_refs` must not be empty"));
            }
        };
        match t {
            "identity" => need(&["name", "identity_class"], &mut bad),
            "observed-data" => {
                need(
                    &[
                        "first_observed",
                        "last_observed",
                        "number_observed",
                        "object_refs",
                    ],
                    &mut bad,
                );
                for f in ["first_observed", "last_observed"] {
                    if !is_timestamp(o[f].as_str().unwrap_or("")) {
                        bad.push(at(&format!("`{f}` is not a STIX timestamp")));
                    }
                }
                if o["last_observed"].as_str() < o["first_observed"].as_str() {
                    bad.push(at("`last_observed` is before `first_observed`"));
                }
                refs("object_refs", &mut bad);
            }
            "relationship" => {
                need(&["relationship_type", "source_ref", "target_ref"], &mut bad);
                refs("source_ref", &mut bad);
                refs("target_ref", &mut bad);
                let rt = o["relationship_type"].as_str().unwrap_or("");
                if rt.len() < 3
                    || !rt
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    bad.push(at(
                        "`relationship_type` is lowercase words joined by hyphens",
                    ));
                }
            }
            "note" => {
                need(&["content", "object_refs"], &mut bad);
                refs("object_refs", &mut bad);
            }
            "opinion" => {
                need(&["opinion", "object_refs"], &mut bad);
                refs("object_refs", &mut bad);
                if ![
                    "strongly-disagree",
                    "disagree",
                    "neutral",
                    "agree",
                    "strongly-agree",
                ]
                .contains(&o["opinion"].as_str().unwrap_or(""))
                {
                    bad.push(at("`opinion` is not one of the five values"));
                }
            }
            "grouping" => {
                need(&["context", "object_refs"], &mut bad);
                refs("object_refs", &mut bad);
            }
            "ipv4-addr" | "ipv6-addr" | "domain-name" | "url" | "email-addr" => {
                need(&["value"], &mut bad)
            }
            "user-account" => need(&["user_id"], &mut bad),
            "windows-registry-key" => need(&["key"], &mut bad),
            "directory" => need(&["path"], &mut bad),
            "file" => {
                if o.get("hashes").is_none() && o.get("name").is_none() {
                    bad.push(at("a file needs `hashes` or `name`"));
                }
            }
            t if t.starts_with("x-") => need(&["created", "modified"], &mut bad),
            other => bad.push(at(&format!("`{other}` is a type this export never writes"))),
        }
        if let Some(c) = o.get("confidence")
            && !c.as_u64().is_some_and(|n| n <= 100)
        {
            bad.push(at("`confidence` is 0 to 100"));
        }
        if let Some(by) = o.get("created_by_ref") {
            refs("created_by_ref", &mut bad);
            let _ = by;
        }
    }
    bad
}

// -- import ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub elements: usize,
    pub edges: usize,
    /// Objects this could not map, and why.
    pub skipped: Vec<String>,
    /// Whether the bundle carried `x_reactor` and was restored exactly.
    pub restored: bool,
}

fn parse_author(s: &str) -> Author {
    let (kind, rest) = s.split_once(':').unwrap_or(("tool", s));
    let (identity, version) = rest.split_once('@').unwrap_or((rest, ""));
    Author {
        kind: if matches!(kind, "analyst" | "tool" | "agent") {
            kind
        } else {
            "tool"
        }
        .to_string(),
        identity: identity.to_string(),
        version: version.to_string(),
    }
}

/// A reference the bundle's own case wrote with its name is the short reference again: it
/// was the same thing inside that case, and is the same thing inside this one.
fn unqualify(uri: &str, own: &[String]) -> String {
    match crate::reference::parse(uri) {
        Ok(r)
            if r.case
                .as_deref()
                .is_some_and(|c| own.iter().any(|o| o.eq_ignore_ascii_case(c))) =>
        {
            r.short()
        }
        _ => uri.to_string(),
    }
}

struct Restored {
    put: Put,
    needs: Vec<String>,
}

fn restore(x: &Value, own: &[String]) -> Option<Restored> {
    let kind = Kind::parse(x["kind"].as_str()?)?;
    if kind == Kind::External {
        return None;
    }
    let time: Time = serde_json::from_value(x["time"].clone()).unwrap_or_default();
    let content = Content {
        value: x["value"].as_str().map(str::to_string),
        props: x["props"].as_object().cloned().unwrap_or_default(),
        tags: x["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        status: x["status"].as_str().unwrap_or("active").to_string(),
        confidence: x["confidence"].as_str().map(str::to_string),
        time,
    };
    let mut needs = Vec::new();
    let mut links = Vec::new();
    for l in x["links"].as_array().into_iter().flatten() {
        let to = &l["to"];
        let target = match to["kind"].as_str() {
            Some("external") => Target::External {
                uri: unqualify(to["uri"].as_str()?, own),
            },
            _ => {
                let id = to["id"].as_str()?.to_string();
                needs.push(id.clone());
                Target::Id(id)
            }
        };
        links.push(Link {
            role: l["role"].as_str()?.to_string(),
            to: target,
            pin: l["pinned_ver"].as_u64().is_some(),
            locator: l["locator"].as_str().map(str::to_string),
            fingerprint: l["fingerprint"].as_str().map(str::to_string),
            id: l["id"].as_str().map(str::to_string),
        });
    }
    let mut put = Put::new(kind, x["type"].as_str()?);
    put.id = Some(x["id"].as_str()?.to_string());
    put.content = content;
    put.links = links;
    put.author = x["author"].as_str().map(parse_author);
    put.recorded_at = x["created_at"].as_str().map(str::to_string);
    Some(Restored { put, needs })
}

/// Import a bundle. One with `x_reactor` on its objects is restored; the rest is mapped.
pub fn import(case: &Case, bundle: &Value, name: &str) -> Result<Imported> {
    if bundle["type"] != "bundle" {
        return Err(Error::Invalid(
            "not a STIX bundle (no `type: bundle`)".into(),
        ));
    }
    let objects = bundle["objects"]
        .as_array()
        .ok_or_else(|| Error::Invalid("a bundle needs `objects`".into()))?;
    let mut report = Imported::default();
    let importer = Author {
        kind: "tool".into(),
        identity: "stix-import".into(),
        version: name.to_string(),
    };
    let mut batch = Batch::new(importer);
    batch.kind = "import".into();
    batch.payload = json!({ "format": "stix", "bundle": bundle["id"], "name": name });

    // The names the bundle's case went by.
    let own: Vec<String> = objects
        .iter()
        .filter_map(|o| o.get("x_reactor_case"))
        .flat_map(|c| [c["id"].as_str(), c["alias"].as_str()])
        .flatten()
        .map(str::to_string)
        .collect();
    let restored: Vec<Restored> = objects
        .iter()
        .filter_map(|o| restore(&o["x_reactor"], &own))
        .collect();
    if !restored.is_empty() {
        report.restored = true;
        // Everything an element points at is written before it, so a link finds its end.
        let mut pending: Vec<Restored> = restored;
        let mut placed: BTreeSet<String> = BTreeSet::new();
        let all: BTreeSet<String> = pending.iter().filter_map(|r| r.put.id.clone()).collect();
        while !pending.is_empty() {
            let before = pending.len();
            let mut rest = Vec::new();
            for r in pending {
                let ready = r
                    .needs
                    .iter()
                    .all(|n| placed.contains(n) || !all.contains(n));
                if ready {
                    // A link to something that is not in the bundle (soft-deleted when it was
                    // exported) cannot be made; the rest of the element is.
                    let mut r = r;
                    r.put.links.retain(|l| match &l.to {
                        Target::Id(id) => placed.contains(id),
                        _ => true,
                    });
                    placed.extend(r.put.id.clone());
                    batch.puts.push(r.put);
                } else {
                    rest.push(r);
                }
            }
            if rest.len() == before {
                return Err(Error::Invalid(
                    "the bundle's elements refer to each other in a circle".into(),
                ));
            }
            pending = rest;
        }
    } else {
        map_foreign(objects, name, &mut batch, &mut report);
    }
    report.elements = batch.puts.len();
    report.edges = batch.puts.iter().map(|p| p.links.len()).sum();
    if !batch.puts.is_empty() {
        graph::apply(case.db(), batch)?;
    }
    Ok(report)
}

/// A foreign bundle, as far as it maps: observables become entities, and what relates
/// them becomes claims sourced to the bundle.
fn map_foreign(objects: &[Value], name: &str, batch: &mut Batch, report: &mut Imported) {
    let source = format!("stix:{name}");
    // STIX id → the entity it is.
    let mut entities: HashMap<String, (String, String)> = HashMap::new();
    for o in objects {
        let t = o["type"].as_str().unwrap_or("");
        let text = |k: &str| o[k].as_str().map(str::to_string);
        let mapped: Option<(String, String)> = match t {
            "ipv4-addr" => text("value").map(|v| ("net.ipv4".to_string(), v)),
            "ipv6-addr" => text("value").map(|v| ("net.ipv6".to_string(), v)),
            "domain-name" => text("value").map(|v| ("net.domain".to_string(), v)),
            "url" => text("value").map(|v| ("net.url".to_string(), v)),
            "email-addr" => text("value").map(|v| ("net.email".to_string(), v)),
            "user-account" => text("user_id")
                .or_else(|| text("account_login"))
                .map(|v| ("account.user".to_string(), v)),
            "windows-registry-key" => text("key").map(|v| ("registry.key".to_string(), v)),
            "directory" => text("path").map(|v| ("file.path".to_string(), v)),
            "x-reactor-entity" => match (o["entity_type"].as_str(), text("value")) {
                (Some(ty), Some(v)) => Some((ty.to_string(), v)),
                _ => None,
            },
            "file" => o["hashes"]
                .as_object()
                .and_then(|h| {
                    [
                        ("SHA-256", "file.hash.sha256"),
                        ("SHA-1", "file.hash.sha1"),
                        ("MD5", "file.hash.md5"),
                    ]
                    .iter()
                    .find_map(|(k, ty)| {
                        h.get(*k)
                            .and_then(Value::as_str)
                            .map(|v| (ty.to_string(), v.to_string()))
                    })
                })
                .or_else(|| text("name").map(|v| ("file.name".to_string(), v))),
            _ => None,
        };
        if let (Some((ty, v)), Some(id)) = (mapped, o["id"].as_str()) {
            batch.puts.push(Put::entity(&ty, &v));
            entities.insert(id.to_string(), (ty, v));
        }
    }
    let entity_link =
        |role: &str, id: &str| entities.get(id).map(|(ty, v)| Link::entity(role, ty, v));
    let when = |o: &Value, k: &str| o[k].as_str().map(str::to_string);
    for o in objects {
        let t = o["type"].as_str().unwrap_or("");
        let id = o["id"].as_str().unwrap_or("?");
        let refs: Vec<&str> = o["object_refs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        match t {
            "relationship" => {
                let (Some(a), Some(b)) = (
                    o["source_ref"].as_str().and_then(|r| entity_link("src", r)),
                    o["target_ref"].as_str().and_then(|r| entity_link("dst", r)),
                ) else {
                    report.skipped.push(format!(
                        "{id}: a relationship between things this does not map"
                    ));
                    continue;
                };
                let mut put = Put::new(
                    Kind::Observation,
                    &format!(
                        "stix.{}",
                        o["relationship_type"].as_str().unwrap_or("related-to")
                    ),
                );
                put.content.time.valid_from = when(o, "start_time");
                put.content.time.valid_to = when(o, "stop_time");
                put.content.value = o["description"].as_str().map(str::to_string);
                put.links = vec![a, b, Link::source(&source).at(id, None)];
                batch.puts.push(put);
            }
            "observed-data" => {
                let links: Vec<Link> = refs
                    .iter()
                    .filter_map(|r| entity_link("object", r))
                    .collect();
                if links.is_empty() {
                    report
                        .skipped
                        .push(format!("{id}: observed data about nothing this maps"));
                    continue;
                }
                let mut put = Put::new(Kind::Event, "stix.observed-data");
                put.content.time.occurred_at = when(o, "first_observed");
                put.content.time.raw = when(o, "first_observed");
                put.links = links;
                put.links.push(Link::source(&source).at(id, None));
                batch.puts.push(put);
            }
            "note" | "opinion" | "grouping" => {
                let (type_, text, role) = match t {
                    "note" => ("analysis.note", o["content"].as_str(), "about"),
                    "opinion" => ("analysis.judgement", o["explanation"].as_str(), "about"),
                    _ => ("analysis.cluster", o["name"].as_str(), "member"),
                };
                let mut put = Put::new(Kind::Annotation, type_);
                put.content.value = text.map(str::to_string);
                if t == "opinion" {
                    put.content.confidence = o["opinion"]
                        .as_str()
                        .and_then(words_of_opinion)
                        .map(str::to_string);
                }
                put.links = refs.iter().filter_map(|r| entity_link(role, r)).collect();
                batch.puts.push(put);
            }
            "identity" | "bundle" => {}
            ty if entities.contains_key(id) || ty.is_empty() => {}
            other => report
                .skipped
                .push(format!("{id}: `{other}` is not mapped")),
        }
    }
}
