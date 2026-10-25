//! How a property is named, drawn and read
//! ([ADR-0067](../../../../docs/adr/0067-a-property-is-named-in-a-namespace-that-decides-how-it-is-drawn-and-read.md)).
//!
//! A property's name is a dotted path. Its first segment, a **field namespace**, is one of the
//! [`Shape`]s the drawing has a renderer for (`time`, `size`, `ref`, ...); what follows is the
//! writer's. A name takes the shape of its longest known prefix, the way a type takes its
//! look from its namespace in [`super::vocab`]; a name with no known namespace is text.
//!
//! A *known field* says what a name is for, and an *alias* folds a spelling to it when a
//! property is written, so what is stored, hashed and exported is the known name.
//!
//! Like the type vocabulary this is conventions kept as data: no versions and no row in the
//! change log.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

use super::vocab::{check_colour, check_icon, check_name};
use crate::db::Db;
use crate::error::{Error, Result};

/// How a value is drawn and read. The closed set; each has one renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// Prose or a short label.
    #[default]
    Text,
    /// An ISO 8601 instant.
    Time,
    /// Milliseconds.
    Duration,
    /// Bytes.
    Size,
    /// A number, a count included.
    Number,
    /// An opaque identifier: a pid, a SID, a digest.
    Id,
    /// True or false.
    Flag,
    /// A reference of ADR-0066: a place in the case's own evidence.
    Ref,
    /// A URL or a path, opened by its scheme.
    Uri,
    /// A command, source or record, kept exactly as written.
    Verbatim,
    /// A short classification; a chip that clusters nothing.
    Category,
}

impl Shape {
    pub const ALL: [Shape; 11] = [
        Shape::Text,
        Shape::Time,
        Shape::Duration,
        Shape::Size,
        Shape::Number,
        Shape::Id,
        Shape::Flag,
        Shape::Ref,
        Shape::Uri,
        Shape::Verbatim,
        Shape::Category,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Text => "text",
            Shape::Time => "time",
            Shape::Duration => "duration",
            Shape::Size => "size",
            Shape::Number => "number",
            Shape::Id => "id",
            Shape::Flag => "flag",
            Shape::Ref => "ref",
            Shape::Uri => "uri",
            Shape::Verbatim => "verbatim",
            Shape::Category => "category",
        }
    }

    pub fn parse(s: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

/// A dotted prefix that says what its names are drawn and read as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldNamespace {
    pub prefix: String,
    pub shape: Shape,
    pub definition: String,
    pub icon: Option<String>,
    pub colour: Option<String>,
}

/// A name the vocabulary knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownField {
    pub name: String,
    pub definition: String,
    /// The STIX 2.1 property it corresponds to, where there is one.
    pub stix: Option<String>,
    pub icon: Option<String>,
    pub colour: Option<String>,
}

/// The field namespaces, known fields and aliases, read once for a drawing or a write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    pub namespaces: BTreeMap<String, FieldNamespace>,
    pub fields: BTreeMap<String, KnownField>,
    pub aliases: BTreeMap<String, String>,
}

/// `a.b.c` itself, then `a.b`, then `a`.
fn self_and_prefixes(name: &str) -> impl Iterator<Item = &str> {
    std::iter::once(name).chain(name.rmatch_indices('.').map(move |(i, _)| &name[..i]))
}

impl Fields {
    /// The name a spelling folds to: itself when it is not an alias.
    pub fn canonical<'a>(&'a self, name: &'a str) -> &'a str {
        match self.aliases.get(&name.trim().to_lowercase()) {
            Some(known) => known,
            None => name,
        }
    }

    /// The longest known namespace a name is in, or is.
    pub fn namespace_of(&self, name: &str) -> Option<&FieldNamespace> {
        let lower = name.trim().to_lowercase();
        self_and_prefixes(&lower).find_map(|p| self.namespaces.get(p))
    }

    /// How a name is drawn and read: its namespace's shape, else text.
    pub fn shape(&self, name: &str) -> Shape {
        self.namespace_of(name).map(|n| n.shape).unwrap_or_default()
    }

    /// The icon a name is drawn with: its own, else its namespace's.
    pub fn icon(&self, name: &str) -> Option<&str> {
        self.fields
            .get(name.trim().to_lowercase().as_str())
            .and_then(|f| f.icon.as_deref())
            .or_else(|| self.namespace_of(name).and_then(|n| n.icon.as_deref()))
    }

    /// The name as a person reads it: the part after the namespace, humanised, so
    /// `time.first_seen` is *First seen* and `size` is *Size*. A name with no known namespace
    /// is humanised whole.
    pub fn label(&self, name: &str) -> String {
        let lower = name.trim().to_lowercase();
        let rest = match self.namespace_of(&lower) {
            Some(ns) if lower.len() > ns.prefix.len() => &lower[ns.prefix.len() + 1..],
            _ => lower.as_str(),
        };
        humanise(rest)
    }

    /// The names of `props` whose namespace is not known, once aliases are folded: what the
    /// write says is drawn as text.
    pub fn without_namespace<'a>(
        &self,
        props: impl IntoIterator<Item = &'a String>,
    ) -> Vec<String> {
        let mut out: Vec<String> = props
            .into_iter()
            .map(|k| self.canonical(k).to_string())
            .filter(|k| self.namespace_of(k).is_none() && !RESERVED.contains(&k.as_str()))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Names the graph itself reads out of `props`; they are not about drawing, so they are
/// never reported.
const RESERVED: [&str; 5] = ["scope", "hypothesis_status", "host", "pid", "start"];

fn humanise(s: &str) -> String {
    let spaced = s.replace(['_', '.'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Everything the vocabulary says about property names.
pub fn load(db: &Db) -> Result<Fields> {
    db.read(|conn| {
        let mut fields = Fields::default();
        let mut stmt =
            conn.prepare("SELECT prefix, shape, definition, icon, colour FROM field_namespace")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (prefix, shape, definition, icon, colour) = row?;
            if let Some(shape) = Shape::parse(&shape) {
                fields.namespaces.insert(
                    prefix.clone(),
                    FieldNamespace {
                        prefix,
                        shape,
                        definition,
                        icon,
                        colour,
                    },
                );
            }
        }
        let mut stmt =
            conn.prepare("SELECT name, definition, stix, icon, colour FROM known_field")?;
        let rows = stmt.query_map([], |r| {
            Ok(KnownField {
                name: r.get(0)?,
                definition: r.get(1)?,
                stix: r.get(2)?,
                icon: r.get(3)?,
                colour: r.get(4)?,
            })
        })?;
        for f in rows {
            let f = f?;
            fields.fields.insert(f.name.clone(), f);
        }
        let mut stmt = conn.prepare("SELECT alias, name FROM field_alias")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (alias, name) = row?;
            fields.aliases.insert(alias, name);
        }
        Ok(fields)
    })
}

/// Fold the aliases among `props`' names to the names they stand for, in place. A name that
/// is not an alias is left exactly as written. When a property is given under an alias and
/// under its known name, the one under the known name stays.
pub fn fold(conn: &Connection, props: &mut Map<String, Value>) -> Result<()> {
    let mut stmt = conn.prepare_cached("SELECT name FROM field_alias WHERE alias = ?1")?;
    let keys: Vec<String> = props.keys().cloned().collect();
    for key in keys {
        let known: Option<String> = stmt
            .query_row([key.trim().to_lowercase()], |r| r.get(0))
            .optional()?;
        if let Some(known) = known
            && let Some(value) = props.remove(&key)
        {
            props.entry(known).or_insert(value);
        }
    }
    Ok(())
}

/// Fold one name.
pub fn fold_name(conn: &Connection, name: &str) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT name FROM field_alias WHERE alias = ?1",
            [name.trim().to_lowercase()],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(|| name.to_string()))
}

/// A name or prefix that would be confused with a type: a type, a type's alias, or the first
/// segment of a type or of a type namespace.
fn clashes_with_a_type(conn: &Connection, name: &str) -> Result<Option<&'static str>> {
    let exists = |sql: &str, arg: &str| -> Result<bool> {
        Ok(conn.query_row(sql, [arg], |_| Ok(())).optional()?.is_some())
    };
    if exists("SELECT 1 FROM known_type WHERE name = ?1", name)? {
        return Ok(Some("a type"));
    }
    if exists("SELECT 1 FROM type_alias WHERE alias = ?1", name)? {
        return Ok(Some("the alias of a type"));
    }
    let root = name.split('.').next().unwrap_or(name);
    let like = format!("{root}.%");
    if exists("SELECT 1 FROM known_type WHERE name LIKE ?1 LIMIT 1", &like)?
        || exists("SELECT 1 FROM type_namespace WHERE prefix = ?1", root)?
    {
        return Ok(Some("the namespace of types"));
    }
    Ok(None)
}

/// A name to make known, or to say more about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Define {
    pub name: String,
    pub definition: Option<String>,
    /// The STIX 2.1 property it corresponds to.
    pub stix: Option<String>,
    pub icon: Option<String>,
    pub colour: Option<String>,
    /// Spellings that fold to it.
    pub aliases: Vec<String>,
}

/// Make a property name known, or change what is said of one that is: only what is given
/// changes. The name must be in a known namespace (the shape is the namespace's), and must not
/// be confused with a type.
pub fn define_field(db: &Db, d: Define) -> Result<KnownField> {
    let name = check_name("property name", &d.name)?;
    let (icon, colour) = (
        check_icon(d.icon.as_deref())?,
        check_colour(d.colour.as_deref())?,
    );
    let aliases = d
        .aliases
        .iter()
        .map(|a| check_name("alias", a))
        .collect::<Result<Vec<_>>>()?;
    let loaded = load(db)?;
    if loaded.namespace_of(&name).is_none() {
        return Err(Error::Invalid(format!(
            "`{name}` is not in a known namespace ({}): a property is named `<namespace>.<name>`",
            loaded
                .namespaces
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    db.write(move |tx| {
        if let Some(what) = clashes_with_a_type(tx, &name)? {
            return Err(Error::Invalid(format!(
                "`{name}` is {what}: a thing with an identity is an element, linked by a role, not a property"
            )));
        }
        let alias_of: Option<String> = tx
            .query_row("SELECT name FROM field_alias WHERE alias = ?1", [&name], |r| r.get(0))
            .optional()?;
        if let Some(of) = alias_of {
            return Err(Error::Invalid(format!(
                "`{name}` is an alias of `{of}`; a name cannot be both"
            )));
        }
        tx.execute(
            "INSERT INTO known_field(name, definition, stix, icon, colour)
             VALUES (?1, coalesce(?2, ''), ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET definition = coalesce(?2, definition),
                    stix = coalesce(?3, stix), icon = coalesce(?4, icon),
                    colour = coalesce(?5, colour)",
            params![name, d.definition, d.stix, icon, colour],
        )?;
        for alias in &aliases {
            let known: bool = tx
                .query_row("SELECT 1 FROM known_field WHERE name = ?1", [alias], |_| Ok(()))
                .optional()?
                .is_some();
            let is_namespace: bool = tx
                .query_row(
                    "SELECT 1 FROM field_namespace WHERE prefix = ?1",
                    [alias],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if known || is_namespace || *alias == name {
                return Err(Error::Invalid(format!(
                    "`{alias}` is a known name; an alias is a spelling that is not"
                )));
            }
            if let Some(what) = clashes_with_a_type(tx, alias)? {
                return Err(Error::Invalid(format!("`{alias}` is {what}, not a spelling of a property")));
            }
            tx.execute(
                "INSERT INTO field_alias(alias, name) VALUES (?1, ?2)
                 ON CONFLICT(alias) DO UPDATE SET name = ?2",
                params![alias, name],
            )?;
        }
        Ok(tx.query_row(
            "SELECT name, definition, stix, icon, colour FROM known_field WHERE name = ?1",
            [&name],
            |r| {
                Ok(KnownField {
                    name: r.get(0)?,
                    definition: r.get(1)?,
                    stix: r.get(2)?,
                    icon: r.get(3)?,
                    colour: r.get(4)?,
                })
            },
        )?)
    })
}

/// Add a prefix that points at one of the shapes, or say more about one that does. The set of
/// shapes is the drawing's; a prefix only chooses among them.
pub fn define_namespace(
    db: &Db,
    prefix: &str,
    shape: Option<Shape>,
    definition: Option<&str>,
    icon: Option<&str>,
    colour: Option<&str>,
) -> Result<()> {
    let prefix = check_name("namespace", prefix)?;
    let (icon, colour) = (check_icon(icon)?, check_colour(colour)?);
    let definition = definition.map(str::to_string);
    db.write(move |tx| {
        let known: bool = tx
            .query_row(
                "SELECT 1 FROM field_namespace WHERE prefix = ?1",
                [&prefix],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !known {
            let Some(shape) = shape else {
                return Err(Error::Invalid(format!(
                    "`{prefix}` is not a known namespace yet: say its shape ({})",
                    Shape::ALL.map(Shape::as_str).join(", ")
                )));
            };
            if let Some(what) = clashes_with_a_type(tx, &prefix)? {
                return Err(Error::Invalid(format!(
                    "`{prefix}` is {what}: a property namespace is a shape, not a kind of thing"
                )));
            }
            tx.execute(
                "INSERT INTO field_namespace(prefix, shape, definition, icon, colour)
                 VALUES (?1, ?2, coalesce(?3, ''), ?4, ?5)",
                params![prefix, shape.as_str(), definition, icon, colour],
            )?;
        } else {
            tx.execute(
                "UPDATE field_namespace SET definition = coalesce(?2, definition),
                        icon = coalesce(?3, icon), colour = coalesce(?4, colour)
                  WHERE prefix = ?1",
                params![prefix, definition, icon, colour],
            )?;
        }
        Ok(())
    })
}

/// The namespaces and known names as text, one namespace after another with its names under it
/// -- for the agent, which is told the shapes there are before it names a property. `under`
/// narrows it to one namespace or name prefix (`time`, `time.first`).
pub fn describe(db: &Db, under: Option<&str>) -> Result<String> {
    let fields = load(db)?;
    let under = under.map(|u| u.trim().trim_end_matches('.').to_lowercase());
    let within = |name: &str| match &under {
        Some(u) => name.starts_with(u.as_str()),
        None => true,
    };
    let mut out = String::new();
    for ns in fields.namespaces.values().filter(|n| {
        within(&n.prefix)
            || fields
                .fields
                .keys()
                .any(|k| k.starts_with(&format!("{}.", n.prefix)) && within(k))
    }) {
        out.push_str(&format!(
            "{} ({}) -- {}\n",
            ns.prefix,
            ns.shape.as_str(),
            ns.definition
        ));
        for f in fields.fields.values().filter(|f| {
            (f.name == ns.prefix || f.name.starts_with(&format!("{}.", ns.prefix)))
                && within(&f.name)
        }) {
            let aliases: Vec<&str> = fields
                .aliases
                .iter()
                .filter(|(_, n)| **n == f.name)
                .map(|(a, _)| a.as_str())
                .collect();
            let also = if aliases.is_empty() {
                String::new()
            } else {
                format!(" (also {})", aliases.join(", "))
            };
            out.push_str(&format!("  {} {}{also}\n", f.name, f.definition));
        }
    }
    if out.is_empty() {
        out.push_str("no known property names there");
    }
    Ok(out.trim_end().to_string())
}

// -- writing a value a person typed ----------------------------------------------------------

/// The value a person's text stands for, as its shape stores it: `180KiB` under `size` is
/// the number of bytes, `2026-03-12 09:14` under `time` is an RFC 3339 instant, `yes` under
/// `flag` is a boolean, a number under `number` is a number. Text that does not suit the
/// shape is kept as typed, and is drawn as not suiting it; nothing is refused for that.
///
/// One value, or several: lines are the items of an array, except under `verbatim` and `text`,
/// where the whole text is one value unless `list` says the property already holds an array.
pub fn value_from_text(shape: Shape, text: &str, list: bool) -> Result<Value> {
    let whole = text.trim_matches(['\r', '\n']);
    if whole.trim().is_empty() {
        return Err(Error::Invalid(
            "a property needs a value; to take it away, remove it".into(),
        ));
    }
    let lines = || -> Vec<&str> {
        whole
            .split('\n')
            .map(|l| l.trim_end_matches('\r').trim())
            .filter(|l| !l.is_empty())
            .collect()
    };
    let free = matches!(shape, Shape::Text | Shape::Verbatim);
    if free && !list {
        return Ok(Value::String(whole.to_string()));
    }
    let items: Vec<Value> = lines()
        .into_iter()
        .map(|l| item_from_text(shape, l))
        .collect();
    Ok(match items.len() {
        1 if !list => items.into_iter().next().unwrap_or(Value::Null),
        _ => Value::Array(items),
    })
}

fn item_from_text(shape: Shape, text: &str) -> Value {
    let kept = || Value::String(text.to_string());
    match shape {
        Shape::Size => parse_size(text).map(Value::from).unwrap_or_else(kept),
        Shape::Duration => parse_duration(text).map(number).unwrap_or_else(kept),
        Shape::Number => text
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| text.parse::<f64>().map(number))
            .unwrap_or_else(|_| kept()),
        Shape::Time => crate::time::parse_source_time(text)
            .map(|p| Value::String(p.utc))
            .unwrap_or_else(kept),
        Shape::Flag => match text.to_lowercase().as_str() {
            "true" | "yes" => Value::Bool(true),
            "false" | "no" => Value::Bool(false),
            _ => kept(),
        },
        _ => kept(),
    }
}

/// A float that is whole is stored as an integer, so `4200` does not become `4200.0`.
fn number(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

// -- finding by property --------------------------------------------------------------------

/// How a property is compared in a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Has,
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A search on a property: `size>1MiB`, `time.first_seen<2026-03-01`, `verbatim.command_line=ls`,
/// or a name (or namespace, `time.`) alone for "has it".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropFilter {
    pub name: String,
    pub op: Op,
    pub value: String,
}

impl PropFilter {
    pub fn parse(text: &str) -> Result<PropFilter> {
        let text = text.trim();
        let at = text.find(['<', '>', '=']);
        let Some(at) = at else {
            return Ok(PropFilter {
                name: text.trim_end_matches('.').to_lowercase(),
                op: Op::Has,
                value: String::new(),
            });
        };
        let name = text[..at].trim().trim_end_matches('.').to_lowercase();
        let rest = &text[at..];
        let (op, len) = if rest.starts_with("<=") {
            (Op::Le, 2)
        } else if rest.starts_with(">=") {
            (Op::Ge, 2)
        } else if rest.starts_with('<') {
            (Op::Lt, 1)
        } else if rest.starts_with('>') {
            (Op::Gt, 1)
        } else {
            (Op::Eq, 1)
        };
        if name.is_empty() {
            return Err(Error::Invalid(format!(
                "`{text}`: a property filter is `name`, `name=value`, `name<value` or `name>value`"
            )));
        }
        Ok(PropFilter {
            name,
            op,
            value: rest[len..].trim().to_string(),
        })
    }

    /// Whether `props` has a property this filter takes. A name matches itself and every name
    /// under it, so `time.` takes every time property.
    pub fn matches(&self, fields: &Fields, props: &Map<String, Value>) -> bool {
        let wanted = fields.canonical(&self.name).to_string();
        props.iter().any(|(key, value)| {
            let key = fields.canonical(key);
            let under = key == wanted || key.starts_with(&format!("{wanted}."));
            under && (self.op == Op::Has || compare(fields.shape(key), value, self.op, &self.value))
        })
    }
}

fn compare(shape: Shape, value: &Value, op: Op, rhs: &str) -> bool {
    // An array holds when any of its items does.
    if let Value::Array(items) = value {
        return items.iter().any(|v| compare(shape, v, op, rhs));
    }
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let order = match shape {
        Shape::Size => parse_size(&text)
            .zip(parse_size(rhs))
            .map(|(a, b)| a.cmp(&b)),
        Shape::Duration => parse_duration(&text)
            .zip(parse_duration(rhs))
            .map(|(a, b)| a.total_cmp(&b)),
        Shape::Number => text
            .trim()
            .parse::<f64>()
            .ok()
            .zip(rhs.trim().parse::<f64>().ok())
            .map(|(a, b)| a.total_cmp(&b)),
        Shape::Time => crate::time::parse_source_time(&text)
            .zip(crate::time::parse_source_time(rhs))
            .map(|(a, b)| a.utc.cmp(&b.utc)),
        Shape::Flag => {
            let flag = |s: &str| match s.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" => Some(true),
                "false" | "no" | "0" => Some(false),
                _ => None,
            };
            flag(&text).zip(flag(rhs)).map(|(a, b)| a.cmp(&b))
        }
        Shape::Id | Shape::Category => Some(text.to_lowercase().cmp(&rhs.trim().to_lowercase())),
        Shape::Text | Shape::Ref | Shape::Uri | Shape::Verbatim => {
            Some(text.to_lowercase().cmp(&rhs.trim().to_lowercase()))
        }
    };
    use std::cmp::Ordering::*;
    match (order, op) {
        (None, _) => false,
        (Some(_), Op::Has) => true,
        (Some(o), Op::Eq) => o == Equal,
        (Some(o), Op::Lt) => o == Less,
        (Some(o), Op::Le) => o != Greater,
        (Some(o), Op::Gt) => o == Greater,
        (Some(o), Op::Ge) => o != Less,
    }
}

/// `184320`, `180KiB`, `1.5 MB`: bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let unit = unit.trim().to_lowercase();
    let k = match unit.as_str() {
        "" | "b" | "bytes" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" | "k" => 1024.0,
        "mib" | "m" => 1024.0 * 1024.0,
        "gib" | "g" => 1024.0 * 1024.0 * 1024.0,
        "tib" | "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * k).round() as u64)
}

/// `1500`, `1.5s`, `2m`: milliseconds.
pub fn parse_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let k = match unit.trim().to_lowercase().as_str() {
        "" | "ms" => 1.0,
        "s" | "sec" => 1e3,
        "m" | "min" => 6e4,
        "h" => 3.6e6,
        "d" => 8.64e7,
        _ => return None,
    };
    Some(n * k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes_and_durations_read_their_units() {
        assert_eq!(parse_size("180KiB"), Some(184_320));
        assert_eq!(parse_size("1.5 MB"), Some(1_500_000));
        assert_eq!(parse_size("184320"), Some(184_320));
        assert_eq!(parse_size("big"), None);
        assert_eq!(parse_duration("1.5s"), Some(1500.0));
        assert_eq!(parse_duration("2m"), Some(120_000.0));
    }

    #[test]
    fn a_filter_is_a_name_an_operator_and_a_value() {
        let f = PropFilter::parse("size>=1MiB").unwrap();
        assert_eq!(
            (f.name.as_str(), f.op, f.value.as_str()),
            ("size", Op::Ge, "1MiB")
        );
        let f = PropFilter::parse("time.").unwrap();
        assert_eq!((f.name.as_str(), f.op), ("time", Op::Has));
        assert!(PropFilter::parse(">3").is_err());
    }

    #[test]
    fn a_name_matches_itself_and_what_is_under_it() {
        let mut fields = Fields::default();
        for (p, s) in [("time", Shape::Time), ("size", Shape::Size)] {
            fields.namespaces.insert(
                p.into(),
                FieldNamespace {
                    prefix: p.into(),
                    shape: s,
                    definition: String::new(),
                    icon: None,
                    colour: None,
                },
            );
        }
        fields.aliases.insert("size_bytes".into(), "size".into());
        let props = json!({"time.first_seen": "2026-03-12T09:14:00Z", "size_bytes": 184320})
            .as_object()
            .cloned()
            .unwrap();
        let yes = |f: &str| PropFilter::parse(f).unwrap().matches(&fields, &props);
        assert!(yes("time."));
        assert!(yes("time.first_seen<2026-03-13"));
        assert!(!yes("time.first_seen<2026-03-01"));
        // An alias in the filter and in the stored name are the same name.
        assert!(yes("size>100KiB") && !yes("size>1MiB"));
        assert!(!yes("duration"));
    }
}
