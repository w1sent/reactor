//! How the vocabulary's types are grouped and drawn.
//!
//! A type is dot-namespaced (`net.ipv4`, `file.hash.sha256`). A **namespace** is a dotted
//! prefix that says what its types have in common (`net`, `file.hash`) and how they are drawn
//! when they say nothing themselves. A type may name an icon and a colour of its own -- the
//! seeded ones do, and a person or the agent can give one to any type, their own included.
//!
//! What a type looks like is found from the most specific thing that says so: the type, then
//! the longest namespace it is in, then the next. What is left unsaid is the drawing's to
//! decide (an icon for the kind, a neutral colour).
//!
//! This is presentation, like [`super::layout`]: no versions and no row in the change log.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use super::Kind;
use crate::db::Db;
use crate::error::{Error, Result};

/// The colours a type or namespace can be given, by name. What each looks like is the
/// drawing's business. Orange and red are not among them: they mean *disputed* and
/// *rejected* wherever the graph is drawn.
pub const COLOURS: [&str; 20] = [
    "grey", "slate", "cream", "sand", "brown", "yellow", "olive", "lime", "green", "mint", "teal",
    "cyan", "sky", "blue", "cobalt", "indigo", "violet", "purple", "magenta", "pink",
];

/// A dotted prefix and what its types have in common.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    pub prefix: String,
    pub definition: String,
    pub icon: Option<String>,
    pub colour: Option<String>,
}

/// A type the vocabulary knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownType {
    pub name: String,
    pub kind: Option<Kind>,
    pub definition: String,
    pub icon: Option<String>,
    pub colour: Option<String>,
}

/// How a type is drawn, as far as the vocabulary says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Look {
    pub icon: Option<String>,
    pub colour: Option<String>,
    /// The type itself says so (either of the two), not only a namespace it is in.
    pub own: bool,
}

/// The types and namespaces, read once for a drawing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    pub types: BTreeMap<String, KnownType>,
    pub namespaces: BTreeMap<String, Namespace>,
    /// The colour a tag was given, by name (a prefix such as `phase` speaks for `phase.c2`).
    pub tags: BTreeMap<String, String>,
}

/// The prefixes of a type, longest first: `a.b.c` gives `a.b`, then `a`.
fn prefixes(type_: &str) -> impl Iterator<Item = &str> {
    type_.rmatch_indices('.').map(move |(i, _)| &type_[..i])
}

impl Vocabulary {
    /// The namespaces a type is in, the most specific first.
    pub fn namespaces_of<'a>(&'a self, type_: &'a str) -> impl Iterator<Item = &'a Namespace> {
        prefixes(type_).filter_map(|p| self.namespaces.get(p))
    }

    /// The outermost namespace of a type as written, known or not: `net` for `net.tls.ja3`.
    pub fn root(type_: &str) -> &str {
        type_.split('.').next().unwrap_or(type_)
    }

    /// The colour a tag was given: its own, else that of the longest dotted prefix that has
    /// one. `None` leaves it to the drawing.
    pub fn tag_colour(&self, tag: &str) -> Option<&str> {
        let tag = tag.trim().to_lowercase();
        self.tags
            .get(&tag)
            .or_else(|| prefixes(&tag).find_map(|p| self.tags.get(p)))
            .map(String::as_str)
    }

    pub fn look(&self, type_: &str) -> Look {
        let own = self.types.get(type_);
        let mut look = Look {
            icon: own.and_then(|t| t.icon.clone()),
            colour: own.and_then(|t| t.colour.clone()),
            own: false,
        };
        look.own = look.icon.is_some() || look.colour.is_some();
        for ns in self.namespaces_of(type_) {
            if look.icon.is_none() {
                look.icon = ns.icon.clone();
            }
            if look.colour.is_none() {
                look.colour = ns.colour.clone();
            }
        }
        look
    }
}

/// Everything the vocabulary says about types and namespaces.
pub fn load(db: &Db) -> Result<Vocabulary> {
    db.read(|conn| {
        let mut vocabulary = Vocabulary::default();
        let mut stmt = conn
            .prepare("SELECT name, kind, definition, icon, colour FROM known_type ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(KnownType {
                name: r.get(0)?,
                kind: Kind::parse(&r.get::<_, String>(1)?),
                definition: r.get(2)?,
                icon: r.get(3)?,
                colour: r.get(4)?,
            })
        })?;
        for t in rows {
            let t = t?;
            vocabulary.types.insert(t.name.clone(), t);
        }
        let mut stmt = conn.prepare(
            "SELECT prefix, definition, icon, colour FROM type_namespace ORDER BY prefix",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Namespace {
                prefix: r.get(0)?,
                definition: r.get(1)?,
                icon: r.get(2)?,
                colour: r.get(3)?,
            })
        })?;
        for ns in rows {
            let ns = ns?;
            vocabulary.namespaces.insert(ns.prefix.clone(), ns);
        }
        let mut stmt = conn.prepare("SELECT tag, colour FROM tag_look ORDER BY tag")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (tag, colour) = row?;
            vocabulary.tags.insert(tag, colour);
        }
        Ok(vocabulary)
    })
}

fn check_name(what: &str, name: &str) -> Result<String> {
    let name = name.trim().to_lowercase();
    let part_ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    if name.is_empty() || !name.split('.').all(part_ok) {
        return Err(Error::Invalid(format!(
            "`{name}` is not a {what}: lowercase words joined by dots, as in `net.ipv4`"
        )));
    }
    Ok(name)
}

fn check_colour(colour: Option<&str>) -> Result<Option<String>> {
    match colour.map(|c| c.trim().to_lowercase()) {
        None => Ok(None),
        Some(c) if COLOURS.contains(&c.as_str()) => Ok(Some(c)),
        Some(c) => Err(Error::Invalid(format!(
            "`{c}` is not a colour a type can have ({}); orange and red mean disputed and rejected",
            COLOURS.join(", ")
        ))),
    }
}

/// An icon's name as stored: a Lucide icon's, lowercase with dashes. Whether there is such
/// an icon is for whoever draws it to know; one that there is not is drawn as if unset.
fn check_icon(icon: Option<&str>) -> Result<Option<String>> {
    match icon.map(|i| i.trim().to_lowercase().replace('_', "-")) {
        None => Ok(None),
        Some(i)
            if !i.is_empty()
                && i.len() <= 48
                && i.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') =>
        {
            Ok(Some(i))
        }
        Some(i) => Err(Error::Invalid(format!(
            "`{i}` is not an icon's name: a Lucide icon, as in `shield-alert`"
        ))),
    }
}

/// Give a tag a colour, or (with `None`) take it back to the one its name gives it. A tag is
/// free text, so it need not be known first; names are compared in lower case.
pub fn set_tag_colour(db: &Db, tag: &str, colour: Option<&str>) -> Result<()> {
    let tag = tag.trim().to_lowercase();
    if tag.is_empty() {
        return Err(Error::Invalid("a tag needs a name".into()));
    }
    let colour = check_colour(colour)?;
    db.write(move |tx| {
        match colour {
            Some(colour) => tx.execute(
                "INSERT INTO tag_look(tag, colour) VALUES (?1, ?2)
                 ON CONFLICT(tag) DO UPDATE SET colour = ?2",
                params![tag, colour],
            )?,
            None => tx.execute("DELETE FROM tag_look WHERE tag = ?1", [&tag])?,
        };
        Ok(())
    })
}

/// Say how a type is drawn: its icon, its colour, or (with `None`) neither any more. A type
/// the vocabulary does not know yet becomes known, as a type of `kind`.
pub fn set_look(
    db: &Db,
    type_: &str,
    kind: Kind,
    icon: Option<&str>,
    colour: Option<&str>,
) -> Result<()> {
    let name = check_name("type", type_)?;
    let (icon, colour) = (check_icon(icon)?, check_colour(colour)?);
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO known_type(name, kind, icon, colour) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET icon = ?3, colour = ?4",
            params![name, kind.as_str(), icon, colour],
        )?;
        Ok(())
    })
}

/// A type to make known, or to say more about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Define {
    pub name: String,
    /// Needed for a type that is not known yet.
    pub kind: Option<Kind>,
    pub definition: Option<String>,
    pub icon: Option<String>,
    pub colour: Option<String>,
}

/// Make a type known, or change what is said of one that is: only what is given changes.
/// Returns the type as it now stands.
pub fn define_type(db: &Db, d: Define) -> Result<KnownType> {
    let name = check_name("type", &d.name)?;
    let (icon, colour) = (
        check_icon(d.icon.as_deref())?,
        check_colour(d.colour.as_deref())?,
    );
    if matches!(d.kind, Some(Kind::External)) {
        return Err(Error::Invalid("an external has no type to define".into()));
    }
    db.write(move |tx| {
        let known: Option<String> = tx
            .query_row(
                "SELECT kind FROM known_type WHERE name = ?1",
                [&name],
                |r| r.get(0),
            )
            .optional()?;
        match (known, d.kind) {
            (None, None) => {
                return Err(Error::Invalid(format!(
                    "`{name}` is not a known type yet: say its kind (entity, event, observation)"
                )));
            }
            (None, Some(kind)) => {
                tx.execute(
                    "INSERT INTO known_type(name, kind, definition, normalizer, icon, colour)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        name,
                        kind.as_str(),
                        d.definition.clone().unwrap_or_default(),
                        (kind == Kind::Entity).then_some("trim"),
                        icon,
                        colour
                    ],
                )?;
            }
            (Some(was), kind) => {
                if let Some(kind) = kind
                    && kind.as_str() != was
                {
                    return Err(Error::Invalid(format!(
                        "`{name}` is a known {was} type; it cannot become a type of {}",
                        kind.as_str()
                    )));
                }
                tx.execute(
                    "UPDATE known_type SET definition = coalesce(?2, definition),
                            icon = coalesce(?3, icon), colour = coalesce(?4, colour)
                      WHERE name = ?1",
                    params![name, d.definition, icon, colour],
                )?;
            }
        }
        Ok(tx.query_row(
            "SELECT name, kind, definition, icon, colour FROM known_type WHERE name = ?1",
            [&name],
            |r| {
                Ok(KnownType {
                    name: r.get(0)?,
                    kind: Kind::parse(&r.get::<_, String>(1)?),
                    definition: r.get(2)?,
                    icon: r.get(3)?,
                    colour: r.get(4)?,
                })
            },
        )?)
    })
}

/// Say what a namespace is and how its types are drawn. Only what is given changes on one
/// that exists.
pub fn define_namespace(
    db: &Db,
    prefix: &str,
    definition: Option<&str>,
    icon: Option<&str>,
    colour: Option<&str>,
) -> Result<()> {
    let prefix = check_name("namespace", prefix)?;
    let (icon, colour) = (check_icon(icon)?, check_colour(colour)?);
    let definition = definition.map(str::to_string);
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO type_namespace(prefix, definition, icon, colour)
             VALUES (?1, coalesce(?2, ''), ?3, ?4)
             ON CONFLICT(prefix) DO UPDATE SET definition = coalesce(?2, definition),
                    icon = coalesce(?3, icon), colour = coalesce(?4, colour)",
            params![prefix, definition, icon, colour],
        )?;
        Ok(())
    })
}

/// The vocabulary as text, one namespace after another with its types under it -- for the
/// agent, which is told the types there are before it makes up one of its own. `under`
/// narrows it to one namespace (`net`, `file.hash`).
pub fn describe(db: &Db, under: Option<&str>) -> Result<String> {
    let vocabulary = load(db)?;
    let under = under.map(|u| u.trim().trim_end_matches('.').to_lowercase());
    let within = |name: &str| match &under {
        Some(u) => name == u || name.starts_with(&format!("{u}.")),
        None => true,
    };
    let mut roots: BTreeMap<&str, Vec<&KnownType>> = BTreeMap::new();
    for t in vocabulary.types.values().filter(|t| within(&t.name)) {
        roots.entry(Vocabulary::root(&t.name)).or_default().push(t);
    }
    let mut out = String::new();
    for (root, types) in roots {
        let about = vocabulary
            .namespaces
            .get(root)
            .map(|ns| ns.definition.as_str())
            .unwrap_or("");
        out.push_str(&format!("{root} -- {about}\n"));
        for t in types {
            let kind = t.kind.map(Kind::as_str).unwrap_or("?");
            out.push_str(&format!("  {} ({kind}) {}\n", t.name, t.definition));
        }
    }
    if out.is_empty() {
        out.push_str("no known types there");
    }
    Ok(out.trim_end().to_string())
}
