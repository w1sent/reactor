//! The window's keyboard shortcuts, and the person's own choice of them.
//!
//! A [`Shortcut`] is a command reachable from anywhere in the window — a panel, the palette,
//! the settings — with the keys it answers to by default. What the person chose instead is
//! kept in `~/.reactor/gui.json` (`ui.shortcuts`, by [`Shortcut::id`]); an empty choice
//! means no keys at all. The Shortcuts page of the settings window edits it.
//!
//! The bindings are installed apart from every other: tagged with [`META`], so a change
//! takes out exactly these and puts the new ones in, and the keys bound by the components
//! (the inputs, the dock) and by single panels (the graph, the editors) are left alone.
//! Those are about where the focus is, not about the window, and are not configurable here.

use std::collections::BTreeMap;

use gpui_kit::{Action, App, KeyBinding, KeyBindingMetaIndex, Keystroke};

use crate::app::{CloseTab, NewConsole, OpenPalette, OpenSettings, TogglePanelAction};
use crate::layout::PanelKind;

/// Marks the bindings installed from here.
pub const META: KeyBindingMetaIndex = KeyBindingMetaIndex(0x5245_4b31);

/// One command and the keys it has by default (`secondary` is Cmd on macOS, Ctrl elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    /// What `gui.json` stores the choice under.
    pub id: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    command: Command,
    /// Stays active while a terminal has the focus. Most do not: a shell needs `ctrl-p`,
    /// `ctrl-w` and the rest (docs/adr/0054).
    pub in_terminal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Panel(PanelKind),
    NewConsole,
    Palette,
    Settings,
    CloseTab,
}

impl Shortcut {
    const fn in_terminal(mut self) -> Self {
        self.in_terminal = true;
        self
    }
}

const fn panel(
    id: &'static str,
    label: &'static str,
    default: &'static str,
    kind: PanelKind,
) -> Shortcut {
    Shortcut {
        id,
        label,
        default,
        command: Command::Panel(kind),
        in_terminal: false,
    }
}

/// Every configurable shortcut, in the order the settings list them: the panels as the
/// Panels menu has them, then the commands.
pub const ALL: [Shortcut; 12] = [
    panel("panel.tree", "Session Tree", "secondary-1", PanelKind::Tree),
    panel(
        "panel.toolbox",
        "Toolbox",
        "secondary-2",
        PanelKind::Toolbox,
    ),
    panel(
        "panel.services",
        "Services",
        "secondary-3",
        PanelKind::Services,
    ),
    panel("panel.files", "Files", "secondary-4", PanelKind::Files),
    panel(
        "panel.transcript",
        "Transcript",
        "secondary-shift-t",
        PanelKind::Transcript,
    ),
    // The console's own two stay: they are how a terminal is reached from a terminal.
    panel("panel.console", "Console", "ctrl-`", PanelKind::Console).in_terminal(),
    panel(
        "panel.graph",
        "Graph",
        "secondary-shift-g",
        PanelKind::Graph,
    ),
    panel(
        "panel.docs",
        "Documents",
        "secondary-shift-d",
        PanelKind::Docs,
    ),
    Shortcut {
        id: "console.new",
        label: "New Console",
        default: "ctrl-~",
        command: Command::NewConsole,
        in_terminal: true,
    },
    Shortcut {
        id: "palette",
        label: "Command Palette",
        default: "secondary-p",
        command: Command::Palette,
        in_terminal: false,
    },
    Shortcut {
        id: "settings",
        label: "Settings",
        default: "secondary-,",
        command: Command::Settings,
        in_terminal: false,
    },
    Shortcut {
        id: "tab.close",
        label: "Close Tab",
        default: "secondary-w",
        command: Command::CloseTab,
        in_terminal: false,
    },
];

impl Shortcut {
    pub fn find(id: &str) -> Option<Shortcut> {
        ALL.into_iter().find(|s| s.id == id)
    }

    fn action(self) -> Box<dyn Action> {
        match self.command {
            Command::Panel(panel) => Box::new(TogglePanelAction { panel }),
            Command::NewConsole => Box::new(NewConsole),
            Command::Palette => Box::new(OpenPalette),
            Command::Settings => Box::new(OpenSettings),
            Command::CloseTab => Box::new(CloseTab),
        }
    }

    /// The keys in force: the person's choice, else the default. Empty is none.
    pub fn keys(self, chosen: &BTreeMap<String, String>) -> &str {
        chosen
            .get(self.id)
            .map(String::as_str)
            .unwrap_or(self.default)
    }
}

/// The keys a keystroke may name besides a single character.
const NAMED: [&str; 17] = [
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "menu",
    "-",
];

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// Whether `keys` is something a window-wide binding can be made of: keystrokes such as
/// `ctrl-shift-k`, separated by spaces for a sequence, or nothing at all. The first keystroke
/// needs Ctrl, Alt, Cmd or Fn, or must be a function key: a bare `k` or `shift-k` would take
/// that letter from every text field. Returns the keys with the spacing tidied.
pub fn parse(keys: &str) -> Result<String, String> {
    let parts: Vec<&str> = keys.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        let keystroke =
            Keystroke::parse(part).map_err(|_| format!("`{part}` is not a keystroke"))?;
        let key = keystroke.key.as_str();
        if !(key.chars().count() == 1 || NAMED.contains(&key) || is_function_key(key)) {
            return Err(format!("`{part}` does not end in a key"));
        }
        let m = keystroke.modifiers;
        if i == 0 && !(m.control || m.alt || m.platform || m.function || is_function_key(key)) {
            return Err(format!(
                "`{part}` needs ctrl, alt, cmd or fn, or a function key, or it would take the key from text fields"
            ));
        }
    }
    Ok(parts.join(" "))
}

/// Put `chosen` in order after loading a hand-edited file: unknown ids, choices that do not
/// parse and choices equal to the default are dropped.
pub fn normalize(chosen: &mut BTreeMap<String, String>) {
    let mut tidy = BTreeMap::new();
    for (id, keys) in std::mem::take(chosen) {
        let Some(shortcut) = Shortcut::find(&id) else {
            continue;
        };
        if let Ok(keys) = parse(&keys)
            && keys != shortcut.default
        {
            tidy.insert(id, keys);
        }
    }
    *chosen = tidy;
}

/// The keystrokes `keys` stands for on this platform: `secondary-1` and `ctrl-1` are the same
/// keys on Linux. `None` when they do not parse or are empty.
fn pressed(keys: &str) -> Option<Vec<(gpui_kit::Modifiers, String)>> {
    let strokes = keys
        .split_whitespace()
        .map(|part| Keystroke::parse(part).ok().map(|k| (k.modifiers, k.key)))
        .collect::<Option<Vec<_>>>()?;
    (!strokes.is_empty()).then_some(strokes)
}

/// The other shortcuts that answer to the same keys as `shortcut`.
pub fn clashes(shortcut: Shortcut, chosen: &BTreeMap<String, String>) -> Vec<Shortcut> {
    let Some(keys) = pressed(shortcut.keys(chosen)) else {
        return Vec::new();
    };
    ALL.into_iter()
        .filter(|other| {
            other.id != shortcut.id && pressed(other.keys(chosen)).as_ref() == Some(&keys)
        })
        .collect()
}

/// Whether `shortcut`, on the keys it has, is still active while a terminal has the focus: the
/// ones that are flagged, and anything on the platform key (Cmd on macOS), which no program in a
/// terminal is sent.
pub fn stays_in_terminal(shortcut: Shortcut, chosen: &BTreeMap<String, String>) -> bool {
    shortcut.in_terminal
        || pressed(shortcut.keys(chosen))
            .and_then(|strokes| strokes.first().map(|(m, _)| m.platform))
            .unwrap_or(false)
}

/// Whether the shortcut has keys that a focused terminal takes from it, for the settings page to say so.
pub fn shadowed_in_terminal(shortcut: Shortcut, chosen: &BTreeMap<String, String>) -> bool {
    pressed(shortcut.keys(chosen)).is_some() && !stays_in_terminal(shortcut, chosen)
}

/// The key context a shadowed shortcut is bound in: everywhere except a terminal.
const OUTSIDE_TERMINAL: &str = "!Terminal";

/// The bindings for `chosen`.
pub fn bindings(chosen: &BTreeMap<String, String>) -> Vec<KeyBinding> {
    ALL.into_iter()
        .filter_map(|shortcut| {
            let keys = parse(shortcut.keys(chosen)).ok()?;
            if keys.is_empty() {
                return None;
            }
            let context = (!stays_in_terminal(shortcut, chosen)).then(|| {
                std::rc::Rc::new(
                    gpui_kit::KeyBindingContextPredicate::parse(OUTSIDE_TERMINAL)
                        .expect("a valid key context"),
                )
            });
            KeyBinding::load(
                &keys,
                shortcut.action(),
                context,
                false,
                None,
                &gpui_kit::DummyKeyboardMapper,
            )
            .ok()
            .map(|b| b.with_meta(META))
        })
        .collect()
}

/// Install the bindings for `chosen`, replacing the ones installed before.
pub fn install(chosen: &BTreeMap<String, String>, cx: &mut App) {
    let kept: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|b| b.meta() != Some(META))
        .cloned()
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(kept);
    cx.bind_keys(bindings(chosen));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_has_its_own_id_and_a_default_that_parses() {
        let mut ids: Vec<&str> = ALL.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
        for s in ALL {
            assert_eq!(parse(s.default).as_deref(), Ok(s.default), "{}", s.id);
            assert!(clashes(s, &BTreeMap::new()).is_empty(), "{} clashes", s.id);
        }
        // Every panel can be reached from the keyboard.
        for kind in PanelKind::ALL {
            assert!(
                ALL.iter().any(|s| s.command == Command::Panel(kind)),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn keys_are_checked_and_tidied() {
        assert_eq!(parse("  ctrl-k   s ").as_deref(), Ok("ctrl-k s"));
        assert_eq!(parse("").as_deref(), Ok(""));
        assert_eq!(parse("f12").as_deref(), Ok("f12"));
        assert_eq!(parse("alt-pageup").as_deref(), Ok("alt-pageup"));
        // Half typed, or a key a text field needs.
        for bad in ["ctrl", "ctrl-", "c", "shift-k", "ctrl-sh", "f99"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_choice_overrides_and_an_empty_one_unbinds() {
        let mut chosen = BTreeMap::new();
        chosen.insert("panel.console".to_string(), "f12".to_string());
        chosen.insert("palette".to_string(), String::new());
        let bound = bindings(&chosen);
        assert_eq!(bound.len(), ALL.len() - 1, "the palette has no keys");
        assert!(bound.iter().all(|b| b.meta() == Some(META)));
        let console = Shortcut::find("panel.console").unwrap();
        assert_eq!(console.keys(&chosen), "f12");
    }

    #[test]
    fn loading_drops_what_cannot_be_used() {
        let mut chosen = BTreeMap::new();
        chosen.insert("nonsense".to_string(), "f1".to_string());
        chosen.insert("panel.graph".to_string(), "secondary-shift-g".to_string());
        chosen.insert("panel.files".to_string(), "ctrl-".to_string());
        chosen.insert("tab.close".to_string(), "w".to_string());
        chosen.insert("settings".to_string(), "f2".to_string());
        normalize(&mut chosen);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen.get("settings").map(String::as_str), Some("f2"));
    }

    /// What the keymap would run for `keys` with the focus in a terminal, or elsewhere.
    fn action_for(
        chosen: &BTreeMap<String, String>,
        keys: &str,
        in_terminal: bool,
    ) -> Option<String> {
        use gpui_kit::{KeyContext, Keymap};
        let keymap = Keymap::new(bindings(chosen));
        let stroke = vec![Keystroke::parse(keys).unwrap()];
        let stack = if in_terminal {
            vec![
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Terminal").unwrap(),
            ]
        } else {
            vec![KeyContext::parse("Workspace").unwrap()]
        };
        let (found, _) = keymap.bindings_for_input(&stroke, &stack);
        found.first().map(|b| b.action().name().to_owned())
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_terminal_keeps_the_keys_a_shell_needs() {
        let none = BTreeMap::new();
        // Ctrl-P is readline's previous line and Ctrl-W kills a word: only outside a terminal
        // are they the palette and closing a tab.
        assert!(action_for(&none, "ctrl-p", false).is_some());
        assert_eq!(action_for(&none, "ctrl-p", true), None);
        assert!(action_for(&none, "ctrl-w", false).is_some());
        assert_eq!(action_for(&none, "ctrl-w", true), None);
        assert_eq!(action_for(&none, "ctrl-1", true), None);
        assert!(action_for(&none, "ctrl-1", false).is_some());
        // The console's own two stay.
        assert!(action_for(&none, "ctrl-`", true).is_some());
        assert!(action_for(&none, "ctrl-~", true).is_some());
        assert!(action_for(&none, "ctrl-`", false).is_some());
    }

    #[test]
    fn what_stays_in_a_terminal_is_the_flagged_and_the_platform_key() {
        let none = BTreeMap::new();
        let console = Shortcut::find("panel.console").unwrap();
        let palette = Shortcut::find("palette").unwrap();
        assert!(stays_in_terminal(console, &none));
        assert!(Shortcut::find("console.new").unwrap().in_terminal);
        assert!(!shadowed_in_terminal(console, &none));
        // On Cmd (macOS) nothing collides; on Ctrl the palette is shadowed.
        assert_eq!(stays_in_terminal(palette, &none), cfg!(target_os = "macos"));
        let mut chosen = BTreeMap::new();
        chosen.insert("palette".to_string(), "cmd-k".to_string());
        assert!(stays_in_terminal(palette, &chosen));
        chosen.insert("palette".to_string(), "alt-k".to_string());
        assert!(shadowed_in_terminal(palette, &chosen));
        // No keys, nothing to shadow.
        chosen.insert("palette".to_string(), String::new());
        assert!(!shadowed_in_terminal(palette, &chosen));
        let flagged: Vec<&str> = ALL.iter().filter(|s| s.in_terminal).map(|s| s.id).collect();
        assert_eq!(
            flagged,
            ["panel.console", "console.new"],
            "the allowlist is these two"
        );
    }

    #[test]
    fn two_shortcuts_on_the_same_keys_are_found() {
        let mut chosen = BTreeMap::new();
        chosen.insert("panel.graph".to_string(), "secondary-1".to_string());
        let tree = Shortcut::find("panel.tree").unwrap();
        assert_eq!(
            clashes(tree, &chosen),
            vec![Shortcut::find("panel.graph").unwrap()]
        );
        // The same keys written another way: `secondary` is the platform's own modifier.
        let platform = if cfg!(target_os = "macos") {
            "cmd-1"
        } else {
            "ctrl-1"
        };
        chosen.insert("panel.graph".to_string(), platform.to_string());
        assert_eq!(
            clashes(tree, &chosen),
            vec![Shortcut::find("panel.graph").unwrap()]
        );
        chosen.insert("panel.graph".to_string(), String::new());
        assert!(
            clashes(tree, &chosen).is_empty(),
            "no keys clash with nothing"
        );
    }
}
