//! References: one grammar for what a person, the agent and an export point at
//! ([ADR-0066](../../../../docs/adr/0066-the-case-is-addressed-by-references.md)).
//!
//! A reference is a string, `kind:rest`, whose body is a key the database already has:
//!
//! ```text
//! session:<session id>#entry=<n>          an entry of a session
//! graph:<element id>[@<version>]          an element of the graph; no version is the latest
//! graph:entity/<type>/<value>[?scope=s]   an entity by its natural key
//! artifact:<path>[#<locator>]             a file of the case, and a place in it
//! doc:<document id>[#passage=<n>]         a context document, and one of its passages
//! blob:<sha256>                           stored bytes
//! change:<n>                              one write batch of the change log
//! vocab:<type>                            a type of the vocabulary
//! <kind>:?a=b&c=d                         a set: the references that match
//! ```
//!
//! The *short* form is relative to the open case. The *canonical* form,
//! `reactor://<case>/<kind>/<rest>`, names the case (its alias, else its id) and is what
//! leaves it. [`parse`] reads both; [`Reference::short`] and [`Reference::canonical`] write
//! them. A locator is kept as written: what it means depends on the file it is in
//! ([`Locator`]).
//!
//! This module is the grammar and knows no database; [`lookup`] is what answers for one.

pub mod lookup;

use std::fmt;

use crate::error::{Error, Result};

/// The kinds of thing a reference can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Session,
    Graph,
    Artifact,
    Doc,
    Blob,
    Change,
    Vocab,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Session,
        Kind::Graph,
        Kind::Artifact,
        Kind::Doc,
        Kind::Blob,
        Kind::Change,
        Kind::Vocab,
    ];

    /// The scheme it is written with.
    pub fn scheme(self) -> &'static str {
        match self {
            Kind::Session => "session",
            Kind::Graph => "graph",
            Kind::Artifact => "artifact",
            Kind::Doc => "doc",
            Kind::Blob => "blob",
            Kind::Change => "change",
            Kind::Vocab => "vocab",
        }
    }

    pub fn from_scheme(s: &str) -> Option<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.scheme().eq_ignore_ascii_case(s))
    }
}

/// How a graph element is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphId {
    /// The element's id, as the database stores it.
    Id(String),
    /// An entity by what it is, which the id is derived from (ADR-0047). It need not exist.
    Entity {
        type_: String,
        value: String,
        scope: Option<String>,
    },
}

/// What a reference points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Session {
        session: String,
        entry: Option<u64>,
    },
    Graph {
        id: GraphId,
        ver: Option<u32>,
    },
    Artifact {
        path: String,
        /// After the `#`, as written.
        locator: Option<String>,
    },
    Doc {
        id: String,
        passage: Option<u32>,
    },
    Blob {
        sha256: String,
    },
    Change {
        seq: u64,
    },
    Vocab {
        name: String,
    },
    /// The references of a kind that match the filters.
    Set {
        kind: Kind,
        params: Vec<(String, String)>,
    },
}

impl Target {
    pub fn kind(&self) -> Kind {
        match self {
            Target::Session { .. } => Kind::Session,
            Target::Graph { .. } => Kind::Graph,
            Target::Artifact { .. } => Kind::Artifact,
            Target::Doc { .. } => Kind::Doc,
            Target::Blob { .. } => Kind::Blob,
            Target::Change { .. } => Kind::Change,
            Target::Vocab { .. } => Kind::Vocab,
            Target::Set { kind, .. } => *kind,
        }
    }
}

/// A reference: what it points at, and the case it names when it is written out of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// `None` is the open case.
    pub case: Option<String>,
    pub target: Target,
}

impl Reference {
    pub fn new(target: Target) -> Reference {
        Reference { case: None, target }
    }

    pub fn session(session: &str, entry: u64) -> Reference {
        Reference::new(Target::Session {
            session: session.to_string(),
            entry: Some(entry),
        })
    }

    pub fn graph(id: &str) -> Reference {
        Reference::new(Target::Graph {
            id: GraphId::Id(id.to_string()),
            ver: None,
        })
    }

    pub fn artifact(path: &str) -> Reference {
        Reference::new(Target::Artifact {
            path: path.to_string(),
            locator: None,
        })
    }

    pub fn kind(&self) -> Kind {
        self.target.kind()
    }

    /// The form relative to the open case.
    pub fn short(&self) -> String {
        write_short(&self.target)
    }

    /// The form that names the case: `reactor://<case>/<kind>/<rest>`.
    pub fn canonical(&self, case: &str) -> String {
        let short = write_short(&self.target);
        let (scheme, rest) = short.split_once(':').unwrap_or((&short, ""));
        if rest.starts_with('?') || rest.starts_with('#') {
            format!("reactor://{case}/{scheme}{rest}")
        } else {
            format!("reactor://{case}/{scheme}/{rest}")
        }
    }

    /// The same reference, with its graph version pinned (the form an export writes).
    pub fn pinned(mut self, ver: u32) -> Reference {
        if let Target::Graph { ver: v @ None, .. } = &mut self.target {
            *v = Some(ver);
        }
        self
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.case {
            Some(case) => f.write_str(&self.canonical(case)),
            None => f.write_str(&self.short()),
        }
    }
}

// -- schemes programs register -------------------------------------------------------------

static REGISTERED: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// Let a scheme other than the case's own be stored as a source (a program that registered
/// it; the interface for that is a later ADR). It is not a kind of reference: nothing here
/// resolves it.
pub fn register_scheme(scheme: &str) {
    let scheme = scheme.trim().to_ascii_lowercase();
    if Kind::from_scheme(&scheme).is_some() || scheme.is_empty() {
        return;
    }
    let mut all = REGISTERED.write().unwrap();
    if !all.contains(&scheme) {
        all.push(scheme);
    }
}

/// Whether `scheme` may be stored as a source besides the case's own: `stix`, which the
/// bundle importer writes ("this came from that bundle"), and any a program registered.
pub fn is_registered(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("stix")
        || REGISTERED
            .read()
            .unwrap()
            .iter()
            .any(|s| s.eq_ignore_ascii_case(scheme))
}

// -- writing ------------------------------------------------------------------------------

fn write_short(t: &Target) -> String {
    match t {
        Target::Session { session, entry } => {
            let mut s = format!("session:{}", enc(session, ""));
            if let Some(n) = entry {
                s.push_str(&format!("#entry={n}"));
            }
            s
        }
        Target::Graph { id, ver } => match id {
            GraphId::Id(id) => match ver {
                Some(v) => format!("graph:{}@{v}", enc(id, "")),
                None => format!("graph:{}", enc(id, "")),
            },
            GraphId::Entity {
                type_,
                value,
                scope,
            } => {
                let mut s = format!("graph:entity/{}/{}", enc(type_, ""), enc(value, "/"));
                if let Some(scope) = scope {
                    s.push_str(&format!("?scope={}", enc(scope, "&=+/")));
                }
                s
            }
        },
        Target::Artifact { path, locator } => {
            let mut s = format!("artifact:{}", enc(path, ""));
            if let Some(l) = locator {
                s.push('#');
                s.push_str(l);
            }
            s
        }
        Target::Doc { id, passage } => {
            let mut s = format!("doc:{}", enc(id, ""));
            if let Some(n) = passage {
                s.push_str(&format!("#passage={n}"));
            }
            s
        }
        Target::Blob { sha256 } => format!("blob:{sha256}"),
        Target::Change { seq } => format!("change:{seq}"),
        Target::Vocab { name } => format!("vocab:{}", enc(name, "")),
        Target::Set { kind, params } => {
            let mut s = format!("{}:?", kind.scheme());
            let q: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("{}={}", enc(k, "&=+"), enc(v, "&=+")))
                .collect();
            s.push_str(&q.join("&"));
            s
        }
    }
}

/// Percent-encode what would end a reference in running text or be read as syntax, plus
/// the characters in `also`.
fn enc(s: &str, also: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let plain = !c.is_whitespace()
            && !c.is_control()
            && !matches!(c, '%' | '#' | '?' | '"' | '<' | '>' | '`')
            && !also.contains(c);
        if plain {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn dec(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| {
                    Error::Invalid(format!("`{s}`: a `%` must be followed by two hex digits"))
                })?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|_| Error::Invalid(format!("`{s}`: not valid UTF-8 once decoded")))
}

// -- reading ------------------------------------------------------------------------------

fn bad(raw: &str, why: &str) -> Error {
    Error::Invalid(format!("`{raw}`: {why}"))
}

/// Whether a case part is a name a case can have: an alias, or an id.
fn valid_authority(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 64
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `s` begins like a reference (a known scheme, or `reactor://`), without saying
/// whether the rest is well formed.
pub fn looks_like(s: &str) -> bool {
    let s = s.trim_start();
    if s.starts_with("reactor://") {
        return true;
    }
    s.split_once(':')
        .is_some_and(|(scheme, rest)| !rest.is_empty() && Kind::from_scheme(scheme).is_some())
}

/// Read a reference, in either form.
pub fn parse(raw: &str) -> Result<Reference> {
    let s = raw.trim();
    if let Some(rest) = s.strip_prefix("reactor://") {
        let (case, tail) = rest
            .split_once('/')
            .ok_or_else(|| bad(raw, "a canonical reference is reactor://<case>/<kind>/..."))?;
        if !valid_authority(case) {
            return Err(bad(
                raw,
                "the case is an alias or an id: letters, digits, `-` and `_`",
            ));
        }
        // `<kind>` ends at the first `/`, `?` or `#`; the rest is the short form's.
        let end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        let (scheme, after) = tail.split_at(end);
        let after = after.strip_prefix('/').unwrap_or(after);
        let mut r = parse_short(&format!("{scheme}:{after}"), raw)?;
        r.case = Some(case.to_string());
        return Ok(r);
    }
    parse_short(s, raw)
}

fn parse_short(s: &str, raw: &str) -> Result<Reference> {
    let (scheme, rest) = s
        .split_once(':')
        .ok_or_else(|| bad(raw, "not a reference: no `kind:`"))?;
    let kind = Kind::from_scheme(scheme).ok_or_else(|| {
        bad(
            raw,
            "not a kind of reference (session, graph, artifact, doc, blob, change, vocab)",
        )
    })?;
    let (before_frag, frag) = match rest.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (rest, None),
    };
    let (body, query) = match before_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_frag, None),
    };
    if body.is_empty()
        && let Some(q) = query
    {
        if frag.is_some() {
            return Err(bad(raw, "a set has no `#`"));
        }
        return Ok(Reference::new(Target::Set {
            kind,
            params: parse_query(q, raw)?,
        }));
    }
    if body.is_empty() {
        return Err(bad(raw, "nothing after the kind"));
    }
    let only_frag = |want: &str| -> Result<Option<String>> {
        match frag {
            None => Ok(None),
            Some(f) => f
                .strip_prefix(want)
                .map(|v| Some(v.to_string()))
                .ok_or_else(|| bad(raw, &format!("only `#{want}<n>` can follow this kind"))),
        }
    };
    let number =
        |v: &str| -> Result<u64> { v.parse::<u64>().map_err(|_| bad(raw, "expected a number")) };
    let target = match kind {
        Kind::Session => Target::Session {
            session: dec(body)?,
            entry: only_frag("entry=")?.as_deref().map(number).transpose()?,
        },
        Kind::Graph => {
            if frag.is_some() {
                return Err(bad(raw, "a graph reference has no `#`"));
            }
            if let Some(key) = body.strip_prefix("entity/") {
                let (type_, value) = key
                    .split_once('/')
                    .filter(|(t, v)| !t.is_empty() && !v.is_empty())
                    .ok_or_else(|| bad(raw, "an entity is graph:entity/<type>/<value>"))?;
                let scope = match query {
                    None => None,
                    Some(q) => parse_query(q, raw)?
                        .into_iter()
                        .find(|(k, _)| k == "scope")
                        .map(|(_, v)| v),
                };
                Target::Graph {
                    id: GraphId::Entity {
                        type_: dec(type_)?.to_lowercase(),
                        value: dec(value)?,
                        scope,
                    },
                    ver: None,
                }
            } else {
                if query.is_some() {
                    return Err(bad(raw, "only an entity takes `?scope=`"));
                }
                let (id, ver) = match body.rsplit_once('@') {
                    Some((i, v)) => (
                        i,
                        Some(
                            v.parse::<u32>()
                                .map_err(|_| bad(raw, "a version is `@<n>`"))?,
                        ),
                    ),
                    None => (body, None),
                };
                if id.contains('/') {
                    return Err(bad(raw, "an element id has no `/`"));
                }
                Target::Graph {
                    id: GraphId::Id(dec(id)?),
                    ver,
                }
            }
        }
        Kind::Artifact => {
            if query.is_some() {
                return Err(bad(raw, "a file takes a `#locator`, not `?`"));
            }
            let path = dec(body)?.replace('\\', "/");
            if path.starts_with('/') || path.split('/').any(|p| p == "..") {
                return Err(bad(
                    raw,
                    "a file is named relative to the case and stays inside it",
                ));
            }
            Target::Artifact {
                path,
                locator: frag.map(str::to_string).filter(|f| !f.is_empty()),
            }
        }
        Kind::Doc => Target::Doc {
            id: dec(body)?,
            passage: only_frag("passage=")?
                .as_deref()
                .map(|v| number(v).map(|n| n as u32))
                .transpose()?,
        },
        Kind::Blob => {
            let h = body.to_ascii_lowercase();
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(bad(raw, "a blob is its 64-digit SHA-256"));
            }
            Target::Blob { sha256: h }
        }
        Kind::Change => Target::Change { seq: number(body)? },
        Kind::Vocab => Target::Vocab {
            name: dec(body)?.to_lowercase(),
        },
    };
    if query.is_some() && !matches!(kind, Kind::Graph) {
        return Err(bad(raw, "`?` makes a set: write `kind:?filters`"));
    }
    Ok(Reference::new(target))
}

fn parse_query(q: &str, raw: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for part in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let k = dec(k)?;
        if k.is_empty() {
            return Err(bad(raw, "a filter needs a name"));
        }
        out.push((k, dec(v)?));
    }
    Ok(out)
}

/// The value of a filter of a set, if it has one.
pub fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

// -- finding them in text -------------------------------------------------------------------

/// A reference found in running text, with the span it was written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub reference: Reference,
    pub span: std::ops::Range<usize>,
}

/// The references in `text`. One starts at the beginning, after whitespace or after an
/// opening bracket or quote, and runs to the next whitespace; sentence punctuation and a
/// closing bracket after it are not part of it. Text that begins like a reference but does
/// not parse is left alone, so prose such as `type: x` costs nothing.
pub fn find(text: &str) -> Vec<Found> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let starts = (text.is_char_boundary(i))
            && (i == 0
                || text[..i].chars().next_back().is_some_and(|c| {
                    c.is_whitespace() || matches!(c, '(' | '[' | '"' | '\'' | '`' | '<')
                }));
        if !starts || !(bytes[i].is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..len];
        let trimmed =
            word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '`', '>']);
        if looks_like(trimmed)
            && let Ok(reference) = parse(trimmed)
        {
            let end = i + trimmed.len();
            out.push(Found {
                reference,
                span: i..end,
            });
            i = end;
            continue;
        }
        i += word.len().max(1);
    }
    out
}

/// A source written as a reference with a place in it, taken apart the way the graph keeps
/// it: what the External is (a file, a table of a file, an array of a JSON file) and the place
/// on the edge that points to it (`row=12`). `artifact:data/a.csv#row=12` is the file and
/// `row=12`; `artifact:db.sqlite#table=t&row=7` is the table and `row=7`. Anything else is
/// returned as it was, with no place.
pub fn split_source(raw: &str) -> (String, Option<String>) {
    let Ok(mut r) = parse(raw) else {
        return (raw.to_string(), None);
    };
    let Target::Artifact { path, locator } = &mut r.target else {
        return (raw.to_string(), None);
    };
    let Some(loc) = locator.clone() else {
        return (raw.to_string(), None);
    };
    let _ = path;
    match Locator::parse(&loc) {
        Some(Locator::Table {
            table,
            row: Some(n),
        }) => {
            *locator = Some(format!("table={}", enc(&table, "&=")));
            (r.short(), Some(format!("row={n}")))
        }
        Some(
            Locator::Row(_)
            | Locator::Col(_)
            | Locator::Cell(..)
            | Locator::Line(_)
            | Locator::Char(_)
            | Locator::Bytes { .. },
        ) => {
            *locator = None;
            (r.short(), Some(loc))
        }
        _ => (raw.to_string(), None),
    }
}

/// `raw` as the open case writes it: a reference that names this case loses the case, so the
/// same thing has one spelling inside it. Anything else (another case's reference, a URL,
/// text) is returned as it was.
pub fn localize(case: &crate::case::Case, raw: &str) -> String {
    match parse(raw) {
        Ok(r) if r.case.as_deref().is_some_and(|c| case.is_named(c)) => r.short(),
        _ => raw.to_string(),
    }
}

/// `text` with each short reference in it written out with the case's name: what leaves the
/// case in an export, where "this case" would mean nothing. References that name a case
/// already, and everything that is not one, are left as they are.
pub fn canonicalize_text(text: &str, authority: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in find(text) {
        if found.reference.case.is_some() {
            continue;
        }
        out.push_str(&text[last..found.span.start]);
        out.push_str(&found.reference.canonical(authority));
        last = found.span.end;
    }
    out.push_str(&text[last..]);
    out
}

// -- locators -----------------------------------------------------------------------------

/// A place in a file, read from a reference's `#...` once the file's kind is known. The
/// forms of text, CSV and JSON are the standard fragments (RFC 5147, RFC 7111, RFC 6901).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    /// `line=12`, or `line=12,14` (the first is the place).
    Line(u64),
    /// `char=340`.
    Char(u64),
    Row(u64),
    Col(u64),
    /// `cell=12,3`.
    Cell(u64, u64),
    /// `/events/3`.
    JsonPointer(String),
    /// `table=fw_conn`, and `&row=<rowid>` for one row of it.
    Table {
        table: String,
        row: Option<i64>,
    },
    /// `offset=0x1a0&length=16`.
    Bytes {
        offset: u64,
        length: Option<u64>,
    },
}

impl Locator {
    pub fn parse(s: &str) -> Option<Locator> {
        let s = s.trim().trim_start_matches('#');
        if s.starts_with('/') {
            return Some(Locator::JsonPointer(s.to_string()));
        }
        let pairs: Vec<(&str, &str)> = s.split('&').filter_map(|p| p.split_once('=')).collect();
        let get = |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
        let first = |v: &str| v.split([',', '-']).next().and_then(number);
        if let Some(t) = get("table") {
            return Some(Locator::Table {
                table: dec(t).ok()?,
                row: get("row").and_then(|r| r.parse().ok()),
            });
        }
        if let Some(o) = get("offset") {
            return Some(Locator::Bytes {
                offset: number(o)?,
                length: get("length").and_then(number),
            });
        }
        if let Some(v) = get("line") {
            return first(v).map(Locator::Line);
        }
        if let Some(v) = get("char") {
            return first(v).map(Locator::Char);
        }
        if let Some(v) = get("cell") {
            let (r, c) = v.split_once(',')?;
            return Some(Locator::Cell(number(r)?, number(c)?));
        }
        if let Some(v) = get("row") {
            return first(v).map(Locator::Row);
        }
        if let Some(v) = get("col") {
            return first(v).map(Locator::Col);
        }
        None
    }
}

fn number(v: &str) -> Option<u64> {
    let v = v.trim();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(short: &str) {
        let r = parse(short).unwrap_or_else(|e| panic!("{short}: {e}"));
        assert_eq!(r.short(), short, "short round trip");
        let c = r.canonical("acme");
        let back = parse(&c).unwrap_or_else(|e| panic!("{c}: {e}"));
        assert_eq!(back.case.as_deref(), Some("acme"));
        assert_eq!(back.target, r.target, "canonical round trip of {c}");
        assert_eq!(back.to_string(), c);
    }

    #[test]
    fn every_kind_round_trips_in_both_forms() {
        for s in [
            "session:1759-1",
            "session:1759-1#entry=42",
            "graph:entity--0123456789abcdef0123456789abcdef",
            "graph:annotation--0123456789abcdef0123456789abcdef@3",
            "graph:entity/net.ipv4/198.51.100.7",
            "graph:entity/net.ipv4/10.0.0.1?scope=site-a",
            "artifact:data/raw/vpn.csv",
            "artifact:data/raw/vpn.csv#row=12",
            "artifact:reactor_data.sqlite#table=fw_conn&row=7",
            "artifact:a/b%20c.txt#line=4",
            "doc:0190aa",
            "doc:0190aa#passage=3",
            "change:17",
            "vocab:net.ipv4",
            "graph:?type=net.&tag=c2&limit=40",
            "session:?",
            "artifact:?text=vpn",
        ] {
            round(s);
        }
        let hash = "a".repeat(64);
        round(&format!("blob:{hash}"));
    }

    #[test]
    fn a_canonical_reference_names_its_case() {
        let r = parse("reactor://acme/artifact/data/vpn.csv#row=2").unwrap();
        assert_eq!(r.case.as_deref(), Some("acme"));
        assert_eq!(
            r.target,
            Target::Artifact {
                path: "data/vpn.csv".into(),
                locator: Some("row=2".into())
            }
        );
        let q = parse("reactor://0190-aa/graph?kind=entity").unwrap();
        assert!(matches!(
            q.target,
            Target::Set {
                kind: Kind::Graph,
                ..
            }
        ));
        assert!(parse("reactor://bad name/graph/x").is_err());
        assert!(parse("reactor://acme").is_err());
    }

    #[test]
    fn values_with_syntax_in_them_are_encoded() {
        let r = Reference::new(Target::Graph {
            id: GraphId::Entity {
                type_: "host.name".into(),
                value: "a/b c#d?e".into(),
                scope: None,
            },
            ver: None,
        });
        let s = r.short();
        assert!(
            !s.contains(' ') && !s.contains('#') && !s.contains('?'),
            "{s}"
        );
        assert_eq!(parse(&s).unwrap().target, r.target);
    }

    #[test]
    fn what_is_not_a_reference_is_refused() {
        for bad in [
            "",
            "foo:bar",
            "graph:",
            "artifact:/etc/passwd",
            "artifact:../x",
            "blob:xyz",
            "change:abc",
            "graph:entity--abc@x",
            "session:1#entry=x",
            "doc:1#2.2",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn references_are_found_in_text_and_prose_is_left_alone() {
        let text = "see artifact:data/a.csv#row=2, and (graph:entity--0123456789abcdef0123456789abcdef). \
                    type: x, note: this, https://example.com and session:1#entry=3.";
        let found = find(text);
        let got: Vec<String> = found.iter().map(|f| f.reference.short()).collect();
        assert_eq!(
            got,
            [
                "artifact:data/a.csv#row=2",
                "graph:entity--0123456789abcdef0123456789abcdef",
                "session:1#entry=3"
            ]
        );
        for f in &found {
            assert!(parse(&text[f.span.clone()]).is_ok());
        }
    }

    #[test]
    fn locators_follow_the_standards() {
        assert_eq!(Locator::parse("line=12"), Some(Locator::Line(12)));
        assert_eq!(Locator::parse("row=5-7"), Some(Locator::Row(5)));
        assert_eq!(Locator::parse("cell=2,3"), Some(Locator::Cell(2, 3)));
        assert_eq!(
            Locator::parse("/events/3"),
            Some(Locator::JsonPointer("/events/3".into()))
        );
        assert_eq!(
            Locator::parse("table=t&row=9"),
            Some(Locator::Table {
                table: "t".into(),
                row: Some(9)
            })
        );
        assert_eq!(
            Locator::parse("offset=0x1a0&length=16"),
            Some(Locator::Bytes {
                offset: 0x1a0,
                length: Some(16)
            })
        );
        assert_eq!(Locator::parse("nonsense"), None);
    }

    #[test]
    fn a_source_is_split_into_what_it_is_and_where_in_it() {
        assert_eq!(
            split_source("artifact:data/a.csv#row=12"),
            (
                "artifact:data/a.csv".to_string(),
                Some("row=12".to_string())
            )
        );
        assert_eq!(
            split_source("artifact:db.sqlite#table=t&row=7"),
            (
                "artifact:db.sqlite#table=t".to_string(),
                Some("row=7".to_string())
            )
        );
        for whole in [
            "artifact:db.sqlite#table=t",
            "artifact:r.json#/hits",
            "artifact:a.csv",
            "session:3#entry=7",
            "https://x.example/y#z",
        ] {
            assert_eq!(split_source(whole), (whole.to_string(), None), "{whole}");
        }
    }

    #[test]
    fn short_references_in_text_are_written_with_the_case_for_an_export() {
        assert_eq!(
            canonicalize_text(
                "see graph:abc@2, `artifact:a.csv#row=1`; reactor://x/graph/y and plain text",
                "acme"
            ),
            "see reactor://acme/graph/abc@2, `reactor://acme/artifact/a.csv#row=1`; reactor://x/graph/y and plain text"
        );
    }

    #[test]
    fn a_version_can_be_pinned_once() {
        let r = Reference::graph("entity--x").pinned(2);
        assert_eq!(r.short(), "graph:entity--x@2");
        assert_eq!(r.clone().pinned(5).short(), "graph:entity--x@2");
    }
}
