//! How types are grouped into namespaces and drawn (`graph::vocab`), and where a node's own
//! size is kept.

use reactor_db::Case;
use reactor_db::graph::vocab::{self, COLOURS, Define, Vocabulary};
use reactor_db::graph::{Kind, layout, query};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

#[test]
fn a_type_looks_like_itself_else_its_nearest_namespace() {
    let (_d, case) = case();
    let v = vocab::load(case.db()).unwrap();
    // A seeded type with an icon of its own, and its namespace's colour.
    let domain = v.look("net.domain");
    assert_eq!(
        (domain.icon.as_deref(), domain.colour.as_deref(), domain.own),
        (Some("globe"), Some("blue"), true)
    );
    // One that says nothing: the nearest namespace speaks, `file.hash` before `file`.
    let sha = v.look("file.hash.sha256");
    assert_eq!(
        (sha.icon.as_deref(), sha.colour.as_deref(), sha.own),
        (Some("hash"), Some("purple"), false)
    );
    // A type nobody defined, in a namespace that is known.
    let custom = v.look("file.hash.authentihash");
    assert_eq!(custom.icon.as_deref(), Some("hash"));
    let asn = v.look("net.peering");
    assert_eq!(
        (asn.icon.as_deref(), asn.colour.as_deref()),
        (Some("network"), Some("blue"))
    );
    // And one in no namespace at all: nothing is said.
    assert_eq!(v.look("zzz.thing"), vocab::Look::default());
    assert_eq!(Vocabulary::root("net.tls.ja3"), "net");
}

#[test]
fn every_seeded_type_is_in_a_namespace_with_a_colour_that_exists() {
    let (_d, case) = case();
    let v = vocab::load(case.db()).unwrap();
    assert!(v.types.len() > 150, "{} types", v.types.len());
    for ns in v.namespaces.values() {
        let colour = ns.colour.as_deref().unwrap_or("none");
        assert!(COLOURS.contains(&colour), "{}: {colour}", ns.prefix);
        assert!(ns.icon.is_some(), "{}", ns.prefix);
        assert!(!ns.definition.is_empty(), "{}", ns.prefix);
    }
    for t in v.types.values() {
        assert!(
            v.namespaces.contains_key(Vocabulary::root(&t.name)),
            "{} is in no namespace",
            t.name
        );
        assert!(t.kind.is_some(), "{}", t.name);
        assert!(!t.definition.is_empty(), "{}", t.name);
        let look = v.look(&t.name);
        assert!(look.icon.is_some() && look.colour.is_some(), "{}", t.name);
    }
}

#[test]
fn a_look_is_set_and_taken_back_and_an_unknown_type_becomes_known() {
    let (_d, case) = case();
    let db = case.db();
    vocab::set_look(db, "Hunt.Lead", Kind::Entity, Some("flag"), Some("magenta")).unwrap();
    let v = vocab::load(db).unwrap();
    let look = v.look("hunt.lead");
    assert_eq!(
        (look.icon.as_deref(), look.colour.as_deref(), look.own),
        (Some("flag"), Some("magenta"), true)
    );
    assert_eq!(v.types["hunt.lead"].kind, Some(Kind::Entity));
    // Taking it back leaves the type known and falls back to whatever is above it.
    vocab::set_look(db, "net.domain", Kind::Entity, None, None).unwrap();
    let domain = vocab::load(db).unwrap().look("net.domain");
    assert_eq!(
        (domain.icon.as_deref(), domain.own),
        (Some("network"), false)
    );
    // Orange and red are taken; a name with spaces is no type; an icon is a name.
    for (icon, colour) in [
        (None, Some("orange")),
        (None, Some("red")),
        (Some("a b"), None),
    ] {
        assert!(vocab::set_look(db, "hunt.lead", Kind::Entity, icon, colour).is_err());
    }
    assert!(vocab::set_look(db, "not a type", Kind::Entity, None, None).is_err());
    // Identity is not disturbed: a value still lands on the same node.
    let a = query::entity_id(db, "net.domain", "Example.ORG.", None).unwrap();
    assert_eq!(
        a,
        query::entity_id(db, "net.domain", "example.org", None).unwrap()
    );
}

#[test]
fn a_type_is_defined_once_and_only_what_is_given_changes() {
    let (_d, case) = case();
    let db = case.db();
    let missing_kind = vocab::define_type(
        db,
        Define {
            name: "ics.plc".into(),
            ..Define::default()
        },
    );
    assert!(missing_kind.is_err());
    let made = vocab::define_type(
        db,
        Define {
            name: "ics.plc".into(),
            kind: Some(Kind::Entity),
            definition: Some("A programmable logic controller.".into()),
            icon: Some("cpu".into()),
            colour: None,
        },
    )
    .unwrap();
    assert_eq!((made.icon.as_deref(), made.colour), (Some("cpu"), None));
    let again = vocab::define_type(
        db,
        Define {
            name: "ics.plc".into(),
            colour: Some("teal".into()),
            ..Define::default()
        },
    )
    .unwrap();
    assert_eq!(again.definition, "A programmable logic controller.");
    assert_eq!(
        (again.icon.as_deref(), again.colour.as_deref()),
        (Some("cpu"), Some("teal"))
    );
    // A kind does not change under what already uses it.
    assert!(
        vocab::define_type(
            db,
            Define {
                name: "ics.plc".into(),
                kind: Some(Kind::Event),
                ..Define::default()
            },
        )
        .is_err()
    );
    vocab::define_namespace(
        db,
        "ics",
        Some("Industrial control systems."),
        Some("factory"),
        Some("olive"),
    )
    .unwrap();
    let v = vocab::load(db).unwrap();
    assert_eq!(v.look("ics.hmi").colour.as_deref(), Some("olive"));
    let text = vocab::describe(db, Some("ics")).unwrap();
    assert!(
        text.starts_with("ics -- Industrial control systems."),
        "{text}"
    );
    assert!(
        text.contains("ics.plc (entity) A programmable logic controller."),
        "{text}"
    );
    assert!(!text.contains("net.ipv4"));
    assert!(
        vocab::describe(db, None)
            .unwrap()
            .contains("  net.ipv4 (entity)")
    );
}

#[test]
fn a_new_normalizer_keeps_ids_that_differ_only_in_case_together() {
    let (_d, case) = case();
    let db = case.db();
    assert_eq!(
        query::entity_id(db, "vuln.cve", "cve-2024-3400", None).unwrap(),
        query::entity_id(db, "cve", " CVE-2024-3400 ", None).unwrap()
    );
}

#[test]
fn a_node_keeps_a_size_and_a_width_of_its_own_until_both_are_given_up() {
    let (_d, case) = case();
    let db = case.db();
    let size = |size: Option<&str>, width: Option<f32>| layout::NodeSize {
        size: size.map(str::to_string),
        width,
    };
    layout::save_sizes(
        db,
        &[
            ("a".into(), size(Some("xl"), None)),
            ("b".into(), size(None, Some(260.))),
        ],
    )
    .unwrap();
    let loaded = layout::load_sizes(db).unwrap();
    assert_eq!(loaded["a"], size(Some("xl"), None));
    assert_eq!(loaded["b"], size(None, Some(260.)));
    layout::save_sizes(db, &[("a".into(), size(None, None))]).unwrap();
    let loaded = layout::load_sizes(db).unwrap();
    assert!(!loaded.contains_key("a") && loaded.contains_key("b"));
    // Forgetting where things are does not forget how big they were made.
    layout::clear(db).unwrap();
    assert_eq!(layout::load_sizes(db).unwrap().len(), 1);
}
