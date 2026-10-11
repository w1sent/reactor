//! What is written on a node, and how big that makes it.
//!
//! A node is as big as what it says. Its text wraps -- a long path is shown whole, over as
//! many lines as it takes -- and its width is chosen so that the box keeps a shape that reads
//! well: about two and a half times as wide as it is tall. How much it says is its *size*:
//! from only its name (S) to everything known about it (XL).
//!
//! Everything here is a pure function of the node, in pixels at zoom 1. The width of a
//! character is an estimate ([`Metrics`], which the panel takes from the fonts in use), so the
//! height is one too: the panel lets a card grow if the text needs a line more, and nothing is
//! ever cut off. The estimate is what lines end at and what the layout makes room for.

use super::model::{Node, NodeKind};

/// How much a node says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Size {
    /// Its name.
    S,
    /// Its whole value; the start of a text.
    #[default]
    M,
    /// And its tags, and what a text's first paragraph says in full.
    L,
    /// Everything: the whole text, every property.
    Xl,
}

impl Size {
    pub const ALL: [Size; 4] = [Size::S, Size::M, Size::L, Size::Xl];

    pub fn label(self) -> &'static str {
        match self {
            Size::S => "S",
            Size::M => "M",
            Size::L => "L",
            Size::Xl => "XL",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Size::S => "s",
            Size::M => "m",
            Size::L => "l",
            Size::Xl => "xl",
        }
    }

    pub fn parse(s: &str) -> Option<Size> {
        Size::ALL.into_iter().find(|z| z.slug() == s)
    }

    /// What it shows, in a few words (a tooltip).
    pub fn tip(self) -> &'static str {
        match self {
            Size::S => "the name only",
            Size::M => "the whole value, wrapped; the first lines of a text",
            Size::L => "and tags, and a text's first paragraph in full",
            Size::Xl => "everything: the whole text and every property",
        }
    }

    pub fn smaller(self) -> Size {
        match self {
            Size::S | Size::M => Size::S,
            Size::L => Size::M,
            Size::Xl => Size::L,
        }
    }

    pub fn larger(self) -> Size {
        match self {
            Size::S => Size::M,
            Size::M => Size::L,
            Size::L | Size::Xl => Size::Xl,
        }
    }
}

/// How wide text runs: the font's size in pixels at zoom 1, and the width of a character as
/// a part of that -- exact for the monospaced font, an average for the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font: f32,
    pub mono: f32,
    pub sans: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            font: 12.0,
            mono: 0.6,
            sans: 0.52,
        }
    }
}

// The card's measures, in font sizes. The panel draws with the same ones.
/// Space left and right of what is written.
pub const PAD_X: f32 = 0.75;
/// The header: icon, type, and how sure.
pub const HEAD_H: f32 = 2.0;
pub const PAD_TOP: f32 = 0.5;
pub const PAD_BOTTOM: f32 = 0.6;
/// A line of text.
pub const LINE: f32 = 1.45;
/// Between two paragraphs.
pub const GAP: f32 = 0.45;
/// The size of what is small: the type, tags, properties, the last row.
pub const SMALL: f32 = 0.85;
/// A row of small things.
pub const ROW: f32 = 1.5;
/// The icon, and the room it takes with the space after it.
pub const ICON: f32 = 1.1;
pub const ICON_ROOM: f32 = 1.6;
/// The column a property's name is written in.
pub const KEY_W: f32 = 6.0;

/// Twelve pixels: the font size the pixel measures below are meant at.
const BASE: f32 = 12.0;
const MIN_W: f32 = 120.0;
const MAX_W: f32 = 340.0;
/// How wide a long text is written when all of it is: about 58 characters a line.
const TEXT_W: f32 = 400.0;
/// The start of a text: this many lines, then how many more there are.
const START_LINES: usize = 3;
const START_W: f32 = 260.0;
/// Width to height, as a box reads best.
const RATIO: f32 = 2.5;
/// The narrowest and widest a width set by hand may be.
pub const HAND_MIN: f32 = 80.0;
pub const HAND_MAX: f32 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Values: addresses, paths, hashes.
    Mono,
    /// Prose.
    Sans,
}

/// A paragraph on a card.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub text: String,
    pub face: Face,
    /// A heading within a text.
    pub strong: bool,
    /// An item of a list.
    pub bullet: bool,
    /// Said aside: when, how many.
    pub muted: bool,
}

impl Block {
    fn new(text: impl Into<String>, face: Face) -> Block {
        Block {
            text: text.into(),
            face,
            strong: false,
            bullet: false,
            muted: false,
        }
    }
}

/// The header of a card.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    /// The type; an annotation's without `analysis.`.
    pub label: String,
    /// A hypothesis's standing.
    pub chip: Option<String>,
    /// Whether how sure it is is drawn.
    pub meter: bool,
    /// Whether who wrote it (the agent or a person) is drawn, on any kind of node.
    pub author: bool,
}

/// What a node says at a size, and how big that is.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub size: Size,
    /// In pixels at zoom 1.
    pub w: f32,
    pub h: f32,
    /// None at S: an icon and the name.
    pub head: Option<Head>,
    pub blocks: Vec<Block>,
    /// How many lines of its text are not shown.
    pub more: usize,
    pub tags: Vec<String>,
    pub props: Vec<(String, String)>,
    /// A last row: what supports and refutes it, what it is about.
    pub foot: Option<String>,
    /// Whether it says that it is disputed.
    pub disputed: bool,
    /// At the smallest size, which has no header: who wrote it, at the end of the row.
    pub author: bool,
}

// -- wrapping ---------------------------------------------------------------------------------

/// Whether a line may not break before this character: the text system's rule, which keeps
/// a word and what is attached to it together and lets a path break before a `/`.
fn holds(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '-' | '_'
                | '.'
                | '\''
                | '$'
                | '%'
                | '@'
                | '#'
                | '^'
                | '~'
                | ','
                | '='
                | ':'
                | ';'
                | '!'
                | ')'
                | ']'
                | '}'
                | '"'
                | '…'
        )
}

/// One paragraph as the lines it wraps to at `per_line` characters: where each begins and
/// ends, in bytes. A line breaks after a space or before a character that is not part of a
/// word; a word longer than a line breaks where the line ends. Every character is on a line.
pub fn wrap(text: &str, per_line: usize) -> Vec<(usize, usize)> {
    let per_line = per_line.max(1);
    let mut lines = Vec::new();
    let (mut start, mut count) = (0usize, 0usize);
    // Where the line could break, and how many characters of it come before that.
    let mut candidate: Option<(usize, usize)> = None;
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        if i > start && ((prev == ' ' && c != ' ') || (c != ' ' && !holds(c))) {
            candidate = Some((i, count));
        }
        if count >= per_line && c != ' ' {
            let (at, before) = match candidate {
                Some((at, before)) if at > start => (at, before),
                _ => (i, count),
            };
            lines.push((start, at));
            start = at;
            count -= before;
            candidate = None;
        }
        count += 1;
        prev = c;
    }
    if start < text.len() || lines.is_empty() {
        lines.push((start, text.len()));
    }
    lines
}

fn width_of(face: Face, m: &Metrics) -> f32 {
    match face {
        Face::Mono => m.mono,
        Face::Sans => m.sans,
    }
}

/// How many characters of a face fit in this many pixels.
fn per_line(inner: f32, face: Face, scale: f32, m: &Metrics) -> usize {
    ((inner / (width_of(face, m) * scale * m.font)).floor() as usize).max(4)
}

// -- what a node says -------------------------------------------------------------------------

/// The last part of a path: what a file or a key is called. Anything else is its own name.
fn short_name<'a>(type_: &str, text: &'a str) -> &'a str {
    let pathlike = type_.starts_with("file.path")
        || type_.starts_with("file.dir")
        || type_.starts_with("registry.")
        || type_.starts_with("host.share");
    if !pathlike {
        return text;
    }
    let trimmed = text.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(i) if i + 1 < trimmed.len() => &trimmed[i + 1..],
        _ => text,
    }
}

/// The first thing a text says: up to its first full stop, semicolon or line end.
fn first_clause(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = plain(line).0;
    let cut = [". ", "; ", " -- ", " — "]
        .iter()
        .filter_map(|stop| line.find(stop))
        .filter(|&i| i >= 12)
        .min()
        .unwrap_or(line.len());
    let clause = &line[..cut];
    if clause.chars().count() > 64 {
        format!("{}…", clause.chars().take(63).collect::<String>())
    } else {
        clause.to_string()
    }
}

/// A line of a text without the marks that say what it is: `# heading`, `- item`. Returns
/// the words, whether it is a heading, and whether it is an item.
fn plain(line: &str) -> (&str, bool, bool) {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('#') {
        return (rest.trim_start_matches('#').trim(), true, false);
    }
    for mark in ["- ", "* ", "• "] {
        if let Some(rest) = line.strip_prefix(mark) {
            return (rest.trim(), false, true);
        }
    }
    (line, false, false)
}

/// A text as paragraphs: one for each line that says something.
fn paragraphs(text: &str) -> Vec<Block> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (words, strong, bullet) = plain(l);
            Block {
                strong,
                bullet,
                ..Block::new(words, Face::Sans)
            }
        })
        .collect()
}

fn annotation_label(type_: &str) -> String {
    type_
        .strip_prefix("analysis.")
        .unwrap_or(type_)
        .replace('_', " ")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a node says at a size, before it has a width.
struct Draft {
    head: Option<Head>,
    blocks: Vec<Block>,
    /// The rest of a text, counted in lines once the width is known.
    rest: Option<String>,
    /// Keep only the first lines of the first block.
    start_only: bool,
    tags: Vec<String>,
    props: Vec<(String, String)>,
    foot: Option<String>,
    disputed: bool,
    /// No header, so who wrote it goes at the end of the one row.
    tail_author: bool,
}

fn draft(node: &Node, size: Size, plain_picture: bool) -> Draft {
    let disputed = node.status == "conflicting" && !plain_picture && size != Size::S;
    let meter = node.confidence.is_some() && !plain_picture;
    let mut d = Draft {
        head: None,
        blocks: Vec::new(),
        rest: None,
        start_only: false,
        tags: Vec::new(),
        props: Vec::new(),
        foot: None,
        disputed,
        tail_author: false,
    };
    let with_tags = size >= Size::L;
    let with_props = size == Size::Xl;
    if with_tags {
        d.tags = node.tags.clone();
    }
    if with_props {
        // Everything known of it, so that the largest size always says more than the one
        // before: when, how sure, how it stands, who recorded it, how connected it is.
        let mut known: Vec<(String, String)> = Vec::new();
        if let Some(when) = &node.when {
            known.push(("when".into(), when.clone()));
        }
        if let (Some(c), false) = (&node.confidence, plain_picture) {
            known.push(("confidence".into(), c.clone()));
        }
        if node.status != "active" && !plain_picture {
            known.push(("status".into(), node.status.clone()));
        }
        if node.kind != NodeKind::Annotation {
            let by = if node.by_agent { "agent" } else { "person" };
            known.push(("by".into(), by.into()));
            known.push(("lines".into(), node.links.to_string()));
        }
        known.extend(node.props.iter().cloned());
        d.props = known;
    }
    // From L up a thing says how connected it is.
    if with_tags && node.kind != NodeKind::Annotation && !with_props && node.links > 0 {
        d.foot = Some(plural(node.links, "line", "lines"));
    }
    match node.kind {
        NodeKind::Entity => {
            let value = if node.text.is_empty() {
                node.label.as_str()
            } else {
                node.text.as_str()
            };
            if size == Size::S {
                d.blocks
                    .push(Block::new(short_name(&node.type_, value), Face::Mono));
                d.tail_author = true;
                return d;
            }
            d.head = Some(Head {
                label: node.type_.clone(),
                chip: None,
                meter,
                author: true,
            });
            d.blocks.push(Block::new(value, Face::Mono));
        }
        NodeKind::Claim => {
            if size == Size::S {
                d.blocks.push(Block::new(node.type_.clone(), Face::Mono));
                d.tail_author = true;
                return d;
            }
            d.head = Some(Head {
                label: node.type_.clone(),
                chip: None,
                meter,
                author: true,
            });
            if !node.text.is_empty() && node.text != node.type_ {
                d.blocks.push(Block::new(node.text.clone(), Face::Sans));
            }
            let mut aside = Vec::new();
            if let (Some(when), false) = (&node.when, with_props) {
                aside.push(when.clone());
            }
            if node.roles > 0 {
                aside.push(plural(node.roles, "role", "roles"));
            }
            if !aside.is_empty() {
                d.blocks.push(Block {
                    muted: true,
                    ..Block::new(aside.join(" · "), Face::Mono)
                });
            }
        }
        NodeKind::Annotation => {
            let text = if node.text.is_empty() {
                node.label.as_str()
            } else {
                node.text.as_str()
            };
            if size == Size::S {
                d.blocks.push(Block::new(first_clause(text), Face::Sans));
                d.tail_author = true;
                return d;
            }
            d.head = Some(Head {
                label: annotation_label(&node.type_),
                chip: node.hypothesis.clone().filter(|_| !plain_picture),
                meter,
                author: true,
            });
            let mut all = paragraphs(text);
            match size {
                Size::Xl => d.blocks = all,
                _ => {
                    // The first paragraph; at M only its first lines.
                    if !all.is_empty() {
                        let rest = all.split_off(1);
                        d.rest = Some(
                            rest.iter()
                                .map(|b| b.text.as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                        )
                        .filter(|r: &String| !r.is_empty());
                        d.blocks = all;
                    }
                    d.start_only = size == Size::M;
                }
            }
            if size >= Size::L {
                let mut said = Vec::new();
                if node.type_ == "analysis.hypothesis" || node.supports + node.refutes > 0 {
                    said.push(format!("+{} supports", node.supports));
                    said.push(format!("{} refutes", node.refutes));
                }
                if node.roles > 0 && size == Size::Xl {
                    said.push(format!("about {}", node.roles));
                }
                if !said.is_empty() {
                    d.foot = Some(said.join(" · "));
                }
            }
        }
    }
    d
}

// -- how big that is --------------------------------------------------------------------------

/// The narrowest a header can be: its icon, its words and what is at its right.
fn head_width(head: &Head, m: &Metrics) -> f32 {
    let f = m.font;
    // The space at both sides, the icon, the gap after it, and a little over for the border
    // and for the estimate being a character short.
    let mut w = (2.0 * PAD_X + ICON + 0.5 + 0.9) * f;
    w += head.label.chars().count() as f32 * m.mono * SMALL * f;
    if let Some(chip) = &head.chip {
        w += (chip.chars().count() as f32 * m.sans * SMALL + 1.6) * f;
    }
    if head.meter || head.author {
        // The gaps either side of the room that pushes them to the right.
        w += 1.0 * f;
    }
    if head.meter {
        w += 3.3 * f;
    }
    if head.author {
        w += (ICON + 0.5) * f;
    }
    w
}

/// How high the rows of tags are, wrapped into `inner` pixels.
fn tag_rows(tags: &[String], inner: f32, m: &Metrics) -> usize {
    let f = m.font;
    let (mut rows, mut x) = (1usize, 0.0f32);
    for tag in tags {
        let w = (tag.chars().count() as f32 * m.mono * SMALL + 1.2) * f;
        if x > 0.0 && x + w > inner {
            rows += 1;
            x = 0.0;
        }
        x += w + 0.4 * f;
    }
    rows
}

/// The height of a draft at a width, and how many lines of its text that leaves out.
fn height(d: &Draft, w: f32, m: &Metrics) -> (f32, usize) {
    let f = m.font;
    let side = if d.head.is_none() {
        ICON_ROOM + if d.tail_author { ICON_ROOM } else { 0.0 }
    } else {
        0.0
    };
    let inner = w - (2.0 * PAD_X + side) * f;
    let mut h = PAD_TOP + PAD_BOTTOM;
    if d.head.is_some() {
        h += HEAD_H;
    }
    let mut more = 0usize;
    for (i, b) in d.blocks.iter().enumerate() {
        let room = inner - if b.bullet { 1.1 * f } else { 0.0 };
        let mut lines = wrap(&b.text, per_line(room, b.face, 1.0, m)).len();
        if d.start_only && i == 0 && lines > START_LINES {
            more += lines - START_LINES;
            lines = START_LINES;
        }
        h += lines as f32 * LINE;
        if i > 0 {
            h += GAP;
        }
    }
    if let Some(rest) = &d.rest {
        let per = per_line(inner, Face::Sans, 1.0, m);
        more += rest.lines().map(|l| wrap(l, per).len()).sum::<usize>();
    }
    if more > 0 {
        h += ROW;
    }
    if !d.tags.is_empty() {
        h += tag_rows(&d.tags, inner, m) as f32 * ROW + GAP;
    }
    if !d.props.is_empty() {
        let per = per_line(inner - KEY_W * SMALL * f, Face::Mono, SMALL, m);
        let lines: usize = d.props.iter().map(|(_, v)| wrap(v, per).len()).sum();
        h += lines as f32 * LINE * SMALL + 2.0 * GAP;
    }
    if d.foot.is_some() {
        h += ROW;
    }
    if d.disputed {
        h += ROW;
    }
    (h * f, more)
}

/// The width a draft would need for each of its paragraphs to be one line.
fn natural(d: &Draft, m: &Metrics) -> f32 {
    let f = m.font;
    let side = if d.head.is_none() {
        ICON_ROOM + if d.tail_author { ICON_ROOM } else { 0.0 }
    } else {
        0.0
    };
    let longest = d
        .blocks
        .iter()
        .map(|b| {
            b.text.chars().count() as f32 * width_of(b.face, m) * f
                + if b.bullet { 1.1 * f } else { 0.0 }
        })
        .fold(0.0f32, f32::max);
    // A little over, so that the estimate being a character short does not wrap the line.
    longest + (2.0 * PAD_X + side + 0.6) * f
}

/// What `node` says at `size`, and how big that is. `width` is one set by hand; `plain` is
/// the picture without status and confidence (the *Basic* level of detail).
pub fn card(node: &Node, size: Size, width: Option<f32>, plain: bool, m: &Metrics) -> Card {
    let d = draft(node, size, plain);
    let k = m.font / BASE;
    let floor = d
        .head
        .as_ref()
        .map_or(0.0, |h| head_width(h, m))
        .max(if d.head.is_some() {
            MIN_W * k
        } else {
            3.0 * m.font
        });
    let nat = natural(&d, m);
    let w = match width {
        Some(w) => w.clamp(HAND_MIN * k, HAND_MAX * k),
        None => {
            let text = node.kind == NodeKind::Annotation;
            let cap = match (size, text) {
                (Size::S, _) => 220.0 * k,
                (Size::Xl, true) => TEXT_W * k,
                (Size::M, true) => START_W * k,
                _ => MAX_W * k,
            };
            let cap = cap.max(floor);
            if nat <= cap {
                // Everything on one line each: as wide as that takes.
                (nat / 10.0).ceil() * 10.0
            } else if text && size != Size::L {
                // Prose is written at a width that reads, however long it is.
                cap
            } else {
                // The width whose box comes nearest the shape that reads best; the wider
                // of two that are as near.
                let mut best = (cap, f32::MAX);
                let mut w = (floor / 20.0).ceil() * 20.0;
                while w <= cap + 0.5 {
                    let off = (w / height(&d, w, m).0 - RATIO).abs();
                    if off <= best.1 {
                        best = (w, off);
                    }
                    w += 20.0;
                }
                best.0
            }
            .max(floor)
        }
    };
    let (h, more) = height(&d, w, m);
    Card {
        size,
        w,
        h,
        head: d.head,
        blocks: d.blocks,
        more,
        tags: d.tags,
        props: d.props,
        foot: d.foot,
        disputed: d.disputed,
        author: d.tail_author,
    }
}

/// The first lines of a paragraph, as the card of a text's start shows them: cut where the
/// last of them ends, with an ellipsis if there was more.
pub fn start_of(text: &str, w: f32, m: &Metrics) -> String {
    let inner = w - 2.0 * PAD_X * m.font;
    let lines = wrap(text, per_line(inner, Face::Sans, 1.0, m));
    if lines.len() <= START_LINES {
        return text.to_string();
    }
    let end = lines[START_LINES - 1].1;
    // Room for the ellipsis on the last line.
    let cut = text[..end].trim_end();
    let cut = match cut.char_indices().nth_back(1) {
        Some((i, _)) if cut.len() > 2 => &cut[..i],
        _ => cut,
    };
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/private/var/mobile/Containers/Data/Application/7C1E0A52-9B3D-4F6A-A1C8-2E5D90B7F314/Library/Caches/com.apple.nsurlsessiond/Downloads/update_helper.dylib";
    const NOTE: &str = "Ask IT whether ws-042 was reimaged in March. The EDR gap would explain the missing process events.\n\n## Checked so far\n- EDR has no process events for ws-042 from 1 to 19 March.\n- The VPN log has jdoe logging on from ws-042 on 12 March, so the host was in use.\n- Asset inventory lists a new disk serial from 20 March.\n\nIf it was reimaged, the dylib cannot have been written before the 20th, and the foothold hypothesis needs another first host.";

    fn m() -> Metrics {
        Metrics::default()
    }

    fn entity(type_: &str, value: &str) -> Node {
        Node::bare("e", NodeKind::Entity, type_, value)
    }

    fn note(text: &str) -> Node {
        Node {
            text: text.into(),
            ..Node::bare("n", NodeKind::Annotation, "analysis.note", "a note")
        }
    }

    fn joined(text: &str, per: usize) -> String {
        wrap(text, per)
            .into_iter()
            .map(|(a, b)| &text[a..b])
            .collect()
    }

    #[test]
    fn a_line_breaks_after_a_space_or_before_a_separator_and_loses_nothing() {
        let lines: Vec<&str> = wrap(PATH, 36)
            .into_iter()
            .map(|(a, b)| &PATH[a..b])
            .collect();
        assert!(lines.len() >= 4, "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 36), "{lines:?}");
        // A path breaks before a slash where it can, not in the middle of a name.
        assert!(
            lines[1..].iter().filter(|l| l.starts_with('/')).count() >= lines.len() - 2,
            "{lines:?}"
        );
        for per in [1, 5, 16, 36, 80, 400] {
            assert_eq!(joined(PATH, per), PATH, "at {per}");
            assert_eq!(joined(NOTE, per), NOTE, "at {per}");
        }
        // A word longer than a line is broken where the line ends; words stay whole.
        let hash = "9f2c4e1ab07d53c8e6f1420b9a7d3e5c81f0a2b4c6d8e0f13579bdf2468ace01";
        assert_eq!(wrap(hash, 16).len(), 4);
        let words: Vec<&str> = wrap("one two three four", 9)
            .into_iter()
            .map(|(a, b)| &"one two three four"[a..b])
            .collect();
        assert_eq!(words, ["one two ", "three ", "four"]);
        assert_eq!(wrap("", 10), [(0, 0)]);
        assert_eq!(wrap("häuser straße", 6).len(), 2);
    }

    #[test]
    fn a_short_value_is_one_line_and_a_long_path_is_shown_whole_in_a_box_that_reads() {
        let ip = card(
            &entity("net.ipv4", "198.51.100.7"),
            Size::M,
            None,
            false,
            &m(),
        );
        assert!(ip.w >= 120.0 && ip.w <= 180.0, "{}", ip.w);
        assert!(ip.h < 60.0, "{}", ip.h);
        assert_eq!(ip.blocks[0].text, "198.51.100.7");

        let path = card(&entity("file.path", PATH), Size::M, None, false, &m());
        assert_eq!(path.blocks[0].text, PATH, "nothing is cut");
        assert_eq!(path.more, 0);
        assert!(path.w <= 340.0, "{}", path.w);
        let shape = path.w / path.h;
        assert!(
            (1.7..=3.4).contains(&shape),
            "{} x {} = {shape}",
            path.w,
            path.h
        );
        // The type always fits in the header.
        let long_type = card(
            &entity("file.hash.authentihash.sha256", "ab"),
            Size::M,
            None,
            false,
            &m(),
        );
        assert!(long_type.w > 200.0, "{}", long_type.w);
    }

    #[test]
    fn each_size_says_more_and_the_smallest_says_the_name() {
        let node = Node {
            tags: vec!["phase.c2".into(), "stage2".into()],
            props: vec![("size".into(), "184320".into())],
            ..entity("file.path", PATH)
        };
        let at = |size| card(&node, size, None, false, &m());
        let (s, mid, l, xl) = (at(Size::S), at(Size::M), at(Size::L), at(Size::Xl));
        assert!(s.head.is_none());
        assert_eq!(s.blocks[0].text, "update_helper.dylib");
        assert!(
            s.h < mid.h && mid.h < l.h && l.h < xl.h,
            "{} {} {} {}",
            s.h,
            mid.h,
            l.h,
            xl.h
        );
        assert!(mid.tags.is_empty() && l.tags.len() == 2 && l.props.is_empty());
        assert!(
            xl.props
                .contains(&("size".to_string(), "184320".to_string()))
        );
        // The largest says who recorded it and how connected it is, tags or not.
        assert!(xl.props.contains(&("by".to_string(), "person".to_string())));
        let bare = entity("net.ipv4", "198.51.100.7");
        let linked = Node {
            links: 3,
            ..bare.clone()
        };
        let h = |n: &Node, size| card(n, size, None, false, &m()).h;
        assert!(h(&bare, Size::Xl) > h(&bare, Size::M));
        assert!(h(&linked, Size::L) > h(&linked, Size::M));
        assert_eq!(
            card(&linked, Size::L, None, false, &m()).foot.as_deref(),
            Some("3 lines")
        );
        // A value that is no path is its own name; a path that ends in a slash too.
        let short = |t: &str, v: &str| {
            card(&entity(t, v), Size::S, None, false, &m()).blocks[0]
                .text
                .clone()
        };
        assert_eq!(
            short("net.url", "https://a.example/x/y"),
            "https://a.example/x/y"
        );
        assert_eq!(short("file.dir", "C:\\Users\\jdoe\\"), "jdoe");
        assert_eq!(short("registry.key", "HKLM\\Software\\Run"), "Run");
    }

    #[test]
    fn a_text_starts_at_m_has_its_first_paragraph_at_l_and_is_whole_at_xl() {
        let n = note(NOTE);
        let at = |size| card(&n, size, None, false, &m());
        let (s, mid, l, xl) = (at(Size::S), at(Size::M), at(Size::L), at(Size::Xl));
        assert_eq!(
            s.blocks[0].text,
            "Ask IT whether ws-042 was reimaged in March"
        );
        assert_eq!(mid.blocks.len(), 1);
        assert!(mid.more >= 6, "{}", mid.more);
        assert!(mid.w <= 260.0);
        // Its first paragraph is three lines at this width, so it is all there; a longer
        // one is cut where the third line ends.
        assert_eq!(
            start_of(&mid.blocks[0].text, mid.w, &m()),
            mid.blocks[0].text
        );
        let long = "word ".repeat(60);
        let shown = start_of(long.trim(), mid.w, &m());
        assert!(shown.ends_with('…') && shown.len() < 140, "{shown}");
        assert!(long.starts_with(shown.trim_end_matches('…')));
        let long_note = card(&note(long.trim()), Size::M, None, false, &m());
        assert!(
            long_note.more >= 4 && long_note.w == 260.0,
            "{}",
            long_note.more
        );
        // L: the first paragraph in full, and how much is left.
        assert_eq!(l.blocks.len(), 1);
        assert!(l.blocks[0].text.ends_with("process events."));
        assert!(l.more >= 5 && l.more <= mid.more, "{} {}", l.more, mid.more);
        // XL: every line, at the width a text is read at; headings and items are marked.
        assert_eq!(xl.more, 0);
        assert_eq!(xl.blocks.len(), 6);
        assert_eq!(xl.w, 400.0);
        assert!(xl.blocks[1].strong && xl.blocks[1].text == "Checked so far");
        assert!(xl.blocks[2].bullet && xl.blocks[2].text.starts_with("EDR has"));
        assert!(xl.h > l.h && l.h >= mid.h, "{} {} {}", xl.h, l.h, mid.h);
        let said: String = xl
            .blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        for word in ["reimaged", "inventory", "foothold", "another first host."] {
            assert!(said.contains(word), "{word}");
        }
        // A short one is itself at every size from M.
        let short = note("Looks benign.");
        for size in [Size::M, Size::L, Size::Xl] {
            let c = card(&short, size, None, false, &m());
            assert_eq!((c.blocks[0].text.as_str(), c.more), ("Looks benign.", 0));
            assert_eq!(start_of(&c.blocks[0].text, c.w, &m()), "Looks benign.");
        }
    }

    #[test]
    fn a_width_set_by_hand_is_kept_and_the_height_follows_the_text() {
        let node = entity("file.path", PATH);
        let narrow = card(&node, Size::M, Some(160.0), false, &m());
        let wide = card(&node, Size::M, Some(700.0), false, &m());
        assert_eq!((narrow.w, wide.w), (160.0, 700.0));
        assert!(narrow.h > wide.h * 2.0, "{} {}", narrow.h, wide.h);
        assert_eq!(card(&node, Size::M, Some(5.0), false, &m()).w, HAND_MIN);
        assert_eq!(card(&node, Size::M, Some(9e9), false, &m()).w, HAND_MAX);
        // A bigger font makes a bigger card, in proportion.
        let big = Metrics { font: 24.0, ..m() };
        let (a, b) = (
            card(&node, Size::M, None, false, &m()),
            card(&node, Size::M, None, false, &big),
        );
        assert!((b.w / a.w - 2.0).abs() < 0.2 && (b.h / a.h - 2.0).abs() < 0.3);
    }

    #[test]
    fn what_was_concluded_says_how_it_stands_unless_the_picture_is_plain() {
        let hyp = Node {
            confidence: Some("likely".into()),
            hypothesis: Some("open".into()),
            status: "conflicting".into(),
            supports: 1,
            ..Node {
                text: "ws-042 was the initial foothold.".into(),
                ..Node::bare("h", NodeKind::Annotation, "analysis.hypothesis", "h")
            }
        };
        let c = card(&hyp, Size::L, None, false, &m());
        let head = c.head.clone().unwrap();
        assert_eq!(head.label, "hypothesis");
        assert_eq!(head.chip.as_deref(), Some("open"));
        assert!(head.meter && head.author && c.disputed);
        assert_eq!(c.foot.as_deref(), Some("+1 supports · 0 refutes"));
        let plain = card(&hyp, Size::L, None, true, &m());
        let head = plain.head.unwrap();
        assert!(head.chip.is_none() && !head.meter && !plain.disputed);
        assert!(plain.h < c.h);
        // A claim says when and how many take part.
        let claim = Node {
            when: Some("2026-03-12T02:11:47Z".into()),
            roles: 3,
            ..Node::bare("c", NodeKind::Claim, "process.start", "process.start")
        };
        let c = card(&claim, Size::M, None, false, &m());
        assert_eq!(c.head.unwrap().label, "process.start");
        assert_eq!(c.blocks[0].text, "2026-03-12T02:11:47Z · 3 roles");
        assert!(c.blocks[0].muted);
        assert_eq!(
            card(&claim, Size::S, None, false, &m()).blocks[0].text,
            "process.start"
        );
    }

    #[test]
    fn every_kind_of_node_says_who_wrote_it_in_its_header_or_at_the_smallest_size() {
        let claim = Node::bare("c", NodeKind::Claim, "process.start", "process.start");
        for node in [
            entity("net.ipv4", "198.51.100.7"),
            claim,
            note("Looks benign."),
        ] {
            for size in [Size::M, Size::L, Size::Xl] {
                let c = card(&node, size, None, false, &m());
                assert!(
                    c.head.as_ref().is_some_and(|h| h.author),
                    "{:?} at {size:?}",
                    node.kind
                );
                assert!(!c.author);
            }
            // No header at S: the author sits at the end of the row, which makes room for it.
            let s = card(&node, Size::S, None, false, &m());
            assert!(s.head.is_none() && s.author, "{:?}", node.kind);
            let bare = Draft {
                tail_author: false,
                ..draft(&node, Size::S, false)
            };
            assert!(s.w > natural(&bare, &m()), "{:?}", node.kind);
        }
    }

    #[test]
    fn sizes_step_and_stop_at_the_ends() {
        assert_eq!(Size::S.smaller(), Size::S);
        assert_eq!(Size::Xl.larger(), Size::Xl);
        assert_eq!(Size::M.larger().larger(), Size::Xl);
        assert_eq!(Size::M.smaller(), Size::S);
        for size in Size::ALL {
            assert_eq!(Size::parse(size.slug()), Some(size));
        }
        assert_eq!(Size::parse("huge"), None);
        assert_eq!(Size::default(), Size::M);
    }
}
