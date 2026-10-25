//! How a property is named, drawn and read (ADR-0067): the shape a namespace gives, aliases
//! folded when a property is written, what a name may not be, and finding by property.

use reactor_db::Case;
use reactor_db::graph::fields::{self, Define, Shape};
use reactor_db::graph::{self, Batch, Change, Content, Filter, Kind, Put, query};
use reactor_db::manifest::Author;
use serde_json::{Map, Value, json};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn props(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

fn note(case: &Case, text: &str, p: Value) -> String {
    let put = Put {
        content: Content {
            value: Some(text.into()),
            props: props(p),
            ..Content::default()
        },
        ..Put::new(Kind::Annotation, "analysis.note")
    };
    graph::apply(case.db(), Batch::new(Author::analyst()).put(put)).unwrap();
    query::find(
        case.db(),
        &Filter {
            text: Some(text.into()),
            ..Filter::default()
        },
    )
    .unwrap()
    .remove(0)
    .id
}

#[test]
fn a_name_has_the_shape_of_its_longest_known_namespace() {
    let (_d, case) = case();
    let f = fields::load(case.db()).unwrap();
    assert_eq!(f.shape("time.first_seen"), Shape::Time);
    // A name nobody defined, under a namespace that is known, takes its shape.
    assert_eq!(f.shape("size.payload"), Shape::Size);
    assert_eq!(f.shape("size"), Shape::Size);
    assert_eq!(f.shape("verbatim.pattern.yara"), Shape::Verbatim);
    // No namespace, no shape: text.
    assert_eq!(f.shape("size_in_bytes"), Shape::Text);
    assert!(f.namespace_of("whatever").is_none());
    assert_eq!(f.label("time.first_seen"), "First seen");
    assert_eq!(f.label("size"), "Size");
    assert_eq!(f.label("number.port.src"), "Port src");
    assert_eq!(f.icon("time.first_seen"), Some("clock"));
}

#[test]
fn the_seed_never_collides_with_a_type() {
    let (_d, case) = case();
    let f = fields::load(case.db()).unwrap();
    let v = reactor_db::graph::vocab::load(case.db()).unwrap();
    let roots: Vec<&str> = v
        .types
        .keys()
        .map(|t| reactor_db::graph::vocab::Vocabulary::root(t))
        .collect();
    for ns in f.namespaces.keys() {
        assert!(!roots.contains(&ns.as_str()), "{ns} is the root of a type");
    }
    for name in f.fields.keys() {
        assert!(!v.types.contains_key(name), "{name} is a type");
        assert!(f.namespace_of(name).is_some(), "{name} has no namespace");
    }
    for (alias, name) in &f.aliases {
        assert!(f.fields.contains_key(name), "{alias} points at {name}");
        assert!(!f.fields.contains_key(alias) && !f.namespaces.contains_key(alias));
    }
}

#[test]
fn an_alias_is_folded_to_the_known_name_when_a_property_is_written() {
    let (_d, case) = case();
    let id = note(
        &case,
        "dropper",
        json!({"size_bytes": 184320, "cmdline": "a.exe -x", "custom_thing": "kept"}),
    );
    let e = query::get(case.db(), &id).unwrap().unwrap();
    let p = &e.content.props;
    assert_eq!(p.get("size"), Some(&json!(184320)));
    assert_eq!(p.get("verbatim.command_line"), Some(&json!("a.exe -x")));
    // A name that is not an alias is stored as written.
    assert_eq!(p.get("custom_thing"), Some(&json!("kept")));
    assert!(!p.contains_key("size_bytes") && !p.contains_key("cmdline"));

    // A change folds too, for what it sets and for what it removes.
    graph::apply(
        case.db(),
        Batch::new(Author::analyst()).change(Change {
            id: id.clone(),
            set_props: props(json!({"file_size": 7})),
            remove_props: vec!["cmd".into(), "custom_thing".into()],
            ..Change::default()
        }),
    )
    .unwrap();
    let e = query::get(case.db(), &id).unwrap().unwrap();
    assert_eq!(e.content.props.get("size"), Some(&json!(7)));
    assert!(!e.content.props.contains_key("custom_thing"));

    // Said again under another spelling, it is the same version: nothing is written.
    let again = Batch::new(Author::analyst()).change(Change {
        id: id.clone(),
        set_props: props(json!({"bytes": 7})),
        ..Change::default()
    });
    assert_eq!(graph::apply(case.db(), again).unwrap().event_seq, None);
}

#[test]
fn the_known_name_wins_when_both_it_and_an_alias_are_given() {
    let (_d, case) = case();
    let id = note(&case, "both", json!({"bytes": 1, "size": 2}));
    let e = query::get(case.db(), &id).unwrap().unwrap();
    assert_eq!(e.content.props.get("size"), Some(&json!(2)));
    assert_eq!(e.content.props.len(), 1);
}

#[test]
fn a_known_name_is_in_a_namespace_and_is_not_a_type() {
    let (_d, case) = case();
    let db = case.db();
    let made = fields::define_field(
        db,
        Define {
            name: "size.payload".into(),
            definition: Some("Bytes of a payload.".into()),
            aliases: vec!["payload_size".into()],
            ..Define::default()
        },
    )
    .unwrap();
    assert_eq!(made.name, "size.payload");
    let id = note(&case, "p", json!({"payload_size": 9}));
    let e = query::get(db, &id).unwrap().unwrap();
    assert_eq!(e.content.props.get("size.payload"), Some(&json!(9)));

    // No namespace, no definition.
    assert!(
        fields::define_field(
            db,
            Define {
                name: "loose".into(),
                ..Define::default()
            }
        )
        .is_err()
    );
    // A spelling that is a known name is not an alias.
    let clash = fields::define_field(
        db,
        Define {
            name: "size.other".into(),
            aliases: vec!["size.payload".into()],
            ..Define::default()
        },
    );
    assert!(clash.is_err());
    // A new prefix points at a shape, and may not be a type's root or alias.
    fields::define_namespace(
        db,
        "when",
        Some(Shape::Time),
        Some("An instant."),
        None,
        None,
    )
    .unwrap();
    assert_eq!(fields::load(db).unwrap().shape("when.opened"), Shape::Time);
    for bad in ["file", "net", "host", "code", "ip", "url"] {
        assert!(
            fields::define_namespace(db, bad, Some(Shape::Text), None, None, None).is_err(),
            "{bad}"
        );
    }
    assert!(fields::define_namespace(db, "unshaped", None, None, None, None).is_err());
}

#[test]
fn names_without_a_namespace_are_listed_for_the_notice() {
    let (_d, case) = case();
    let f = fields::load(case.db()).unwrap();
    let keys: Vec<String> = ["size_bytes", "time.first_seen", "stuff", "scope", "stuff"]
        .map(String::from)
        .into();
    // `size_bytes` folds to `size`; `scope` is the graph's own; `stuff` is reported once.
    assert_eq!(f.without_namespace(&keys), vec!["stuff".to_string()]);
}

#[test]
fn the_vocabulary_lists_its_names_for_the_agent() {
    let (_d, case) = case();
    let all = fields::describe(case.db(), None).unwrap();
    assert!(all.contains("time (time) --"));
    assert!(all.contains("  time.first_seen "));
    assert!(all.contains("(also size_bytes, file_size, bytes)") || all.contains("also bytes"));
    let one = fields::describe(case.db(), Some("verbatim")).unwrap();
    assert!(one.contains("verbatim.command_line") && !one.contains("time.first_seen"));
    assert_eq!(
        fields::describe(case.db(), Some("nothing")).unwrap(),
        "no known property names there"
    );
}

#[test]
fn a_search_compares_a_property_by_its_shape() {
    let (_d, case) = case();
    let db = case.db();
    note(
        &case,
        "small",
        json!({"size": 2048, "time.first_seen": "2026-03-01T00:00:00Z", "category.severity": "Low"}),
    );
    note(
        &case,
        "big",
        json!({"size_bytes": 5242880, "time.first_seen": "2026-04-01T00:00:00Z", "category.severity": "high", "flag.hidden": true}),
    );
    note(&case, "plain", json!({"loose": "x"}));
    let names = |props: &[&str]| -> Vec<String> {
        let mut got: Vec<String> = query::find(
            db,
            &Filter {
                props: props.iter().map(|p| p.to_string()).collect(),
                ..Filter::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|e| e.content.value.unwrap_or_default())
        .collect();
        got.sort();
        got
    };
    assert_eq!(names(&["size>1MiB"]), ["big"]);
    assert_eq!(names(&["bytes<1MiB"]), ["small"]);
    assert_eq!(names(&["size"]), ["big", "small"]);
    assert_eq!(names(&["time.first_seen<2026-03-15"]), ["small"]);
    // A namespace takes every name under it.
    assert_eq!(names(&["time."]), ["big", "small"]);
    // Categories compare without case; a flag is a boolean.
    assert_eq!(names(&["category.severity=HIGH"]), ["big"]);
    assert_eq!(names(&["flag.hidden=true"]), ["big"]);
    // Every filter must hold; a name nobody wrote matches nothing.
    assert_eq!(names(&["size>1KiB", "time.first_seen>2026-03-15"]), ["big"]);
    assert!(names(&["duration"]).is_empty());
    assert_eq!(names(&["loose=X"]), ["plain"]);
    assert!(
        query::find(
            db,
            &Filter {
                props: vec!["=3".into()],
                ..Filter::default()
            }
        )
        .is_err()
    );
    // A page is cut after the filter, not before it.
    let page = query::find(
        db,
        &Filter {
            props: vec!["time.".into()],
            limit: 1,
            offset: 1,
            ..Filter::default()
        },
    )
    .unwrap();
    assert_eq!(page.len(), 1);
}
