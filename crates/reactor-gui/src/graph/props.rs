//! What a property looks like
//! ([ADR-0067](../../../../docs/adr/0067-a-property-is-named-in-a-namespace-that-decides-how-it-is-drawn-and-read.md)):
//! its name's namespace gives it a shape, and the shape says how its value is written out
//! and what a click on it does. This is the drawing's half and has no widgets: the card
//! writes a property as one line of text, the inspector as parts that can be clicked.
//!
//! A value that does not suit its shape (a word under `size`) is written as it was stored and
//! marked, never changed.

use reactor_db::graph::fields::{self, Fields, Shape};
use serde_json::Value;

/// A property of an element, with how it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    /// The name as stored.
    pub name: String,
    /// The part after the namespace, humanised.
    pub label: String,
    pub shape: Shape,
    /// The value as stored.
    pub value: Value,
    /// The icon of its field or namespace, a Lucide icon's name.
    pub icon: Option<String>,
}

/// What a click on a part of a value does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Go {
    /// A reference of the case (ADR-0066): open what it names.
    Reference(String),
    /// A page: the browser.
    Url(String),
    /// A file or path: the explorer, if the file is one of the case's.
    File(String),
}

/// One item of a value: a value with several items (an array) is drawn as one part each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub text: String,
    pub go: Option<Go>,
    /// The item does not suit the shape of its name; `text` is the stored value.
    pub mismatch: bool,
}

impl Prop {
    /// A property under `name` with a value, as the vocabulary draws it.
    pub fn of(fields: &Fields, name: &str, value: &Value) -> Prop {
        Prop {
            name: name.to_string(),
            label: fields.label(name),
            shape: fields.shape(name),
            value: value.clone(),
            icon: fields.icon(name).map(str::to_string),
        }
    }

    /// A property that is only text, for what has no vocabulary to ask.
    pub fn plain(name: &str, text: &str) -> Prop {
        Prop {
            name: name.to_string(),
            label: name.to_string(),
            shape: Shape::Text,
            value: Value::String(text.to_string()),
            icon: None,
        }
    }

    /// The items of the value, each written for its shape.
    pub fn parts(&self) -> Vec<Part> {
        match &self.value {
            Value::Array(items) if !items.is_empty() => {
                items.iter().map(|v| part(self.shape, v)).collect()
            }
            one => vec![part(self.shape, one)],
        }
    }

    /// Whether any item does not suit the shape.
    pub fn mismatch(&self) -> bool {
        self.parts().iter().any(|p| p.mismatch)
    }

    /// The value as one line, for a card: the items joined, a command or record with its
    /// line breaks and runs of spaces made one space.
    pub fn text(&self) -> String {
        let joined = self
            .parts()
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        match self.shape {
            Shape::Verbatim => joined.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => joined,
        }
    }

    /// The value as stored, for *Copy*: a string as it is, anything else as JSON.
    pub fn copy(&self) -> String {
        match &self.value {
            Value::String(s) => s.clone(),
            Value::Array(items) => items
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            other => other.to_string(),
        }
    }
}

fn stored(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn part(shape: Shape, v: &Value) -> Part {
    let raw = stored(v);
    let fits = |text: Option<String>| match text {
        Some(text) => Part {
            text,
            go: None,
            mismatch: false,
        },
        None => Part {
            text: raw.clone(),
            go: None,
            mismatch: true,
        },
    };
    // A JSON object is a structure whatever its name says: written compactly.
    if v.is_object() {
        return Part {
            text: raw,
            go: None,
            mismatch: false,
        };
    }
    match shape {
        Shape::Text | Shape::Verbatim | Shape::Category => Part {
            text: raw,
            go: None,
            mismatch: false,
        },
        Shape::Time => fits(short_time(&raw)),
        Shape::Duration => fits(ms_of(v).map(duration)),
        Shape::Size => fits(bytes_of(v).map(size)),
        Shape::Number => fits(
            raw.trim()
                .parse::<f64>()
                .ok()
                .map(|_| raw.trim().to_string()),
        ),
        Shape::Flag => fits(match v {
            Value::Bool(true) => Some("yes".into()),
            Value::Bool(false) => Some("no".into()),
            Value::String(s) => match s.trim().to_lowercase().as_str() {
                "true" | "yes" => Some("yes".into()),
                "false" | "no" => Some("no".into()),
                _ => None,
            },
            _ => None,
        }),
        Shape::Id => Part {
            text: middle(&raw, 20),
            go: None,
            mismatch: false,
        },
        Shape::Ref => match reactor_db::reference::parse(raw.trim()) {
            Ok(r) => Part {
                text: r.short(),
                go: Some(Go::Reference(raw.trim().to_string())),
                mismatch: false,
            },
            Err(_) => Part {
                text: raw,
                go: None,
                mismatch: true,
            },
        },
        Shape::Uri => uri(&raw),
    }
}

/// `2026-03-12T09:14:00Z` as `2026-03-12 09:14`; what is not a time is not.
fn short_time(raw: &str) -> Option<String> {
    let parsed = reactor_db::time::parse_source_time(raw)?;
    Some(parsed.utc.replace('T', " ").chars().take(16).collect())
}

fn ms_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => fields::parse_duration(s),
        _ => None,
    }
}

fn bytes_of(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f.round() as u64)),
        Value::String(s) => fields::parse_size(s),
        _ => None,
    }
}

/// The lines of a text for a numbered block: every line kept, blank ones too, a Windows line
/// end not counted, and one line for a text with none.
pub fn lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    // A text that ends with a line end has no line after it.
    if out.len() > 1 && out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// Milliseconds as `850 ms`, `4.2 s`, `3 min 5 s`, `2 h 10 min`.
pub fn duration(ms: f64) -> String {
    if ms < 1000.0 {
        return format!("{} ms", trim_zero(ms));
    }
    let s = ms / 1000.0;
    if s < 60.0 {
        return format!("{} s", trim_zero((s * 10.0).round() / 10.0));
    }
    let whole = s.round() as u64;
    let (h, m, sec) = (whole / 3600, whole % 3600 / 60, whole % 60);
    match (h, sec) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{m} min {sec} s"),
        (_, _) if m == 0 => format!("{h} h"),
        _ => format!("{h} h {m} min"),
    }
}

/// Bytes as `512 B`, `180 KiB`, `1.5 MiB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = bytes as f64;
    let mut unit = 0;
    while n >= 1024.0 && unit < UNITS.len() - 1 {
        n /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{} {}", trim_zero((n * 10.0).round() / 10.0), UNITS[unit])
    }
}

fn trim_zero(n: f64) -> String {
    let s = format!("{n:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// A long text cut in the middle, so both ends stay: `3f2a91c0…9d4e7b`.
fn middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let tail = (max - 1) / 3;
    let head = max - 1 - tail;
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

/// A URL or a path: a page opens in the browser, a file or a path in the explorer. A value
/// with no scheme is a path, and so is one that is `file:`.
fn uri(raw: &str) -> Part {
    let s = raw.trim();
    let scheme = s
        .split_once("://")
        .map(|(sc, _)| sc.to_lowercase())
        .filter(|sc| {
            sc.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        });
    match scheme.as_deref() {
        Some("http" | "https") => {
            let rest = s.split_once("://").map(|(_, r)| r).unwrap_or(s);
            Part {
                text: middle(rest.trim_end_matches('/'), 48),
                go: Some(Go::Url(s.to_string())),
                mismatch: false,
            }
        }
        Some("file") => {
            let path = s.trim_start_matches("file://");
            Part {
                text: middle(path, 48),
                go: Some(Go::File(path.to_string())),
                mismatch: false,
            }
        }
        // Another scheme (`ftp`, `smb`): written, but there is no program to open it.
        Some(_) => Part {
            text: middle(s, 48),
            go: None,
            mismatch: false,
        },
        None => Part {
            text: middle(s, 48),
            go: Some(Go::File(s.to_string())),
            mismatch: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_db::graph::fields::FieldNamespace;
    use serde_json::json;

    fn vocabulary() -> Fields {
        let mut f = Fields::default();
        for shape in Shape::ALL {
            f.namespaces.insert(
                shape.as_str().into(),
                FieldNamespace {
                    prefix: shape.as_str().into(),
                    shape,
                    definition: String::new(),
                    icon: Some(shape.as_str().into()),
                    colour: None,
                },
            );
        }
        f
    }

    fn drawn(name: &str, value: Value) -> String {
        Prop::of(&vocabulary(), name, &value).text()
    }

    #[test]
    fn a_namespace_decides_how_the_value_is_written() {
        assert_eq!(drawn("size", json!(184_320)), "180 KiB");
        assert_eq!(drawn("size.unpacked", json!("1.5MiB")), "1.5 MiB");
        assert_eq!(drawn("size", json!(512)), "512 B");
        assert_eq!(drawn("duration", json!(4200)), "4.2 s");
        assert_eq!(drawn("duration", json!(185_000)), "3 min 5 s");
        assert_eq!(drawn("duration", json!(850)), "850 ms");
        assert_eq!(
            drawn("time.first_seen", json!("2026-03-12T09:14:33Z")),
            "2026-03-12 09:14"
        );
        assert_eq!(drawn("flag.hidden", json!(true)), "yes");
        assert_eq!(drawn("flag.hidden", json!("false")), "no");
        assert_eq!(drawn("number.port", json!(8080)), "8080");
        // A name with no namespace is text, whatever the value.
        assert_eq!(drawn("size_in_bytes", json!(184_320)), "184320");
        assert_eq!(
            drawn("loose", json!("2026-03-12T09:14:33Z")),
            "2026-03-12T09:14:33Z"
        );
    }

    #[test]
    fn many_values_are_many_parts_and_a_digest_keeps_both_ends() {
        let p = Prop::of(
            &vocabulary(),
            "category.technique",
            &json!(["T1059.001", "T1105"]),
        );
        assert_eq!(p.parts().len(), 2);
        assert_eq!(p.text(), "T1059.001, T1105");
        let digest = "3f2a91c0d4e7b81a5c6d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
        let short = drawn("id.imphash", json!(digest));
        assert!(
            short.chars().count() <= 20
                && short.starts_with("3f2a91c0")
                && short.ends_with("b3c4d")
        );
        assert_eq!(drawn("id.pid", json!(4120)), "4120");
        // A command is one line on a card; the stored value is untouched.
        let cmd = Prop::of(
            &vocabulary(),
            "verbatim.command_line",
            &json!("a.exe  -x\n   -y"),
        );
        assert_eq!(cmd.text(), "a.exe -x -y");
        assert_eq!(cmd.copy(), "a.exe  -x\n   -y");
    }

    #[test]
    fn a_value_that_does_not_suit_its_name_is_shown_as_stored_and_marked() {
        let p = Prop::of(&vocabulary(), "size", &json!("big"));
        assert_eq!((p.text().as_str(), p.mismatch()), ("big", true));
        let p = Prop::of(&vocabulary(), "time.first_seen", &json!("last Tuesday"));
        assert!(p.mismatch());
        let p = Prop::of(&vocabulary(), "ref.see", &json!("not a reference"));
        assert!(p.mismatch() && p.parts()[0].go.is_none());
        // A structure is written compactly whatever its name says, and is not a mismatch.
        let p = Prop::of(&vocabulary(), "size", &json!({"a": 1}));
        assert_eq!((p.text().as_str(), p.mismatch()), ("{\"a\":1}", false));
    }

    #[test]
    fn a_reference_a_url_and_a_path_can_be_followed() {
        let r = Prop::of(
            &vocabulary(),
            "ref.see",
            &json!(["graph:ev_1", "artifact:data/a.csv"]),
        );
        assert_eq!(
            r.parts().iter().map(|p| p.go.clone()).collect::<Vec<_>>(),
            vec![
                Some(Go::Reference("graph:ev_1".into())),
                Some(Go::Reference("artifact:data/a.csv".into()))
            ]
        );
        let u = Prop::of(
            &vocabulary(),
            "uri.download",
            &json!("https://evil.example/a/b.exe"),
        );
        assert_eq!(
            u.parts()[0].go,
            Some(Go::Url("https://evil.example/a/b.exe".into()))
        );
        assert_eq!(u.text(), "evil.example/a/b.exe");
        let path = Prop::of(&vocabulary(), "uri.cwd", &json!("C:\\Users\\user\\Desktop"));
        assert_eq!(
            path.parts()[0].go,
            Some(Go::File("C:\\Users\\user\\Desktop".into()))
        );
        let file = Prop::of(&vocabulary(), "uri.cwd", &json!("file:///etc/hosts"));
        assert_eq!(file.parts()[0].go, Some(Go::File("/etc/hosts".into())));
        let other = Prop::of(&vocabulary(), "uri.share", &json!("smb://nas/share"));
        assert_eq!(other.parts()[0].go, None);
    }

    #[test]
    fn a_numbered_block_keeps_every_line_and_the_blank_ones() {
        assert_eq!(lines("a\n\nb"), ["a", "", "b"]);
        assert_eq!(lines("a\r\nb\r\n"), ["a", "b"]);
        assert_eq!(lines("one"), ["one"]);
        assert_eq!(lines(""), [""]);
    }

    #[test]
    fn a_label_is_the_part_after_the_namespace_with_the_namespace_icon() {
        let p = Prop::of(&vocabulary(), "time.first_seen", &json!("2026-03-12"));
        assert_eq!(
            (p.label.as_str(), p.icon.as_deref()),
            ("First seen", Some("time"))
        );
        assert_eq!(Prop::of(&vocabulary(), "size", &json!(1)).label, "Size");
        assert_eq!(Prop::plain("asn", "AS64500").text(), "AS64500");
    }
}
