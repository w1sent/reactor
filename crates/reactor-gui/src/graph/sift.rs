//! What of the graph is drawn, and what is found in it.
//!
//! **Filter layers** stack: each is a filter ([`super::model::Filter`]) and what to do with
//! the nodes it matches -- show only those, hide them, dim them, or highlight them. They are
//! applied one after another, so "only the C2 phase, without what was deleted, with the
//! agent's guesses dimmed" is three layers and each can be switched off on its own.
//!
//! **Find** looks for words in what is drawn -- labels, the whole text, and what is known
//! about a node (tags, properties, status, confidence, who wrote it) -- and hides nothing: it
//! says where the matches are.
//!
//! Pure functions of the model, tested without a window.

use std::ops::Range;

use super::model::{Filter, Model, Node};

/// What a layer does with the nodes its filter matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only these are drawn.
    #[default]
    ShowOnly,
    /// These are not drawn.
    Hide,
    /// These are drawn faintly.
    Dim,
    /// These are drawn with a ring round them.
    Highlight,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::ShowOnly, Mode::Hide, Mode::Dim, Mode::Highlight];

    pub fn label(self) -> &'static str {
        match self {
            Mode::ShowOnly => "Show only",
            Mode::Hide => "Hide",
            Mode::Dim => "Dim",
            Mode::Highlight => "Highlight",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Mode::ShowOnly => "only",
            Mode::Hide => "hide",
            Mode::Dim => "dim",
            Mode::Highlight => "highlight",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.slug() == s)
    }

    /// The one after it, round and round: what a click on a layer's mode picks.
    pub fn next(self) -> Mode {
        match self {
            Mode::ShowOnly => Mode::Hide,
            Mode::Hide => Mode::Dim,
            Mode::Dim => Mode::Highlight,
            Mode::Highlight => Mode::ShowOnly,
        }
    }
}

/// One layer: a filter, what it does, and whether it is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub filter: Filter,
    pub mode: Mode,
    pub on: bool,
}

/// How one node is drawn once the layers have had their say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drawn {
    pub shown: bool,
    pub dim: bool,
    pub highlight: bool,
}

impl Default for Drawn {
    fn default() -> Self {
        Drawn {
            shown: true,
            dim: false,
            highlight: false,
        }
    }
}

/// The nodes as the layers leave them, and how many nodes each layer matches (whether it is
/// on or not, so that a layer says what it would do).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sifted {
    pub nodes: Vec<Drawn>,
    pub matched: Vec<usize>,
}

/// Apply the layers, top to bottom. A layer with nothing typed does nothing.
pub fn sift(model: &Model, layers: &[Layer]) -> Sifted {
    let mut nodes = vec![Drawn::default(); model.nodes.len()];
    let mut matched = Vec::with_capacity(layers.len());
    for layer in layers {
        if layer.filter.is_empty() {
            matched.push(0);
            continue;
        }
        let mut count = 0;
        for (node, drawn) in model.nodes.iter().zip(&mut nodes) {
            let hit = layer.filter.matches(node);
            count += usize::from(hit);
            if !layer.on {
                continue;
            }
            match layer.mode {
                Mode::ShowOnly => drawn.shown &= hit,
                Mode::Hide => drawn.shown &= !hit,
                Mode::Dim => drawn.dim |= hit,
                Mode::Highlight => drawn.highlight |= hit,
            }
        }
        matched.push(count);
    }
    Sifted { nodes, matched }
}

/// Where find looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    /// What a node is called and what it is: its label and its type.
    pub labels: bool,
    /// All of its text.
    pub content: bool,
    /// What is known about it: tags, properties, status, confidence, who wrote it, its id.
    pub meta: bool,
}

impl Default for Scope {
    fn default() -> Self {
        Scope {
            labels: true,
            content: true,
            meta: true,
        }
    }
}

/// Where in a node the words were found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Label,
    Content,
    /// In what is known about it: which thing (`tag`, a property's name, `status`…).
    Meta(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// An index into the model's nodes.
    pub node: usize,
    pub found: Found,
}

fn found_in(node: &Node, query: &str, scope: Scope) -> Option<Found> {
    let has = |s: &str| s.to_lowercase().contains(query);
    if scope.labels && (has(&node.label) || has(&node.type_)) {
        return Some(Found::Label);
    }
    if scope.content && has(&node.text) {
        return Some(Found::Content);
    }
    if !scope.meta {
        return None;
    }
    if node.tags.iter().any(|t| has(t)) {
        return Some(Found::Meta("tag".into()));
    }
    if let Some((key, _)) = node.props.iter().find(|(k, v)| has(k) || has(v)) {
        return Some(Found::Meta(key.clone()));
    }
    let known = [
        ("status", Some(node.status.as_str())),
        ("confidence", node.confidence.as_deref()),
        ("hypothesis", node.hypothesis.as_deref()),
        ("when", node.when.as_deref()),
        ("by", Some(if node.by_agent { "agent" } else { "person" })),
        ("id", Some(node.id.as_str())),
    ];
    known
        .into_iter()
        .find(|(_, value)| value.is_some_and(has))
        .map(|(what, _)| Found::Meta(what.into()))
}

/// The shown nodes that have `query` in them, and where: at most one hit a node, the first
/// place it is found in (label, then text, then the rest). Case is ignored; nothing typed
/// finds nothing.
pub fn find(model: &Model, shown: &[bool], query: &str, scope: Scope) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    model
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| shown.get(*i).copied().unwrap_or(false))
        .filter_map(|(i, node)| found_in(node, &query, scope).map(|found| Hit { node: i, found }))
        .collect()
}

/// Where `query` is in `text`, for marking it: every place, not overlapping. Case is ignored
/// for the letters a to z, which keeps the places exact in text that has others.
pub fn marks(text: &str, query: &str) -> Vec<Range<usize>> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(&query) {
        let start = from + at;
        out.push(start..start + query.len());
        from = start + query.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{NodeKind, parse_filter};

    fn model() -> Model {
        let node = |id: &str, kind, type_: &str, label: &str, tags: &[&str], agent: bool| Node {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            by_agent: agent,
            ..Node::bare(id, kind, type_, label)
        };
        let mut hyp = node(
            "h",
            NodeKind::Annotation,
            "analysis.hypothesis",
            "ws-042 was the foothold",
            &["phase.c2"],
            true,
        );
        hyp.text = "ws-042 was the foothold.\n\nThe loader came over the Beacon channel.".into();
        hyp.confidence = Some("likely".into());
        let mut ip = node(
            "i",
            NodeKind::Entity,
            "net.ipv4",
            "198.51.100.7",
            &["c2.beacon"],
            false,
        );
        ip.props = vec![("asn".into(), "AS64500".into())];
        let mut gone = node("g", NodeKind::Entity, "host.name", "old-host", &[], false);
        gone.status = "soft-deleted".into();
        Model {
            nodes: vec![
                hyp,
                ip,
                node(
                    "p",
                    NodeKind::Entity,
                    "process.name",
                    "powershell.exe",
                    &["phase.c2"],
                    true,
                ),
                gone,
            ],
            lines: Vec::new(),
        }
    }

    fn layer(text: &str, mode: Mode, on: bool) -> Layer {
        Layer {
            filter: parse_filter(text),
            mode,
            on,
        }
    }

    #[test]
    fn layers_stack_and_one_that_is_off_or_empty_does_nothing() {
        let m = model();
        let none = sift(&m, &[]);
        assert!(none.nodes.iter().all(|d| *d == Drawn::default()));
        let layers = [
            layer("tag:phase.c2|c2.", Mode::ShowOnly, true),
            layer("status:deleted", Mode::Hide, true),
            layer("type:process.", Mode::Dim, true),
            layer("conf:>=likely", Mode::Highlight, true),
            layer("by:agent", Mode::Hide, false),
            layer("", Mode::Hide, true),
        ];
        let s = sift(&m, &layers);
        let shown: Vec<bool> = s.nodes.iter().map(|d| d.shown).collect();
        assert_eq!(shown, [true, true, true, false]);
        assert_eq!(
            s.nodes.iter().map(|d| d.dim).collect::<Vec<_>>(),
            [false, false, true, false]
        );
        assert_eq!(
            s.nodes.iter().map(|d| d.highlight).collect::<Vec<_>>(),
            [true, false, false, false]
        );
        // Each says how many it matches, the one that is off too; the empty one none.
        assert_eq!(s.matched, [3, 1, 1, 1, 2, 0]);
        // Two that show only are both to hold; hiding wins over showing.
        let s = sift(
            &m,
            &[
                layer("tag:phase.c2", Mode::ShowOnly, true),
                layer("by:agent", Mode::ShowOnly, true),
                layer("kind:note", Mode::Hide, true),
            ],
        );
        assert_eq!(
            s.nodes.iter().map(|d| d.shown).collect::<Vec<_>>(),
            [false, false, true, false]
        );
    }

    #[test]
    fn find_looks_in_labels_text_and_what_is_known_and_only_in_what_is_shown() {
        let m = model();
        let all = [true; 4];
        let hits = find(&m, &all, " BEACON ", Scope::default());
        assert_eq!(
            hits,
            [
                Hit {
                    node: 0,
                    found: Found::Content
                },
                Hit {
                    node: 1,
                    found: Found::Meta("tag".into())
                },
            ]
        );
        // A scope that is off is not looked in.
        let only = |labels, content, meta| Scope {
            labels,
            content,
            meta,
        };
        assert_eq!(find(&m, &all, "beacon", only(true, false, false)), []);
        assert_eq!(find(&m, &all, "beacon", only(false, true, false)).len(), 1);
        assert_eq!(find(&m, &all, "beacon", only(false, false, true)).len(), 1);
        // The label and the type; a property's name or value; who wrote it; the status.
        assert_eq!(
            find(&m, &all, "ipv4", Scope::default())[0].found,
            Found::Label
        );
        assert_eq!(
            find(&m, &all, "as64500", Scope::default())[0].found,
            Found::Meta("asn".into())
        );
        assert_eq!(find(&m, &all, "agent", Scope::default()).len(), 2);
        assert_eq!(
            find(&m, &all, "likely", Scope::default())[0].found,
            Found::Meta("confidence".into())
        );
        assert_eq!(find(&m, &all, "soft-deleted", Scope::default())[0].node, 3);
        // What is not drawn is not found, and nothing typed finds nothing.
        assert_eq!(
            find(&m, &[true, false, true, true], "beacon", Scope::default()).len(),
            1
        );
        assert_eq!(find(&m, &all, "  ", Scope::default()), []);
        assert_eq!(find(&m, &all, "nowhere", Scope::default()), []);
    }

    #[test]
    fn marks_are_every_place_the_words_are_whatever_their_case() {
        assert_eq!(
            marks("the Beacon and the beacon", "beacon"),
            [4..10, 19..25]
        );
        assert_eq!(marks("aaaa", "aa"), [0..2, 2..4]);
        assert_eq!(marks("straße BEACON", "Beacon"), vec![8..14]);
        assert!(marks("nothing", "beacon").is_empty());
        assert!(marks("anything", " ").is_empty());
    }

    #[test]
    fn a_mode_is_stepped_through_and_read_back() {
        let mut mode = Mode::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(mode);
            mode = mode.next();
        }
        assert_eq!(seen, Mode::ALL);
        assert_eq!(mode, Mode::ShowOnly);
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.slug()), Some(mode));
        }
    }
}
