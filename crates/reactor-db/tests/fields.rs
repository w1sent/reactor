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

// -- a person writes a property --------------------------------------------------------------

#[test]
fn what_a_person_types_is_stored_as_the_shape_of_its_name_says() {
    use reactor_db::graph::fields::value_from_text as v;
    assert_eq!(v(Shape::Size, "180KiB", false).unwrap(), json!(184_320));
    assert_eq!(v(Shape::Size, "184320", false).unwrap(), json!(184_320));
    assert_eq!(v(Shape::Duration, "4.2s", false).unwrap(), json!(4200));
    assert_eq!(v(Shape::Number, "8080", false).unwrap(), json!(8080));
    assert_eq!(v(Shape::Number, "7.42", false).unwrap(), json!(7.42));
    assert_eq!(v(Shape::Flag, "Yes", false).unwrap(), json!(true));
    assert_eq!(v(Shape::Flag, "no", false).unwrap(), json!(false));
    assert_eq!(
        v(Shape::Time, "2026-03-12 09:14", false).unwrap(),
        json!("2026-03-12T09:14:00.000Z")
    );
    // Text that does not suit the shape is kept as typed, not refused.
    assert_eq!(v(Shape::Size, "big", false).unwrap(), json!("big"));
    assert_eq!(v(Shape::Flag, "maybe", false).unwrap(), json!("maybe"));
    // Lines are items, except for text kept whole.
    assert_eq!(
        v(Shape::Category, "T1059.001\nT1105\n\n", false).unwrap(),
        json!(["T1059.001", "T1105"])
    );
    assert_eq!(v(Shape::Category, "T1105", false).unwrap(), json!("T1105"));
    assert_eq!(v(Shape::Category, "T1105", true).unwrap(), json!(["T1105"]));
    assert_eq!(
        v(Shape::Verbatim, "a.exe  -x\n  -y\n", false).unwrap(),
        json!("a.exe  -x\n  -y")
    );
    assert_eq!(
        v(Shape::Text, "one\ntwo", true).unwrap(),
        json!(["one", "two"])
    );
    assert!(v(Shape::Text, "  \n ", false).is_err());
}

#[test]
fn a_person_sets_and_removes_a_property_and_every_step_is_a_version() {
    use reactor_db::graph::edit::{self, At};
    let (_d, case) = case();
    let db = case.db();
    let at = At::new("s1", 3);
    let id = note(&case, "evidence", json!({"loose": "x"}));

    edit::set_prop(db, &at, &id, "size_bytes", json!(184_320)).unwrap();
    edit::set_prop(
        db,
        &at,
        &id,
        "time.first_seen",
        json!("2026-03-12T09:14:00Z"),
    )
    .unwrap();
    let e = query::get(db, &id).unwrap().unwrap();
    // The alias was folded; what was there stays.
    assert_eq!(e.content.props.get("size"), Some(&json!(184_320)));
    assert_eq!(e.content.props.get("loose"), Some(&json!("x")));
    assert_eq!(e.ver, 3);

    // The same value again is no new version.
    let again = edit::set_prop(db, &at, &id, "size", json!(184_320)).unwrap();
    assert_eq!(again.event_seq, None);

    // A name must be words joined by dots; the graph's own names are not for hand.
    for bad in ["Size Bytes", "", "a..b", "scope", "hypothesis_status"] {
        assert!(
            edit::set_prop(db, &at, &id, bad, json!(1)).is_err(),
            "{bad:?}"
        );
    }
    assert!(edit::set_prop(db, &at, "nothing", "size", json!(1)).is_err());

    // Removing takes it away in a new version, by an alias too, and says if it was not there.
    edit::remove_prop(db, &at, &id, "bytes").unwrap();
    edit::remove_prop(db, &at, &id, "loose").unwrap();
    let e = query::get(db, &id).unwrap().unwrap();
    assert_eq!(e.content.props.len(), 1);
    assert!(edit::remove_prop(db, &at, &id, "size").is_err());
    assert_eq!(query::history(db, &id).unwrap().len(), 5);
    // The old versions keep what was removed.
    assert!(
        query::history(db, &id).unwrap()[0]
            .content
            .props
            .contains_key("loose")
    );
}

#[test]
fn an_entitys_key_cannot_be_set_by_hand_but_its_details_can() {
    use reactor_db::graph::edit::{self, At};
    let (_d, case) = case();
    let db = case.db();
    let at = At::new("s1", 3);
    let id = edit::add_entity(db, &at, "net.ipv4", "198.51.100.7").unwrap();
    edit::set_prop(db, &at, &id, "category.severity", json!("high")).unwrap();
    assert!(edit::set_prop(db, &at, &id, "scope", json!("lab")).is_err());
    let e = query::get(db, &id).unwrap().unwrap();
    assert_eq!(
        e.content.props.get("category.severity"),
        Some(&json!("high"))
    );
}
