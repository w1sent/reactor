//! References against a case: the alias a case answers to, what exists, and what a set holds
//! ([ADR-0066](../../../docs/adr/0066-the-case-is-addressed-by-references.md)).

use reactor_db::Case;
use reactor_db::graph::{self, Batch, Content, Kind, Link, Put};
use reactor_db::manifest::Author;
use reactor_db::reference::lookup::{self, Page};
use reactor_db::reference::{self, GraphId, Reference, Target};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), Some("Incident 17")).unwrap();
    (dir, case)
}

fn write(case: &Case, kind: Kind, type_: &str, value: &str, links: Vec<Link>) -> String {
    let mut put = Put::new(kind, type_);
    put.content = Content::value(value);
    put.links = links;
    let batch = Batch::new(Author::analyst()).put(put);
    let applied = graph::apply(case.db(), batch).unwrap();
    applied.ids[0].clone()
}

#[test]
fn a_case_answers_to_its_id_its_alias_and_the_aliases_it_had() {
    let (_d, case) = case();
    let id = case.meta().id;
    assert_eq!(case.authority(), id, "no alias yet: the id");
    assert!(case.is_named(&id.to_uppercase()));

    case.set_alias(Some("acme")).unwrap();
    assert_eq!(case.authority(), "acme");
    assert!(case.is_named("ACME") && case.is_named(&id));

    case.set_alias(Some("acme-2")).unwrap();
    assert_eq!(case.authority(), "acme-2");
    assert!(case.is_named("acme"), "the old alias still names it");
    assert!(!case.is_named("other"));

    assert!(case.set_alias(Some("has space")).is_err());
    assert!(case.set_alias(Some("")).is_ok(), "an empty alias takes it away");
    assert_eq!(case.authority(), id);

    // It is in case.json, so it survives a reopen.
    case.set_alias(Some("kept")).unwrap();
    let root = case.root().to_path_buf();
    drop(case);
    let again = Case::open(&root).unwrap();
    assert_eq!(again.alias().as_deref(), Some("kept"));
    assert!(again.is_named("acme-2"));
}

#[test]
fn what_exists_is_told_from_what_does_not() {
    let (_d, case) = case();
    let ip = write(&case, Kind::Entity, "net.ipv4", "198.51.100.7", vec![]);
    std::fs::write(case.root().join("a.csv"), "x\n1\n").unwrap();

    let yes = |s: &str| lookup::exists(&case, &reference::parse(s).unwrap().target).unwrap();
    assert!(yes(&format!("graph:{ip}")));
    assert!(yes("graph:entity/net.ipv4/198.51.100.7"));
    assert!(!yes("graph:entity/net.ipv4/203.0.113.9"));
    assert!(yes(&format!("graph:{ip}@1")));
    assert!(!yes(&format!("graph:{ip}@2")));
    assert!(!yes("graph:entity--nope"));
    assert!(yes("artifact:a.csv#row=1"));
    assert!(!yes("artifact:missing.csv"));
    assert!(!yes("session:nope"));
    assert!(!yes("change:9999"));
    assert!(yes("change:1"));
    assert!(yes("vocab:net.ipv4"));
    assert!(yes("graph:?type=net."));
}

#[test]
fn a_set_is_a_page_of_references() {
    let (_d, case) = case();
    for i in 0..5 {
        write(&case, Kind::Entity, "net.ipv4", &format!("198.51.100.{i}"), vec![]);
    }
    write(&case, Kind::Entity, "host.name", "ws-042", vec![]);

    let set = |s: &str, page: Page| {
        let Target::Set { kind, params } = reference::parse(s).unwrap().target else {
            panic!("{s} is not a set")
        };
        lookup::list(&case, kind, &params, page).unwrap()
    };
    let all = set("graph:?type=net.", Page { limit: 3, offset: 0 });
    assert_eq!(all.items.len(), 3);
    assert!(all.more);
    let rest = set("graph:?type=net.", Page { limit: 3, offset: 3 });
    assert_eq!(rest.items.len(), 2);
    assert!(!rest.more);
    assert!(all.items[0].preview.starts_with("entity net.ipv4 198.51.100."));
    assert!(matches!(
        &rest.items[0].reference.target,
        Target::Graph { id: GraphId::Id(_), ver: None }
    ));

    let one = set("graph:?text=ws-042", Page::default());
    assert_eq!(one.items.len(), 1);
    assert!(lookup::list(&case, reference::Kind::Blob, &[], Page::default()).is_err());
    assert!(lookup::list(&case, reference::Kind::Graph, &[("kind".into(), "x".into())], Page::default()).is_err());

    // The references it returns resolve.
    for item in all.items {
        assert!(lookup::exists(&case, &item.reference.target).unwrap());
        let again: Reference = reference::parse(&item.reference.short()).unwrap();
        assert_eq!(again.target, item.reference.target);
    }
}

#[test]
fn an_export_names_the_case_in_every_reference() {
    use reactor_db::interchange::{stix, tables};
    let (_d, case) = case();
    case.set_alias(Some("acme")).unwrap();
    let ip = write(&case, Kind::Entity, "net.ipv4", "198.51.100.7", vec![]);
    let claim = write(
        &case,
        Kind::Observation,
        "net.connection",
        "see graph:entity/net.ipv4/198.51.100.7",
        vec![
            Link::entity("dst", "net.ipv4", "198.51.100.7"),
            Link::source("artifact:data/a.csv").at("row=2", None),
        ],
    );

    let elements = tables::elements(&case, false).unwrap();
    assert!(
        elements.contains(&format!("reactor://acme/graph/{ip}@1")),
        "{elements}"
    );
    assert!(elements.contains("reactor://acme/artifact/data/a.csv"), "{elements}");
    assert!(
        elements.contains("see reactor://acme/graph/entity/net.ipv4/198.51.100.7"),
        "a reference in a text is written out too: {elements}"
    );
    let edges = tables::edges(&case, false).unwrap();
    assert!(edges.contains("reactor://acme/artifact/data/a.csv"), "{edges}");

    // A bundle of the case restores into another case with the sources as they were.
    let bundle = stix::export(&case, &stix::ExportOptions { include_deleted: false }).unwrap();
    let text = bundle.to_string();
    assert!(text.contains("reactor://acme/artifact/data/a.csv"), "{text}");
    assert!(text.contains("\"alias\":\"acme\""), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let other = Case::create(dir.path(), None).unwrap();
    let done = stix::import(&other, &bundle, "acme.json").unwrap();
    assert!(done.restored);
    let sources = graph::query::sources_of(other.db(), &claim).unwrap();
    assert_eq!(sources[0].uri, "artifact:data/a.csv");
    assert_eq!(sources[0].locator.as_deref(), Some("row=2"));
}
