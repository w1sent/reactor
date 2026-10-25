//! Keys to bytes: what a terminal sends the program for a key.
//!
//! Pure, and written from the xterm control-sequences table. A key that is only a character is
//! sent as the text the platform made of it; a key with a name (an arrow, a function key) is a
//! sequence that depends on the modifiers and on the modes the program has set.

use std::borrow::Cow;

use gpui_kit::Keystroke;

use super::types::Modes;

/// Whether Alt sends Escape before the key. Off macOS it always does; on macOS Option types
/// characters (`ß` for Option-s) unless the person asked for it to act as Alt.
pub fn alt_is_meta(option_as_alt: bool) -> bool {
    !cfg!(target_os = "macos") || option_as_alt
}

/// `1 + shift 1 | alt 2 | ctrl 4 | meta 8`, the number that follows the `;` of a modified key.
fn modifier_param(k: &Keystroke) -> u8 {
    let m = k.modifiers;
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control) + 8 * u8::from(m.platform)
}

fn modified(k: &Keystroke) -> bool {
    modifier_param(k) > 1
}

fn owned(bytes: impl Into<Vec<u8>>) -> Option<Cow<'static, [u8]>> {
    Some(Cow::Owned(bytes.into()))
}

fn fixed(bytes: &'static [u8]) -> Option<Cow<'static, [u8]>> {
    Some(Cow::Borrowed(bytes))
}

/// `ESC [ <final>`, or `ESC O <final>` in application mode, or `ESC [ 1 ; m <final>` modified.
fn cursor_key(k: &Keystroke, final_byte: u8, app: bool) -> Option<Cow<'static, [u8]>> {
    if modified(k) {
        owned(format!(
            "\x1b[1;{}{}",
            modifier_param(k),
            final_byte as char
        ))
    } else if app {
        owned(vec![0x1b, b'O', final_byte])
    } else {
        owned(vec![0x1b, b'[', final_byte])
    }
}

/// `ESC [ n ~`, or `ESC [ n ; m ~` modified.
fn tilde_key(k: &Keystroke, n: u8) -> Option<Cow<'static, [u8]>> {
    if modified(k) {
        owned(format!("\x1b[{};{}~", n, modifier_param(k)))
    } else {
        owned(format!("\x1b[{n}~"))
    }
}

/// F1 to F4 are `ESC O P` to `S`; modified they become `ESC [ 1 ; m P`.
fn ss3_key(k: &Keystroke, final_byte: u8) -> Option<Cow<'static, [u8]>> {
    if modified(k) {
        owned(format!(
            "\x1b[1;{}{}",
            modifier_param(k),
            final_byte as char
        ))
    } else {
        owned(vec![0x1b, b'O', final_byte])
    }
}

fn function_key(k: &Keystroke, n: u8) -> Option<Cow<'static, [u8]>> {
    match n {
        1 => ss3_key(k, b'P'),
        2 => ss3_key(k, b'Q'),
        3 => ss3_key(k, b'R'),
        4 => ss3_key(k, b'S'),
        5 => tilde_key(k, 15),
        6 => tilde_key(k, 17),
        7 => tilde_key(k, 18),
        8 => tilde_key(k, 19),
        9 => tilde_key(k, 20),
        10 => tilde_key(k, 21),
        11 => tilde_key(k, 23),
        12 => tilde_key(k, 24),
        13 => tilde_key(k, 25),
        14 => tilde_key(k, 26),
        15 => tilde_key(k, 28),
        16 => tilde_key(k, 29),
        17 => tilde_key(k, 31),
        18 => tilde_key(k, 32),
        19 => tilde_key(k, 33),
        20 => tilde_key(k, 34),
        _ => None,
    }
}

/// The control character a key makes with Ctrl held, if it makes one.
fn control_byte(key: &str, shift: bool) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return match key {
            "space" => Some(0x00),
            _ => None,
        };
    }
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '-' if shift => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// The bytes for a key, or `None` when it is not for the program (a shortcut of the app, a bare
/// modifier, a key with no meaning in a terminal).
pub fn encode(k: &Keystroke, modes: Modes, alt_as_meta: bool) -> Option<Cow<'static, [u8]>> {
    let m = k.modifiers;
    let key = k.key.as_str();

    // Keys with a name.
    match key {
        "enter" => return with_escape(k, alt_as_meta, fixed(b"\r")),
        "tab" if m.shift => return fixed(b"\x1b[Z"),
        "tab" => return with_escape(k, alt_as_meta, fixed(b"\t")),
        "escape" => return with_escape(k, alt_as_meta, fixed(b"\x1b")),
        "backspace" => {
            let byte: &'static [u8] = if m.control { b"\x08" } else { b"\x7f" };
            return with_escape(k, alt_as_meta, fixed(byte));
        }
        "up" => return cursor_key(k, b'A', modes.app_cursor),
        "down" => return cursor_key(k, b'B', modes.app_cursor),
        "right" => return cursor_key(k, b'C', modes.app_cursor),
        "left" => return cursor_key(k, b'D', modes.app_cursor),
        "home" => return cursor_key(k, b'H', modes.app_cursor),
        "end" => return cursor_key(k, b'F', modes.app_cursor),
        "insert" => return tilde_key(k, 2),
        "delete" => return tilde_key(k, 3),
        "pageup" => return tilde_key(k, 5),
        "pagedown" => return tilde_key(k, 6),
        _ => {}
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return function_key(k, n);
    }

    // The command key is for the app's own shortcuts.
    if m.platform {
        return None;
    }

    // Ctrl with a letter or one of the punctuation keys.
    if m.control {
        let byte = control_byte(key, m.shift)?;
        return with_escape(k, alt_as_meta, owned(vec![byte]));
    }

    // Text. Alt types the character on a keyboard where Option does, so what the platform made
    // of the key is used unless Alt is Escape.
    let text = if m.alt && alt_as_meta {
        base_char(k)?
    } else if key == "space" {
        " ".to_owned()
    } else {
        match &k.key_char {
            Some(text) if !text.is_empty() => text.clone(),
            _ => base_char(k)?,
        }
    };
    if m.alt && alt_as_meta {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(text.as_bytes());
        return owned(bytes);
    }
    owned(text.into_bytes())
}

/// The character of a key as it is on the keyboard: Alt-s is `s` for a terminal, not `ß`.
fn base_char(k: &Keystroke) -> Option<String> {
    if k.key == "space" {
        return Some(" ".to_owned());
    }
    let mut chars = k.key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(if k.modifiers.shift {
        c.to_uppercase().collect()
    } else {
        c.to_string()
    })
}

/// Escape before a key when Alt is held and means it.
fn with_escape(
    k: &Keystroke,
    alt_as_meta: bool,
    bytes: Option<Cow<'static, [u8]>>,
) -> Option<Cow<'static, [u8]>> {
    let bytes = bytes?;
    if k.modifiers.alt && alt_as_meta {
        let mut out = vec![0x1b];
        out.extend_from_slice(&bytes);
        owned(out)
    } else {
        Some(bytes)
    }
}

/// What pasting `text` sends: wrapped when the program asked for it, with Escape taken out so the
/// text cannot end the wrapper itself; otherwise line ends become what Enter sends.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.bytes().filter(|&b| b != 0x1b));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// What a change of focus sends to a program that asked to be told.
pub fn focus(gained: bool) -> &'static [u8] {
    if gained { b"\x1b[I" } else { b"\x1b[O" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    fn enc(s: &str) -> Option<Vec<u8>> {
        encode(&key(s), Modes::default(), true).map(Cow::into_owned)
    }

    fn enc_app(s: &str) -> Option<Vec<u8>> {
        let modes = Modes {
            app_cursor: true,
            ..Modes::default()
        };
        encode(&key(s), modes, true).map(Cow::into_owned)
    }

    fn b(s: &str) -> Option<Vec<u8>> {
        Some(s.as_bytes().to_vec())
    }

    #[test]
    fn text_is_what_the_platform_typed() {
        assert_eq!(enc("a->a"), b("a"));
        assert_eq!(enc("shift-a->A"), b("A"));
        assert_eq!(enc("shift-1->!"), b("!"));
        assert_eq!(enc("space"), b(" "));
        assert_eq!(enc("shift-space"), b(" "));
        assert_eq!(enc("ä->ä"), Some("ä".as_bytes().to_vec()));
        assert_eq!(enc("日->日"), Some("日".as_bytes().to_vec()));
    }

    #[test]
    fn alt_sends_escape_before_the_key() {
        assert_eq!(enc("alt-a->a"), b("\x1ba"));
        assert_eq!(enc("alt-shift-a->A"), b("\x1bA"));
        assert_eq!(enc("alt-space"), b("\x1b "));
        assert_eq!(enc("alt-enter"), b("\x1b\r"));
        assert_eq!(enc("alt-escape"), b("\x1b\x1b"));
    }

    #[test]
    fn option_types_characters_unless_it_is_alt() {
        // `ß` is what Option-s types on a Mac; as Alt it is Escape then `s`.
        let k = key("alt-s->ß");
        assert_eq!(
            encode(&k, Modes::default(), false).map(Cow::into_owned),
            Some("ß".as_bytes().to_vec())
        );
        assert_eq!(
            encode(&k, Modes::default(), true).map(Cow::into_owned),
            b("\x1bs")
        );
        assert_eq!(alt_is_meta(false), !cfg!(target_os = "macos"));
        assert!(alt_is_meta(true));
    }

    #[test]
    fn ctrl_letters_are_the_control_characters() {
        for (i, c) in ('a'..='z').enumerate() {
            assert_eq!(
                enc(&format!("ctrl-{c}")),
                Some(vec![i as u8 + 1]),
                "ctrl-{c}"
            );
        }
        assert_eq!(enc("ctrl-a->a"), Some(vec![1]));
        assert_eq!(enc("ctrl-shift-a->A"), Some(vec![1]));
        assert_eq!(enc("ctrl-space"), Some(vec![0]));
        assert_eq!(enc("ctrl-@"), Some(vec![0]));
        assert_eq!(enc("ctrl-["), Some(vec![0x1b]));
        assert_eq!(enc("ctrl-\\"), Some(vec![0x1c]));
        assert_eq!(enc("ctrl-]"), Some(vec![0x1d]));
        assert_eq!(enc("ctrl-^"), Some(vec![0x1e]));
        assert_eq!(enc("ctrl-_"), Some(vec![0x1f]));
        assert_eq!(enc("ctrl-/"), Some(vec![0x1f]));
        assert_eq!(enc("ctrl-?"), Some(vec![0x7f]));
        assert_eq!(enc("ctrl-alt-a"), Some(vec![0x1b, 1]));
        // Nothing for a chord that is not a control character.
        assert_eq!(enc("ctrl-,"), None);
    }

    #[test]
    fn the_editing_keys() {
        assert_eq!(enc("enter"), b("\r"));
        assert_eq!(enc("shift-enter"), b("\r"));
        assert_eq!(enc("tab"), b("\t"));
        assert_eq!(enc("shift-tab"), b("\x1b[Z"));
        assert_eq!(enc("escape"), b("\x1b"));
        assert_eq!(enc("backspace"), Some(vec![0x7f]));
        assert_eq!(enc("ctrl-backspace"), Some(vec![0x08]));
        assert_eq!(enc("alt-backspace"), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn arrows_home_and_end_follow_the_cursor_key_mode() {
        for (name, f) in [
            ("up", 'A'),
            ("down", 'B'),
            ("right", 'C'),
            ("left", 'D'),
            ("home", 'H'),
            ("end", 'F'),
        ] {
            assert_eq!(enc(name), b(&format!("\x1b[{f}")), "{name}");
            assert_eq!(
                enc_app(name),
                b(&format!("\x1bO{f}")),
                "{name} (application)"
            );
        }
    }

    #[test]
    fn a_modified_arrow_names_its_modifiers_in_either_mode() {
        // 1 + shift 1 | alt 2 | ctrl 4 | meta 8
        assert_eq!(enc("shift-up"), b("\x1b[1;2A"));
        assert_eq!(enc("alt-up"), b("\x1b[1;3A"));
        assert_eq!(enc("ctrl-up"), b("\x1b[1;5A"));
        assert_eq!(enc("ctrl-shift-up"), b("\x1b[1;6A"));
        assert_eq!(enc("ctrl-alt-left"), b("\x1b[1;7D"));
        assert_eq!(enc("cmd-right"), b("\x1b[1;9C"));
        assert_eq!(enc_app("ctrl-right"), b("\x1b[1;5C"));
        assert_eq!(enc("ctrl-home"), b("\x1b[1;5H"));
        assert_eq!(enc("shift-end"), b("\x1b[1;2F"));
    }

    #[test]
    fn insert_delete_and_the_pages() {
        for (name, n) in [("insert", 2), ("delete", 3), ("pageup", 5), ("pagedown", 6)] {
            assert_eq!(enc(name), b(&format!("\x1b[{n}~")), "{name}");
            assert_eq!(
                enc(&format!("ctrl-{name}")),
                b(&format!("\x1b[{n};5~")),
                "ctrl-{name}"
            );
            assert_eq!(
                enc(&format!("shift-{name}")),
                b(&format!("\x1b[{n};2~")),
                "shift-{name}"
            );
            assert_eq!(
                enc_app(name),
                b(&format!("\x1b[{n}~")),
                "{name} ignores application mode"
            );
        }
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc("f1"), b("\x1bOP"));
        assert_eq!(enc("f2"), b("\x1bOQ"));
        assert_eq!(enc("f3"), b("\x1bOR"));
        assert_eq!(enc("f4"), b("\x1bOS"));
        let tilde = [
            (5, 15),
            (6, 17),
            (7, 18),
            (8, 19),
            (9, 20),
            (10, 21),
            (11, 23),
            (12, 24),
        ];
        for (f, n) in tilde {
            assert_eq!(enc(&format!("f{f}")), b(&format!("\x1b[{n}~")), "f{f}");
            assert_eq!(
                enc(&format!("shift-f{f}")),
                b(&format!("\x1b[{n};2~")),
                "shift-f{f}"
            );
            assert_eq!(
                enc(&format!("ctrl-f{f}")),
                b(&format!("\x1b[{n};5~")),
                "ctrl-f{f}"
            );
        }
        assert_eq!(enc("shift-f1"), b("\x1b[1;2P"));
        assert_eq!(enc("ctrl-alt-f4"), b("\x1b[1;7S"));
        assert_eq!(enc("f13"), b("\x1b[25~"));
        assert_eq!(enc("f20"), b("\x1b[34~"));
        assert_eq!(enc("f21"), None);
        assert_eq!(enc_app("f1"), b("\x1bOP"));
    }

    #[test]
    fn the_command_key_and_bare_modifiers_are_not_for_the_program() {
        assert_eq!(enc("cmd-c->c"), None);
        assert_eq!(enc("cmd-v"), None);
        for bare in ["shift", "control", "alt", "capslock"] {
            let k = Keystroke {
                key: bare.into(),
                ..Keystroke::default()
            };
            assert_eq!(encode(&k, Modes::default(), true), None, "{bare}");
        }
    }

    #[test]
    fn paste_is_wrapped_when_asked_and_cannot_close_its_own_wrapper() {
        assert_eq!(paste("ls -l", true), b"\x1b[200~ls -l\x1b[201~");
        assert_eq!(
            paste("a\nb\x1b[201~c", true),
            b"\x1b[200~a\nb[201~c\x1b[201~"
        );
        assert_eq!(paste("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste("日本", false), "日本".as_bytes());
    }

    #[test]
    fn focus_reports() {
        assert_eq!(focus(true), b"\x1b[I");
        assert_eq!(focus(false), b"\x1b[O");
    }
}
