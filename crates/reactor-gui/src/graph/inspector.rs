//! What the graph panel's inspector shows for a selected element, as plain data.
//!
//! One structure serves every kind of element -- an entity, an observation, an event, a note, a
//! hypothesis -- so the inspector reads the same whatever is selected: what it is, what it says,
//! how sure, what it rests on, what happened to it. The panel draws it; the popup that reads
//! and edits a long text draws the same data. Built from the database (`reactor-db`), tested
//! without a window.

use reactor_db::Db;
use reactor_db::graph::{Kind, query};

/// Text longer than this, or with more than one line, is *text* (read in a reading view and
/// edited in a popup); shorter is just the title.
pub const LONG_TEXT: usize = 140;

/// How the kind of element is coloured: the panel maps these to theme colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Entity,
    Event,
    Observation,
    Hypothesis,
    Judgement,
    Note,
    Other,
}

/// What other elements say about this one, grouped as a person asks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stand {
    Supports,
    Refutes,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub id: String,
    /// A short name for the element at the other end.
    pub name: String,
    /// Where in it the relation points (`strace.log:412`), when it says.
    pub locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// `Supports`, `Refutes`, `About`, `Mentioned by`, or a role's own name.
    pub title: String,
    pub stand: Stand,
    pub items: Vec<Related>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub ver: u32,
    pub when: String,
    pub who: String,
    /// What this version changed, in words.
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    pub id: String,
    pub ver: u32,
    pub tone: Tone,
    /// `Hypothesis`, `Note`, `Entity · file`, `Observation`.
    pub label: String,
    pub type_: String,
    pub title: String,
    /// The long text, when there is one: Markdown, as written.
    pub text: Option<String>,
    /// Whether a person may rewrite the text (an entity's value is its identity).
    pub editable: bool,
    pub status: String,
    /// `open`, `supported`, `rejected`, for a hypothesis.
    pub hypothesis: Option<String>,
    pub confidence: Option<String>,
    /// The confidence's place on the scale, weakest first: `(index, steps)`.
    pub level: Option<(usize, usize)>,
    pub tags: Vec<String>,
    /// `agent · version 3 · 2026-10-05 12:04`.
    pub meta: String,
    /// Labelled facts besides the text: when it happened, what holds, its properties.
    pub facts: Vec<(String, String)>,
    pub groups: Vec<Group>,
    pub history: Vec<Version>,
    pub recorded_at: String,
    pub author: String,
}

impl Inspected {
    /// How many elements it is related to, for a count on a tab.
    pub fn related(&self) -> usize {
        self.groups.iter().map(|g| g.items.len()).sum()
    }
}

/// The first line, cut at a sentence or a length.
fn title_of(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let first = first.trim_start_matches('#').trim();
    if first.chars().count() <= 100 {
        return first.to_string();
    }
    let cut: String = first.chars().take(100).collect();
    match cut.rfind(". ") {
        Some(at) if at > 40 => cut[..=at].to_string(),
        _ => format!("{}…", cut.trim_end()),
    }
}

/// `Hypothesis`, `Note`, `Entity · file`: what the element is, for a person.
fn label_of(kind: Kind, type_: &str) -> (Tone, String) {
    let short = type_.rsplit('.').next().unwrap_or(type_);
    let title_case = |s: &str| {
        let mut c = s.replace('_', " ").chars().collect::<Vec<_>>();
        if let Some(f) = c.first_mut() {
            *f = f.to_ascii_uppercase();
        }
        c.into_iter().collect::<String>()
    };
    match (kind, type_) {
        (Kind::Annotation, "analysis.hypothesis") => (Tone::Hypothesis, "Hypothesis".into()),
        (Kind::Annotation, "analysis.note") => (Tone::Note, "Note".into()),
        (Kind::Annotation, "analysis.judgement") => (Tone::Judgement, "Judgement".into()),
        (Kind::Annotation, _) => (Tone::Other, title_case(short)),
        (Kind::Entity, _) => (Tone::Entity, format!("Entity · {short}")),
        (Kind::Event, _) => (Tone::Event, format!("Event · {short}")),
        (Kind::Observation, _) => (Tone::Observation, format!("Observation · {short}")),
        (Kind::External, _) => (Tone::Other, format!("External · {short}")),
    }
}

/// `agent:claude-sonnet@5` → `agent · claude-sonnet`; a person is `you`.
pub fn who(author: &str) -> String {
    let (kind, rest) = author.split_once(':').unwrap_or((author, ""));
    let name = rest.split('@').next().unwrap_or("");
    match kind {
        "analyst" => "analyst".to_string(),
        "agent" if !name.is_empty() => format!("agent · {name}"),
        _ if name.is_empty() => kind.to_string(),
        _ => format!("{kind} · {name}"),
    }
}

/// What a version changed compared with the one before it.
fn change_of(prev: Option<&query::Element>, cur: &query::Element) -> String {
    let Some(p) = prev else {
        return "created".into();
    };
    let (a, b) = (&p.content, &cur.content);
    let mut parts: Vec<String> = Vec::new();
    if a.value != b.value {
        parts.push("text changed".into());
    }
    if a.status != b.status {
        parts.push(format!("status → {}", b.status));
    }
    if a.confidence != b.confidence {
        parts.push(match &b.confidence {
            Some(c) => format!("confidence → {c}"),
            None => "confidence cleared".into(),
        });
    }
    if a.tags != b.tags {
        parts.push("tags changed".into());
    }
    let hypothesis = |c: &reactor_db::graph::Content| {
        c.props
            .get("hypothesis_status")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    if hypothesis(a) != hypothesis(b)
        && let Some(h) = hypothesis(b)
    {
        parts.push(format!("hypothesis → {h}"));
    }
    if parts.is_empty() && a.props != b.props {
        parts.push("properties changed".into());
    }
    if parts.is_empty() && a.time != b.time {
        parts.push("time changed".into());
    }
    if parts.is_empty() {
        "changed".into()
    } else {
        parts.join(", ")
    }
}

fn related(db: &Db, id: &str, locator: Option<String>) -> Related {
    Related {
        id: id.to_string(),
        name: reactor_agent::tools::graph::name(db, id),
        locator,
    }
}

fn groups_of(db: &Db, id: &str) -> Vec<Group> {
    let mut supports = Vec::new();
    let mut refutes = Vec::new();
    let mut about = Vec::new();
    let mut mentioned = Vec::new();
    let mut other: Vec<(String, Vec<Related>)> = Vec::new();
    let mut put_other =
        |title: String, r: Related| match other.iter_mut().find(|(t, _)| *t == title) {
            Some((_, items)) => items.push(r),
            None => other.push((title, vec![r])),
        };
    for g in query::edges_from(db, id).unwrap_or_default() {
        let r = related(db, &g.to_id, g.locator.clone());
        match g.role.as_str() {
            "supports" => supports.push(r),
            "refutes" => refutes.push(r),
            "about" => about.push(r),
            role => put_other(format!("{role} →"), r),
        }
    }
    for g in query::edges_to(db, id).unwrap_or_default() {
        let r = related(db, &g.from_id, g.locator.clone());
        match g.role.as_str() {
            "supports" => supports.push(r),
            "refutes" => refutes.push(r),
            "about" => mentioned.push(r),
            role => put_other(format!("← {role}"), r),
        }
    }
    let mut groups = Vec::new();
    let mut push = |title: &str, stand: Stand, items: Vec<Related>| {
        if !items.is_empty() {
            groups.push(Group {
                title: title.to_string(),
                stand,
                items,
            });
        }
    };
    push("Supports", Stand::Supports, supports);
    push("Refutes", Stand::Refutes, refutes);
    push("About", Stand::Neutral, about);
    push("Mentioned by", Stand::Neutral, mentioned);
    for (title, items) in other {
        push(&title, Stand::Neutral, items);
    }
    groups
}

/// The facts of an element besides its text: when, what holds, and its properties. What the
/// header or the groups already say is left out.
fn facts_of(e: &query::Element) -> Vec<(String, String)> {
    let mut facts: Vec<(String, String)> = Vec::new();
    let mut add = |label: &str, value: String| facts.push((label.to_string(), value));
    let t = &e.content.time;
    if let Some(at) = &t.occurred_at {
        add("Happened", at.clone());
    }
    if t.valid_from.is_some() || t.valid_to.is_some() {
        add(
            "Held",
            format!(
                "{} → {}",
                t.valid_from.as_deref().unwrap_or("?"),
                t.valid_to.as_deref().unwrap_or("open")
            ),
        );
    }
    if let Some(raw) = &t.raw {
        add(
            "As written",
            format!(
                "{raw} (zone {})",
                t.tz_certainty.as_deref().unwrap_or("unknown")
            ),
        );
    }
    for (key, value) in &e.content.props {
        if key == "hypothesis_status" {
            continue;
        }
        add(
            key,
            match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            },
        );
    }
    facts
}

/// Everything the inspector shows for `e`. `scale` is the confidence scale, weakest first.
pub fn inspect(db: &Db, e: &query::Element, scale: &[String]) -> Inspected {
    let (tone, label) = label_of(e.kind, &e.type_);
    let value = e.content.value.clone().unwrap_or_default();
    let long = value.trim().chars().count() > LONG_TEXT || value.trim().contains('\n');
    // An entity's value is its name, however long; anything else long is text to read.
    let (title, text) = if e.kind == Kind::Entity || !long {
        (value.trim().to_string(), None)
    } else {
        (title_of(&value), Some(value.trim().to_string()))
    };
    let title = if title.is_empty() {
        e.type_.clone()
    } else {
        title
    };
    let level = e
        .content
        .confidence
        .as_deref()
        .and_then(|c| scale.iter().position(|s| s == c).map(|i| (i, scale.len())));
    let history = {
        let versions = query::history(db, &e.id).unwrap_or_default();
        versions
            .iter()
            .enumerate()
            .map(|(i, v)| Version {
                ver: v.ver,
                when: v.recorded_at.clone(),
                who: who(&v.author),
                change: change_of(i.checked_sub(1).map(|p| &versions[p]), v),
            })
            .collect()
    };
    Inspected {
        id: e.id.clone(),
        ver: e.ver,
        tone,
        label,
        type_: e.type_.clone(),
        title,
        text,
        editable: e.kind != Kind::Entity,
        status: e.content.status.clone(),
        hypothesis: (e.type_ == "analysis.hypothesis").then(|| {
            e.content
                .props
                .get("hypothesis_status")
                .and_then(|v| v.as_str())
                .unwrap_or("open")
                .to_string()
        }),
        confidence: e.content.confidence.clone(),
        level,
        tags: e.content.tags.clone(),
        meta: format!("{} · version {} · {}", who(&e.author), e.ver, e.recorded_at),
        facts: facts_of(e),
        groups: groups_of(db, &e.id),
        history,
        recorded_at: e.recorded_at.clone(),
        author: e.author.clone(),
    }
}

/// The headings of a Markdown text, for an outline: `## What was seen` → `What was seen`.
pub fn headings(markdown: &str) -> Vec<String> {
    let mut in_code = false;
    markdown
        .lines()
        .filter_map(|l| {
            let t = l.trim_start();
            if t.starts_with("```") {
                in_code = !in_code;
                return None;
            }
            if in_code || !t.starts_with('#') {
                return None;
            }
            let h = t.trim_start_matches('#');
            // `#hashtag` is not a heading: a heading has a space after the hashes.
            h.starts_with(' ')
                .then(|| h.trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_db::Case;
    use reactor_db::graph::edit::{self, At};
    use reactor_db::graph::{Batch, Content, Link, Put, Target, apply};
    use reactor_db::manifest::Author;

    fn case() -> (tempfile::TempDir, Case) {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), None).unwrap();
        (dir, case)
    }

    fn put(db: &Db, kind: Kind, type_: &str, value: &str, links: Vec<Link>) -> String {
        let p = Put {
            content: Content::value(value),
            links,
            ..Put::new(kind, type_)
        };
        apply(db, Batch::new(Author::analyst()).put(p))
            .unwrap()
            .ids
            .remove(0)
    }

    fn scale() -> Vec<String> {
        ["unlikely", "possible", "likely", "almost certain"]
            .map(String::from)
            .to_vec()
    }

    const LONG: &str = "## What was seen\n\nThe routine reads the machine id and hashes it with a constant.\n\n## What follows\n\nThe key is a checksum, not a signature.";

    #[test]
    fn a_short_text_is_the_title_and_a_long_one_is_text_with_a_title_of_its_own() {
        let (_d, case) = case();
        let db = case.db();
        let short = put(
            db,
            Kind::Annotation,
            "analysis.note",
            "check the clone",
            vec![],
        );
        let long = put(db, Kind::Annotation, "analysis.hypothesis", LONG, vec![]);

        let s = inspect(db, &query::get(db, &short).unwrap().unwrap(), &scale());
        assert_eq!(
            (s.title.as_str(), s.text.as_deref()),
            ("check the clone", None)
        );
        assert_eq!((s.tone, s.label.as_str()), (Tone::Note, "Note"));

        let l = inspect(db, &query::get(db, &long).unwrap().unwrap(), &scale());
        assert_eq!(l.title, "What was seen", "the first line, without its #");
        assert_eq!(l.text.as_deref(), Some(LONG));
        assert_eq!(l.hypothesis.as_deref(), Some("open"));
        assert!(l.editable);
    }

    #[test]
    fn an_entitys_value_is_its_title_however_long_and_it_cannot_be_edited() {
        let (_d, case) = case();
        let db = case.db();
        let id = edit::add_entity(db, &At::new("s", 1), "file.path", &"a/".repeat(100)).unwrap();
        let i = inspect(db, &query::get(db, &id).unwrap().unwrap(), &scale());
        assert!(i.text.is_none() && !i.editable);
        assert_eq!(i.label, "Entity · path");
    }

    #[test]
    fn relations_are_grouped_by_what_they_say_about_the_element() {
        let (_d, case) = case();
        let db = case.db();
        let file = edit::add_entity(db, &At::new("s", 1), "file.path", "licd").unwrap();
        let h = put(
            db,
            Kind::Annotation,
            "analysis.hypothesis",
            "the check is a keyed hash",
            vec![Link::new("about", Target::Id(file.clone()))],
        );
        let j = put(
            db,
            Kind::Annotation,
            "analysis.judgement",
            "the constant is in .rodata",
            vec![Link::new("supports", Target::Id(h.clone()))],
        );
        let r = put(
            db,
            Kind::Annotation,
            "analysis.judgement",
            "a clone accepted the key",
            vec![Link::new("refutes", Target::Id(h.clone()))],
        );

        let i = inspect(db, &query::get(db, &h).unwrap().unwrap(), &scale());
        let titles: Vec<_> = i
            .groups
            .iter()
            .map(|g| (g.title.as_str(), g.stand, g.items.len()))
            .collect();
        assert_eq!(
            titles,
            [
                ("Supports", Stand::Supports, 1),
                ("Refutes", Stand::Refutes, 1),
                ("About", Stand::Neutral, 1)
            ]
        );
        assert_eq!(i.groups[0].items[0].id, j);
        assert_eq!(i.groups[1].items[0].id, r);
        assert_eq!(i.related(), 3);

        // The file is mentioned by the hypothesis.
        let f = inspect(db, &query::get(db, &file).unwrap().unwrap(), &scale());
        assert_eq!(f.groups[0].title, "Mentioned by");
    }

    #[test]
    fn the_history_says_what_each_version_changed() {
        let (_d, case) = case();
        let db = case.db();
        let at = At::new("s", 1);
        let h = put(
            db,
            Kind::Annotation,
            "analysis.hypothesis",
            "a hypothesis",
            vec![],
        );
        edit::set_confidence(db, &at, std::slice::from_ref(&h), Some("likely")).unwrap();
        edit::set_hypothesis_status(db, &at, &h, "supported").unwrap();
        edit::set_text(db, &at, &h, "a better hypothesis").unwrap();
        edit::set_status(db, &at, std::slice::from_ref(&h), "conflicting").unwrap();

        let i = inspect(db, &query::get(db, &h).unwrap().unwrap(), &scale());
        let changes: Vec<_> = i.history.iter().map(|v| v.change.as_str()).collect();
        assert_eq!(
            changes,
            [
                "created",
                "confidence → likely",
                "hypothesis → supported",
                "text changed",
                "status → conflicting"
            ]
        );
        assert_eq!((i.ver, i.hypothesis.as_deref()), (5, Some("supported")));
        assert_eq!(i.level, Some((2, 4)), "likely is the third of four");
        assert_eq!(i.history[0].who, "analyst");
    }

    #[test]
    fn who_makes_an_author_readable() {
        assert_eq!(who("analyst:alice"), "analyst");
        assert_eq!(who("agent:claude-sonnet@5"), "agent · claude-sonnet");
        assert_eq!(who("tool:strace"), "tool · strace");
    }

    #[test]
    fn a_long_first_line_is_cut_at_a_sentence_or_a_length() {
        assert_eq!(title_of("One. Two."), "One. Two.");
        let long = format!(
            "A sentence that is about forty chars long. {}",
            "word ".repeat(30)
        );
        assert_eq!(
            title_of(&long),
            "A sentence that is about forty chars long."
        );
        assert!(title_of(&"x".repeat(300)).ends_with('…'));
        assert_eq!(title_of("\n\n# Heading\nbody"), "Heading");
    }

    #[test]
    fn headings_are_found_outside_code_and_a_hashtag_is_not_one() {
        let md = "# One\ntext\n## Two\n```\n# not a heading\n```\n#hashtag\n### Three ";
        assert_eq!(headings(md), ["One", "Two", "Three"]);
    }
}
