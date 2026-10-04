//! Keyed archives: what `NSKeyedArchiver` writes, read back into the objects it was made of.
//!
//! An archive is a property list with a fixed shape: `$archiver` says what wrote it, `$top`
//! names the roots, and `$objects` is a table in which every object is kept once and the rest
//! refer to it by position (a UID). Each object is a plain value (a string, a number) or a
//! dictionary with a `$class` that points at the class's own entry (`$classname`, and the
//! chain of superclasses). Collections, strings, dates and the like are stored as their
//! class's *fields* — `NS.objects`, `NS.keys`, `NS.string`, `NS.time` — and reading means
//! putting those back together as the array, dictionary, string and date they were.
//!
//! What is made is the same [`Value`] tree the viewer shows for any property list, so the
//! tree, the copy buttons and the XML view all work on it. A class that is not one of
//! Foundation's keeps its name in `$class` and its fields, resolved. Objects that refer to
//! each other are shown once and then as a pointer, so a cycle does not unfold forever.

use super::plist::Value;

/// Most nodes the resolved tree may have: an archive shares objects, and a hostile one can
/// share them so that the tree is far larger than the file.
const MAX_NODES: usize = 1_000_000;

fn field<'a>(entries: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The position a UID value points at: a UID itself, or the dictionary an XML plist writes
/// one as (`{CF$UID: n}`).
fn uid_of(v: &Value) -> Option<usize> {
    match v {
        Value::Uid(n) => usize::try_from(*n).ok(),
        Value::Dict(d) if d.len() == 1 && d[0].0 == "CF$UID" => match &d[0].1 {
            Value::Int(n) => usize::try_from(*n).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a value is a keyed archive, and if it is, what it holds.
pub fn resolve(root: &Value) -> Option<Value> {
    let Value::Dict(top_level) = root else {
        return None;
    };
    match field(top_level, "$archiver") {
        Some(Value::String(s)) if s.contains("NSKeyedArchiver") => {}
        _ => return None,
    }
    let (Some(Value::Array(objects)), Some(Value::Dict(top))) =
        (field(top_level, "$objects"), field(top_level, "$top"))
    else {
        return None;
    };
    let mut r = Resolver {
        objects,
        nodes: 0,
        walking: Vec::new(),
    };
    Some(Value::Dict(
        top.iter()
            .map(|(name, v)| (name.clone(), r.follow(v)))
            .collect(),
    ))
}

struct Resolver<'a> {
    objects: &'a [Value],
    nodes: usize,
    /// The objects being resolved, outermost first: one met again is a cycle.
    walking: Vec<usize>,
}

impl Resolver<'_> {
    /// A value that may be a reference: the object it points at, or the value itself.
    fn follow(&mut self, v: &Value) -> Value {
        match uid_of(v) {
            Some(n) => self.object(n),
            None => v.clone(),
        }
    }

    fn follow_all(&mut self, v: Option<&Value>) -> Vec<Value> {
        match v {
            Some(Value::Array(items)) | Some(Value::Set(items)) => {
                items.iter().map(|i| self.follow(i)).collect()
            }
            _ => Vec::new(),
        }
    }

    fn class_name(&self, entries: &[(String, Value)]) -> Option<String> {
        let at = uid_of(field(entries, "$class")?)?;
        match self.objects.get(at)? {
            Value::Dict(c) => match field(c, "$classname") {
                Some(Value::String(name)) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn object(&mut self, n: usize) -> Value {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Value::String("‹too large to show›".into());
        }
        let Some(object) = self.objects.get(n) else {
            return Value::String(format!("‹no object {n}›"));
        };
        if self.walking.contains(&n) {
            return Value::String(format!("‹the object {n} again›"));
        }
        let object = object.clone();
        self.walking.push(n);
        let made = match &object {
            // The archive's nil.
            Value::String(s) if s == "$null" => Value::Null,
            Value::Dict(entries) => self.instance(entries),
            other => other.clone(),
        };
        self.walking.pop();
        made
    }

    /// An object that is an instance of a class: put its fields back together.
    fn instance(&mut self, entries: &[(String, Value)]) -> Value {
        let Some(class) = self.class_name(entries) else {
            // Not an instance: a dictionary like any other, with its references followed.
            return Value::Dict(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), self.follow(v)))
                    .collect(),
            );
        };
        match class.as_str() {
            "NSArray" | "NSMutableArray" | "NSOrderedSet" | "NSMutableOrderedSet" => {
                Value::Array(self.follow_all(field(entries, "NS.objects")))
            }
            "NSSet" | "NSMutableSet" => Value::Set(self.follow_all(field(entries, "NS.objects"))),
            "NSDictionary" | "NSMutableDictionary" => {
                let keys = self.follow_all(field(entries, "NS.keys"));
                let values = self.follow_all(field(entries, "NS.objects"));
                Value::Dict(
                    keys.into_iter()
                        .zip(values)
                        .map(|(k, v)| {
                            let key = match k {
                                Value::String(s) => s,
                                other => other.summary(),
                            };
                            (key, v)
                        })
                        .collect(),
                )
            }
            "NSString" | "NSMutableString" | "NSAttributedString" | "NSMutableAttributedString" => {
                let text = field(entries, "NS.string").map(|v| self.follow(v));
                match text {
                    Some(Value::String(s)) => Value::String(s),
                    Some(other) => other,
                    None => Value::String(String::new()),
                }
            }
            "NSData" | "NSMutableData" => field(entries, "NS.data")
                .map(|v| self.follow(v))
                .unwrap_or(Value::Data(Vec::new())),
            "NSDate" => match field(entries, "NS.time") {
                Some(Value::Real(t)) => Value::Date(*t),
                Some(Value::Int(t)) => Value::Date(*t as f64),
                _ => Value::Null,
            },
            "NSURL" => {
                let relative = field(entries, "NS.relative").map(|v| self.follow(v));
                let base = field(entries, "NS.base").map(|v| self.follow(v));
                match (base, relative) {
                    (Some(Value::String(b)), Some(Value::String(r))) => {
                        Value::String(format!("{b}{r}"))
                    }
                    (_, Some(Value::String(r))) => Value::String(r),
                    _ => Value::Null,
                }
            }
            "NSUUID" => match field(entries, "NS.uuidbytes").map(|v| self.follow(v)) {
                Some(Value::Data(b)) if b.len() == 16 => {
                    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
                    Value::String(format!(
                        "{}-{}-{}-{}-{}",
                        &h[..8],
                        &h[8..12],
                        &h[12..16],
                        &h[16..20],
                        &h[20..]
                    ))
                }
                _ => Value::Null,
            },
            "NSNull" => Value::Null,
            // Anything else: the class, and its fields as they are, followed.
            _ => {
                let mut fields = vec![("$class".to_string(), Value::String(class))];
                for (k, v) in entries.iter().filter(|(k, _)| k != "$class") {
                    fields.push((k.clone(), self.follow(v)));
                }
                Value::Dict(fields)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::plist;
    use super::*;

    const ARCHIVE: &[u8] = include_bytes!("../../tests/fixtures/sample-archive.bplist");
    const NESTED: &[u8] = include_bytes!("../../tests/fixtures/sample-nested.bplist");

    fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
        match v {
            Value::Dict(d) => {
                &d.iter()
                    .find(|(k, _)| k == key)
                    .unwrap_or_else(|| panic!("no {key}"))
                    .1
            }
            other => panic!("not a dict: {other:?}"),
        }
    }

    #[test]
    fn an_archive_is_put_back_together_as_the_objects_it_was_made_of() {
        let doc = plist::Document::read(ARCHIVE).unwrap();
        // As written, it is the table: a $objects array and UIDs.
        assert!(matches!(get(&doc.raw, "$objects"), Value::Array(a) if a.len() == 19));
        let resolved = doc.archive.as_ref().expect("it is a keyed archive");
        // The root is the NSMutableDictionary, with its keys as the dictionary's keys.
        let root = get(resolved, "root");
        assert_eq!(get(root, "name"), &Value::String("REactor".into()));
        // An NSArray is an array: a number, a string, NSData, and the archive's nil.
        match get(root, "items") {
            Value::Array(items) => {
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[1], Value::String("two".into()));
                assert_eq!(items[2], Value::Data(vec![1, 2, 3]));
                assert_eq!(items[3], Value::Null);
            }
            other => panic!("{other:?}"),
        }
        // A class of the app's own keeps its name and fields, resolved: an NSMutableString
        // is a string, an NSDate a date, a number a number.
        let note = get(root, "note");
        assert_eq!(get(note, "$class"), &Value::String("Note".into()));
        assert_eq!(get(note, "title"), &Value::String("buy milk".into()));
        assert_eq!(get(note, "count"), &Value::Int(5));
        assert_eq!(
            plist::date_string(match get(note, "when") {
                Value::Date(d) => *d,
                other => panic!("{other:?}"),
            }),
            "2021-01-01T00:00:00Z"
        );
        // The note points back at the root: shown once, then as a pointer, not for ever.
        match get(note, "owner") {
            Value::String(s) => assert!(s.contains("again"), "{s}"),
            other => panic!("{other:?}"),
        }
        // `shown` picks the objects, or the raw table when that is asked for.
        assert_eq!(doc.shown(false), resolved);
        assert_eq!(doc.shown(true), &doc.raw);
    }

    #[test]
    fn a_property_list_that_is_not_an_archive_is_left_alone() {
        let plain =
            plist::Document::read(include_bytes!("../../tests/fixtures/sample.bplist")).unwrap();
        assert!(plain.archive.is_none());
        assert_eq!(plain.shown(false), &plain.raw);
        // A dictionary that merely has the key is not enough.
        let fake = Value::Dict(vec![(
            "$archiver".into(),
            Value::String("NSKeyedArchiver".into()),
        )]);
        assert!(resolve(&fake).is_none());
        assert!(resolve(&Value::Null).is_none());
    }

    #[test]
    fn a_hostile_archive_is_bounded() {
        // Object 1 is an array whose only item is object 1: a cycle through the class's own
        // collection, as well as the self-reference above.
        let class = Value::Dict(vec![("$classname".into(), Value::String("NSArray".into()))]);
        let arr = Value::Dict(vec![
            ("$class".into(), Value::Uid(2)),
            (
                "NS.objects".into(),
                Value::Array(vec![Value::Uid(1), Value::Uid(1)]),
            ),
        ]);
        let archive = Value::Dict(vec![
            ("$archiver".into(), Value::String("NSKeyedArchiver".into())),
            (
                "$objects".into(),
                Value::Array(vec![Value::String("$null".into()), arr, class]),
            ),
            (
                "$top".into(),
                Value::Dict(vec![("root".into(), Value::Uid(1))]),
            ),
        ]);
        let resolved = resolve(&archive).unwrap();
        match get(&resolved, "root") {
            Value::Array(items) => {
                assert!(
                    items
                        .iter()
                        .all(|i| matches!(i, Value::String(s) if s.contains("again")))
                );
            }
            other => panic!("{other:?}"),
        }
        // A reference to nothing is a note, not a crash.
        let dangling = Value::Dict(vec![
            ("$archiver".into(), Value::String("NSKeyedArchiver".into())),
            ("$objects".into(), Value::Array(vec![])),
            (
                "$top".into(),
                Value::Dict(vec![("root".into(), Value::Uid(9))]),
            ),
        ]);
        assert!(
            matches!(get(&resolve(&dangling).unwrap(), "root"), Value::String(s) if s.contains("no object"))
        );
    }

    #[test]
    fn data_that_is_a_property_list_or_an_archive_is_read_as_a_child() {
        let doc = plist::Document::read(NESTED).unwrap();
        // A plain blob is still bytes.
        assert!(matches!(get(&doc.raw, "plain"), Value::Data(_)));
        // A blob that is a binary plist is read: the bytes, and what they hold.
        match get(&doc.raw, "blob") {
            Value::Embedded { bytes, inner } => {
                assert!(bytes.starts_with(b"bplist"));
                assert_eq!(get(inner, "a"), &Value::Int(1));
            }
            other => panic!("{other:?}"),
        }
        // A blob that is a keyed archive is shown as what it archived, not as its table.
        match get(&doc.raw, "archive") {
            Value::Embedded { inner, .. } => {
                assert_eq!(
                    get(get(inner, "root"), "name"),
                    &Value::String("REactor".into())
                );
            }
            other => panic!("{other:?}"),
        }
        // They are children in the tree, reached by a path, and the summary says what they are.
        let blob = doc.raw.at(&[0]).unwrap();
        let rows = plist::rows(
            &doc.raw,
            &plist::containers(&doc.raw, usize::MAX, 1000),
            1000,
        );
        assert!(
            rows.iter().any(|r| r.summary.contains("a property list")),
            "{:?}",
            blob.summary()
        );
        // Written out as XML they are the data they were: nothing is lost.
        let text = plist::to_xml(&doc.raw);
        assert!(text.contains("<data>") && !text.contains("↳"));
    }
}
