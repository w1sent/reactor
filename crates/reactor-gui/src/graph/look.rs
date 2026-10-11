//! How a type looks on the plane: its icon and its colour.
//!
//! The vocabulary says what it can ([`reactor_db::graph::vocab`]): a type's own icon and
//! colour, else those of the namespace it is in. What it leaves unsaid is settled here: an
//! icon for the *kind* of thing (a box for an entity, a bolt for an event, an eye for an
//! observation, a note for an annotation) and a neutral grey. So a type somebody just made
//! up is drawn at once, and can be given a look of its own later.
//!
//! Icons are Lucide's, by their names; the colours are named too, so that what is stored
//! stays the same when a theme changes what they look like.

use std::collections::HashSet;
use std::sync::OnceLock;

use gpui_kit::assets::IconName;
use gpui_kit::{Hsla, SharedString, rgb};
use reactor_db::graph::Kind;
use reactor_db::graph::vocab::Vocabulary;

/// The colours a type can have, as they are drawn. The names are the vocabulary's
/// ([`reactor_db::graph::vocab::COLOURS`]); orange and red are kept for *disputed* and
/// *rejected*.
pub const PALETTE: [(&str, u32); 20] = [
    ("grey", 0xA7B0BF),
    ("slate", 0x8496B0),
    ("cream", 0xE6E1CF),
    ("sand", 0xE6CC96),
    ("brown", 0xBFAA80),
    ("yellow", 0xFFD173),
    ("olive", 0xC5CC5A),
    ("lime", 0xAAD94C),
    ("green", 0x87D96C),
    ("mint", 0x7FD8A6),
    ("teal", 0x95E6CB),
    ("cyan", 0x5CCFE6),
    ("sky", 0x8AD6FF),
    ("blue", 0x59C2FF),
    ("cobalt", 0x6C9CFF),
    ("indigo", 0x8F93FF),
    ("violet", 0xB39DFF),
    ("purple", 0xD2A6FF),
    ("magenta", 0xE58FE8),
    ("pink", 0xF28FAD),
];

/// The colour of a type that has none.
pub const NEUTRAL: &str = "grey";

pub fn colour(name: &str) -> Option<Hsla> {
    PALETTE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, hex)| rgb(*hex).into())
}

/// A palette colour as ink on the theme's ground: as it is on a dark one, darker on a
/// light one, where the pale colours would not read.
pub fn ink(colour: Hsla, dark: bool) -> Hsla {
    if dark {
        colour
    } else {
        Hsla {
            l: (colour.l * 0.5).min(0.38),
            s: (colour.s * 0.9).min(0.8),
            ..colour
        }
    }
}

/// Every icon there is, by name, in order.
pub fn icon_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: Vec<String> = IconName::ALL
            .iter()
            .filter_map(|icon| {
                let path = icon.path();
                Some(
                    path.strip_prefix("icons/")?
                        .strip_suffix(".svg")?
                        .to_string(),
                )
            })
            .collect();
        names.sort();
        names
    })
}

pub fn has_icon(name: &str) -> bool {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| icon_names().iter().map(String::as_str).collect())
        .contains(name)
}

/// Where an icon is among the assets.
pub fn icon_path(name: &str) -> SharedString {
    SharedString::from(format!("icons/{name}.svg"))
}

/// The icon of a kind of element: what a type is drawn with when nothing says otherwise.
pub fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Entity => "box",
        Kind::Event => "zap",
        Kind::Observation => "eye",
        Kind::Annotation => "sticky-note",
        Kind::External => "file-symlink",
    }
}

/// How a type is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// The icon's name, and where it is among the assets.
    pub icon: String,
    pub path: SharedString,
    /// The colour's name and the colour.
    pub colour_name: String,
    pub colour: Hsla,
    /// The type itself says how it looks (not only its namespace, or nothing at all).
    pub own: bool,
}

/// How `type_` is drawn: as the vocabulary says, and where it says nothing -- or names an
/// icon or a colour there is not -- as its kind is.
pub fn look(vocabulary: &Vocabulary, type_: &str, kind: Kind) -> Look {
    let said = vocabulary.look(type_);
    let icon = said
        .icon
        .filter(|i| has_icon(i))
        .unwrap_or_else(|| kind_icon(kind).to_string());
    let (colour_name, colour) = said
        .colour
        .and_then(|name| colour(&name).map(|c| (name, c)))
        .unwrap_or_else(|| (NEUTRAL.to_string(), colour(NEUTRAL).unwrap_or_default()));
    Look {
        path: icon_path(&icon),
        icon,
        colour_name,
        colour,
        own: said.own,
    }
}

/// Icons worth offering before anything is typed: the ones an investigation reaches for.
pub const SUGGESTED: [&str; 48] = [
    "box",
    "zap",
    "eye",
    "sticky-note",
    "globe",
    "network",
    "server",
    "monitor",
    "smartphone",
    "hard-drive",
    "file",
    "file-code",
    "folder",
    "hash",
    "binary",
    "square-terminal",
    "cpu",
    "memory-stick",
    "user",
    "users",
    "key-round",
    "lock",
    "shield",
    "shield-alert",
    "bug",
    "biohazard",
    "skull",
    "ghost",
    "swords",
    "crosshair",
    "target",
    "flag",
    "radar",
    "siren",
    "bomb",
    "flame",
    "cloud",
    "database",
    "mail",
    "link",
    "wallet",
    "building-2",
    "map-pin",
    "clock",
    "tag",
    "puzzle",
    "package",
    "square-function",
];

/// The icons whose names have every word of `query` in them, those that begin with it
/// first; the suggested ones when nothing is typed. At most `limit`.
pub fn search(query: &str, limit: usize) -> Vec<&'static str> {
    let query = query.trim().to_lowercase().replace([' ', '_'], "-");
    if query.is_empty() {
        return SUGGESTED
            .iter()
            .copied()
            .filter(|name| has_icon(name))
            .take(limit)
            .collect();
    }
    let words: Vec<&str> = query.split('-').filter(|w| !w.is_empty()).collect();
    let mut found: Vec<&'static str> = icon_names()
        .iter()
        .map(String::as_str)
        .filter(|name| words.iter().all(|w| name.contains(w)))
        .collect();
    found.sort_by_key(|name| (!name.starts_with(&query), name.len(), *name));
    found.truncate(limit);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_db::Case;
    use reactor_db::graph::vocab;

    #[test]
    fn every_colour_the_vocabulary_names_is_drawn_and_none_is_orange_or_red() {
        let names: Vec<&str> = PALETTE.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vocab::COLOURS);
        for (name, _) in PALETTE {
            let c = colour(name).unwrap();
            // Hue as degrees: red is around 0, orange around 25.
            let hue = c.h * 360.0;
            assert!(
                c.s < 0.25 || (38.0..345.0).contains(&hue),
                "{name} is at {hue}: too near orange or red"
            );
        }
        assert!(colour("orange").is_none());
        assert!(colour(NEUTRAL).is_some());
        // On a light ground the same colour is darker.
        let blue = colour("blue").unwrap();
        assert_eq!(ink(blue, true), blue);
        assert!(ink(blue, false).l < 0.4);
    }

    #[test]
    fn the_icons_are_there_by_name_and_are_found_as_they_are_typed() {
        assert!(icon_names().len() > 1000);
        for kind in [
            Kind::Entity,
            Kind::Event,
            Kind::Observation,
            Kind::Annotation,
            Kind::External,
        ] {
            assert!(has_icon(kind_icon(kind)), "{}", kind_icon(kind));
        }
        for name in SUGGESTED {
            assert!(has_icon(name), "{name}");
        }
        assert!(!has_icon("no-such-icon"));
        assert_eq!(icon_path("globe"), "icons/globe.svg");
        assert_eq!(search("", 12).len(), 12);
        assert_eq!(search("", 12)[0], "box");
        let shields = search("shield", 8);
        assert_eq!(shields[0], "shield");
        assert!(shields.iter().all(|n| n.contains("shield")), "{shields:?}");
        // Words in any order, spaces for dashes.
        assert!(search("alert shield", 8).contains(&"shield-alert"));
        assert!(search("zzzz", 8).is_empty());
    }

    #[test]
    fn a_seeded_type_has_its_look_and_every_seeded_icon_exists() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), None).unwrap();
        let v = vocab::load(case.db()).unwrap();
        for t in v.types.values() {
            if let Some(icon) = &t.icon {
                assert!(has_icon(icon), "{}: no icon `{icon}`", t.name);
            }
        }
        for ns in v.namespaces.values() {
            assert!(has_icon(ns.icon.as_deref().unwrap()), "{}", ns.prefix);
        }
        let domain = look(&v, "net.domain", Kind::Entity);
        assert_eq!(
            (
                domain.icon.as_str(),
                domain.colour_name.as_str(),
                domain.own
            ),
            ("globe", "blue", true)
        );
        // A type nobody defined: its namespace's look; in no namespace, its kind's.
        let peering = look(&v, "net.peering", Kind::Entity);
        assert_eq!((peering.icon.as_str(), peering.own), ("network", false));
        let made_up = look(&v, "zzz.thing", Kind::Event);
        assert_eq!(
            (made_up.icon.as_str(), made_up.colour_name.as_str()),
            ("zap", "grey")
        );
        // An icon that there is not is drawn as if it were not set.
        vocab::set_look(
            case.db(),
            "zzz.thing",
            Kind::Entity,
            Some("no-such-icon"),
            Some("teal"),
        )
        .unwrap();
        let v = vocab::load(case.db()).unwrap();
        let odd = look(&v, "zzz.thing", Kind::Entity);
        assert_eq!(
            (odd.icon.as_str(), odd.colour_name.as_str(), odd.own),
            ("box", "teal", true)
        );
    }
}
