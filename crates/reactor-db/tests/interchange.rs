//! The graph in other tools' formats: STIX 2.1 both ways, CSV, and the projection.

use std::collections::BTreeSet;

use reactor_db::Case;
use reactor_db::graph::{self, Batch, Change, Content, Kind, Link, Put, Target, query};
use reactor_db::interchange::projection::{self, Options};
use reactor_db::interchange::{stix, tables};
use reactor_db::manifest::Author;
use serde_json::{Value, json};

fn case(title: Option<&str>) -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), title).unwrap();
    (dir, case)
}

fn claim(kind: Kind, type_: &str, links: Vec<Link>) -> Put {
    Put {
        links,
        ..Put::new(kind, type_)
    }
}

/// A graph with a bit of everything.
fn analysis(case: &Case) {
    let db = case.db();
    let ids = graph::apply(
        db,
        Batch::new(Author::analyst())
            .put(claim(
                Kind::Event,
                "auth.logon",
                vec![
                    Link::entity("actor", "account.user", "jdoe"),
                    Link::entity("target", "host.name", "ws-042"),
                    Link::source("artifact:reactor_data.sqlite#table=vpn_auth")
                        .at("row=1204", Some("f1")),
                ],
            ))
            .put(claim(
                Kind::Event,
                "net.connection",
                vec![
                    Link::entity("src", "host.name", "ws-042"),
                    Link::entity("dst", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:fw.csv"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "net.resolution",
                vec![
                    Link::entity("query", "net.domain", "cdn-sync.example"),
                    Link::entity("answer", "net.ipv4", "198.51.100.7"),
                    Link::source("https://pdns.example/lookup?q=cdn-sync.example")
                        .at("row=7", None),
                ],
            ))
            .put(claim(
                Kind::Event,
                "process.start",
                vec![
                    Link::entity("parent", "process.name", "winword.exe"),
                    Link::entity("child", "process.name", "powershell.exe"),
                    Link::entity("actor", "account.user", "jdoe"),
                    Link::source("samples/edr.json"),
                ],
            ))
            .put(claim(
                Kind::Observation,
                "file.seen",
                vec![
                    Link::entity(
                        "subject",
                        "file.hash.sha256",
                        "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789",
                    ),
                    Link::entity("object", "file.name", "dropper.dll"),
                    Link::source("samples/report.json"),
                ],
            )),
    )
    .unwrap()
    .ids;
    // The resolution has a time and a value; the logon a time with a zone note.
    graph::apply(
        db,
        Batch::new(Author::agent("m")).change(Change {
            id: ids[2].clone(),
            confidence: Some(Some("likely".into())),
            add_tags: vec!["phase.c2".into()],
            time: Some(reactor_db::graph::Time {
                valid_from: Some("2026-09-28T00:00:00.000Z".into()),
                valid_to: Some("2026-10-02T00:00:00.000Z".into()),
                ..Default::default()
            }),
            ..Change::default()
        }),
    )
    .unwrap();
    graph::apply(
        db,
        Batch::new(Author::analyst()).change(Change {
            id: ids[0].clone(),
            time: Some(reactor_db::graph::Time {
                occurred_at: Some("2026-10-01T03:12:07.000Z".into()),
                raw: Some("2026-10-01 05:12:07 +02:00".into()),
                tz_certainty: Some("stated".into()),
                ..Default::default()
            }),
            ..Change::default()
        }),
    )
    .unwrap();
    // Annotations of each kind.
    let hyp = graph::apply(
        db,
        Batch::new(Author::analyst()).put(Put {
            content: Content {
                value: Some("jdoe is compromised".into()),
                ..Content::default()
            },
            ..Put::new(Kind::Annotation, "analysis.hypothesis")
        }),
    )
    .unwrap()
    .ids
    .remove(0);
    graph::apply(
        db,
        Batch::new(Author::agent("m")).put(Put {
            content: Content {
                value: Some("The resolution overlaps the beacon window".into()),
                confidence: Some("very likely".into()),
                ..Content::default()
            },
            links: vec![
                Link::new("about", Target::Id(ids[2].clone())).pinned(),
                Link::new("supports", Target::Id(hyp)),
            ],
            ..Put::new(Kind::Annotation, "analysis.judgement")
        }),
    )
    .unwrap();
    graph::apply(
        db,
        Batch::new(Author::analyst())
            .put(Put {
                content: Content::value("same operator?"),
                links: vec![
                    Link::new("member", Target::Id(ids[0].clone())),
                    Link::new("member", Target::Id(ids[1].clone())),
                ],
                ..Put::new(Kind::Annotation, "analysis.cluster")
            })
            .put(Put {
                content: Content::value("ask the SOC"),
                ..Put::new(Kind::Annotation, "analysis.note")
            }),
    )
    .unwrap();
}

fn id_sets(case: &Case) -> (BTreeSet<String>, BTreeSet<String>) {
    let (elements, edges) = query::snapshot(case.db(), true).unwrap();
    (
        elements.iter().map(|e| e.id.clone()).collect(),
        edges.iter().map(|g| g.id.clone()).collect(),
    )
}

fn types(bundle: &Value) -> BTreeSet<String> {
    bundle["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["type"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_bundle_follows_the_mapping_is_valid_and_is_the_same_every_time() {
    let (_d, case) = case(Some("Incident 17"));
    analysis(&case);
    let bundle = stix::export(&case, &stix::ExportOptions::default()).unwrap();
    let problems = stix::validate(&bundle);
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(bundle["type"], "bundle");

    // The graph concept's table: entities are observables, events observed data, a binary
    // observation a relationship, judgements opinions, clusters groupings, the rest notes.
    let got = types(&bundle);
    for t in [
        "identity",
        "user-account",
        "ipv4-addr",
        "domain-name",
        "file",
        "observed-data",
        "relationship",
        "opinion",
        "grouping",
        "note",
        "x-reactor-entity",
    ] {
        assert!(got.contains(t), "no {t} in {got:?}");
    }
    let objects = bundle["objects"].as_array().unwrap();
    let find = |t: &str| {
        objects
            .iter()
            .filter(|o| o["type"] == t)
            .collect::<Vec<_>>()
    };
    // The identity is the case, and everything else was created by it.
    let identity = &find("identity")[0];
    assert_eq!(identity["name"], "Incident 17");
    assert!(
        objects
            .iter()
            .filter(|o| o["type"] != "identity" && o.get("created_by_ref").is_some())
            .all(|o| o["created_by_ref"] == identity["id"])
    );
    // The resolution is a relationship from the domain to the address, with its validity and its source.
    let rel = &find("relationship")[0];
    assert_eq!(rel["relationship_type"], "net-resolution");
    assert_eq!(rel["start_time"], "2026-09-28T00:00:00.000Z");
    assert_eq!(rel["confidence"], 70);
    assert_eq!(
        rel["external_references"][0]["url"],
        "https://pdns.example/lookup?q=cdn-sync.example"
    );
    let domain = objects.iter().find(|o| o["type"] == "domain-name").unwrap();
    let address = objects.iter().find(|o| o["type"] == "ipv4-addr").unwrap();
    assert_eq!(
        (&rel["source_ref"], &rel["target_ref"]),
        (&domain["id"], &address["id"])
    );
    // An SHA-256 is a file with that hash; a host has no STIX type and is custom.
    assert!(objects.iter().any(|o| o["type"] == "file"
        && o["hashes"]["SHA-256"]
            == "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"));
    assert!(
        objects
            .iter()
            .any(|o| o["type"] == "x-reactor-entity" && o["entity_type"] == "host.name")
    );
    // The judgement is an opinion with the strongest agreement, about its claim.
    let opinion = &find("opinion")[0];
    assert_eq!(opinion["opinion"], "strongly-agree");
    assert_eq!(opinion["confidence"], 85);
    // A note about nothing in particular is about the case.
    assert!(
        find("note")
            .iter()
            .any(|n| n["content"] == "ask the SOC" && n["object_refs"][0] == identity["id"])
    );
    // The same graph exports as the same bundle.
    assert_eq!(
        bundle,
        stix::export(&case, &stix::ExportOptions::default()).unwrap()
    );
}

#[test]
fn a_bundle_with_what_is_wrong_in_it_is_found_out() {
    let (_d, case) = case(None);
    analysis(&case);
    let good = stix::export(&case, &Default::default()).unwrap();
    let broken = |edit: &dyn Fn(&mut Value)| {
        let mut b = good.clone();
        edit(&mut b);
        stix::validate(&b)
    };
    let first_of = |b: &mut Value, t: &str| -> usize {
        b["objects"]
            .as_array()
            .unwrap()
            .iter()
            .position(|o| o["type"] == t)
            .unwrap()
    };
    assert!(
        broken(&|b| {
            let i = first_of(b, "relationship");
            b["objects"][i]["source_ref"] =
                json!("ipv4-addr--00000000-0000-5000-8000-000000000000");
        })
        .iter()
        .any(|p| p.contains("not in the bundle"))
    );
    assert!(
        broken(&|b| {
            let i = first_of(b, "observed-data");
            b["objects"][i]["object_refs"] = json!([]);
        })
        .iter()
        .any(|p| p.contains("must not be empty"))
    );
    assert!(
        broken(&|b| {
            let i = first_of(b, "opinion");
            b["objects"][i]["opinion"] = json!("maybe");
        })
        .iter()
        .any(|p| p.contains("five values"))
    );
    assert!(
        broken(&|b| {
            let i = first_of(b, "note");
            b["objects"][i]["created"] = json!("yesterday");
        })
        .iter()
        .any(|p| p.contains("timestamp"))
    );
    assert!(
        broken(&|b| {
            let i = first_of(b, "note");
            b["objects"][i]["id"] = json!("note--nope");
        })
        .iter()
        .any(|p| p.contains("<type>--<uuid>"))
    );
    assert!(
        broken(&|b| {
            let i = first_of(b, "relationship");
            b["objects"][i]["relationship_type"] = json!("Net.Resolution");
        })
        .iter()
        .any(|p| p.contains("hyphens"))
    );
    assert!(
        broken(&|b| {
            b["objects"] = json!("none");
        })
        .iter()
        .any(|p| p.contains("objects"))
    );
}

#[test]
fn a_bundle_restores_the_graph_exactly_in_another_case() {
    let (_a, a) = case(Some("A"));
    analysis(&a);
    let bundle = stix::export(&a, &Default::default()).unwrap();

    let (_b, b) = case(Some("B"));
    let done = stix::import(&b, &bundle, "from-a").unwrap();
    assert!(done.restored && done.skipped.is_empty(), "{done:?}");

    // The same elements and the same edges, by id.
    let (ea, ga) = id_sets(&a);
    let (eb, gb) = id_sets(&b);
    // Externals are not objects; they are re-made from the links that name them.
    assert_eq!(ea, eb);
    assert_eq!(ga, gb);
    assert_eq!(
        done.elements,
        ea.iter().filter(|id| !id.starts_with("external--")).count()
    );

    // What each says, and who said it and when.
    let (xa, _) = query::snapshot(a.db(), true).unwrap();
    for e in xa.iter().filter(|e| e.kind != Kind::External) {
        let back = query::get(b.db(), &e.id).unwrap().unwrap();
        assert_eq!(back.content, e.content, "{} {}", e.type_, e.id);
        assert_eq!(back.author, e.author, "{}", e.id);
        assert_eq!(back.ver, 1, "history is not exported");
        let first = query::history(a.db(), &e.id).unwrap()[0]
            .recorded_at
            .clone();
        assert_eq!(
            back.recorded_at, first,
            "an element keeps the time it was first recorded"
        );
    }
    // The claims still have their sources, with where in them; the judgement still pins.
    let rel_a = query::relations(a.db(), None).unwrap();
    assert_eq!(query::relations(b.db(), None).unwrap(), rel_a);
    let logon = xa.iter().find(|e| e.type_ == "auth.logon").unwrap();
    let src = query::edges_from(b.db(), &logon.id)
        .unwrap()
        .into_iter()
        .find(|g| g.role == "source")
        .unwrap();
    assert_eq!(
        (src.locator.as_deref(), src.fingerprint.as_deref()),
        (Some("row=1204"), Some("f1"))
    );
    let judgement = xa.iter().find(|e| e.type_ == "analysis.judgement").unwrap();
    assert!(
        query::edges_from(b.db(), &judgement.id)
            .unwrap()
            .iter()
            .any(|g| g.role == "about" && g.pinned_ver.is_some())
    );
    // And it is a valid thing to export again, giving what it was given.
    let again = stix::export(&b, &Default::default()).unwrap();
    assert!(stix::validate(&again).is_empty());
    let object_ids = |v: &Value| -> BTreeSet<String> {
        v["objects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["type"] != "identity")
            .map(|o| o["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(object_ids(&again), object_ids(&bundle));
    // Importing it again changes nothing.
    let n = id_sets(&b);
    stix::import(&b, &bundle, "again").unwrap();
    assert_eq!(id_sets(&b), n);
}

#[test]
fn what_is_soft_deleted_stays_home_unless_asked_for() {
    let (_d, case) = case(None);
    analysis(&case);
    let (elements, _) = query::snapshot(case.db(), false).unwrap();
    let logon = elements
        .iter()
        .find(|e| e.type_ == "auth.logon")
        .unwrap()
        .id
        .clone();
    graph::apply(
        case.db(),
        Batch::new(Author::analyst()).change(Change {
            id: logon,
            status: Some("soft-deleted".into()),
            ..Change::default()
        }),
    )
    .unwrap();
    let plain = stix::export(&case, &Default::default()).unwrap();
    let all = stix::export(
        &case,
        &stix::ExportOptions {
            include_deleted: true,
        },
    )
    .unwrap();
    let count = |b: &Value| b["objects"].as_array().unwrap().len();
    assert_eq!(count(&all), count(&plain) + 1);
    assert!(stix::validate(&plain).is_empty() && stix::validate(&all).is_empty());
    // The cluster that listed the deleted logon no longer points at it, and still validates.
    let cluster = plain["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["type"] == "grouping")
        .unwrap();
    assert_eq!(cluster["object_refs"].as_array().unwrap().len(), 1);
}

#[test]
fn a_bundle_from_elsewhere_is_mapped_as_far_as_it_goes_and_sourced_to_the_bundle() {
    let foreign = json!({
        "type": "bundle", "id": "bundle--5d0092c5-5f74-4287-9642-33f4c354e56d",
        "objects": [
            { "type": "identity", "spec_version": "2.1", "id": "identity--f431f809-377b-45e0-aa1c-6a4751cae5ff",
              "created": "2026-01-01T00:00:00.000Z", "modified": "2026-01-01T00:00:00.000Z", "name": "ACME CTI", "identity_class": "organization" },
            { "type": "ipv4-addr", "spec_version": "2.1", "id": "ipv4-addr--ff26c055-6336-5bc5-b98d-13d6226742dd", "value": "198.51.100.7" },
            { "type": "domain-name", "spec_version": "2.1", "id": "domain-name--3c10e93f-798e-5a26-a0c1-08156efab7f5", "value": "C2.Example.NET." },
            { "type": "file", "spec_version": "2.1", "id": "file--9a8e1f26-6e7c-5d3e-9d3d-0a3b6f2a1c11", "hashes": { "MD5": "d41d8cd98f00b204e9800998ecf8427e", "SHA-256": "E3B0C442" } },
            { "type": "relationship", "spec_version": "2.1", "id": "relationship--57b56a43-b8b0-4cba-9deb-34e3e1faed9e",
              "created": "2026-01-02T00:00:00.000Z", "modified": "2026-01-02T00:00:00.000Z", "relationship_type": "resolves-to",
              "source_ref": "domain-name--3c10e93f-798e-5a26-a0c1-08156efab7f5", "target_ref": "ipv4-addr--ff26c055-6336-5bc5-b98d-13d6226742dd" },
            { "type": "observed-data", "spec_version": "2.1", "id": "observed-data--b67d30ff-02ac-498a-92f9-32f845f448cf",
              "created": "2026-01-02T00:00:00.000Z", "modified": "2026-01-02T00:00:00.000Z",
              "first_observed": "2026-01-01T12:00:00Z", "last_observed": "2026-01-01T12:00:00Z", "number_observed": 1,
              "object_refs": ["ipv4-addr--ff26c055-6336-5bc5-b98d-13d6226742dd", "file--9a8e1f26-6e7c-5d3e-9d3d-0a3b6f2a1c11"] },
            { "type": "opinion", "spec_version": "2.1", "id": "opinion--b01efc25-77b4-4003-b18b-f6e24b5cd9f7",
              "created": "2026-01-02T00:00:00.000Z", "modified": "2026-01-02T00:00:00.000Z", "opinion": "agree", "explanation": "matches our telemetry",
              "object_refs": ["domain-name--3c10e93f-798e-5a26-a0c1-08156efab7f5"] },
            { "type": "note", "spec_version": "2.1", "id": "note--3f4e5a2c-1b1d-4d6f-9c5e-7b8a2d1e0f33",
              "created": "2026-01-02T00:00:00.000Z", "modified": "2026-01-02T00:00:00.000Z", "content": "seen in the wild",
              "object_refs": ["ipv4-addr--ff26c055-6336-5bc5-b98d-13d6226742dd"] },
            { "type": "malware", "spec_version": "2.1", "id": "malware--c1b3d7a8-2a4f-4d4e-8f6a-0d9a7b3c5e21",
              "created": "2026-01-02T00:00:00.000Z", "modified": "2026-01-02T00:00:00.000Z", "name": "x", "is_family": true }
        ]
    });
    let (_d, case) = case(None);
    let done = stix::import(&case, &foreign, "acme.json").unwrap();
    assert!(!done.restored);
    assert_eq!(done.skipped.len(), 1, "{:?}", done.skipped);
    assert!(done.skipped[0].contains("malware"), "{:?}", done.skipped);

    let db = case.db();
    let host = query::entity_id(db, "net.domain", "c2.example.net", None).unwrap();
    let ip = query::entity_id(db, "net.ipv4", "198.51.100.7", None).unwrap();
    let sha = query::entity_id(db, "file.hash.sha256", "e3b0c442", None).unwrap();
    for id in [&host, &ip, &sha] {
        assert!(query::get(db, id).unwrap().is_some(), "{id}");
    }
    // The strongest identifier of a file is the one used.
    assert!(
        query::get(
            db,
            &query::entity_id(
                db,
                "file.hash.md5",
                "d41d8cd98f00b204e9800998ecf8427e",
                None
            )
            .unwrap()
        )
        .unwrap()
        .is_none()
    );
    // The relationship is a claim from the domain to the address, sourced to the bundle.
    let rels = query::relations(db, None).unwrap();
    let rel: Vec<_> = rels
        .iter()
        .filter(|r| r.type_ == "stix.resolves-to")
        .collect();
    assert_eq!(rel.len(), 1, "{rels:?}");
    assert_eq!(
        (rel[0].from_id.as_str(), rel[0].to_id.as_str()),
        (host.as_str(), ip.as_str())
    );
    let claim = query::get(db, &rel[0].claim_id).unwrap().unwrap();
    assert_eq!(claim.author, "tool:stix-import@acme.json");
    let source = query::edges_from(db, &claim.id)
        .unwrap()
        .into_iter()
        .find(|g| g.role == "source")
        .unwrap();
    assert_eq!(
        query::get(db, &source.to_id)
            .unwrap()
            .unwrap()
            .content
            .value
            .as_deref(),
        Some("stix:acme.json")
    );
    // Observed data, the opinion and the note are in too.
    let by_type = |t: &str| {
        query::find(
            db,
            &query::Filter {
                type_prefix: Some(t.into()),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let event = &by_type("stix.observed-data")[0];
    assert_eq!(
        event.content.time.occurred_at.as_deref(),
        Some("2026-01-01T12:00:00Z")
    );
    assert_eq!(
        by_type("analysis.judgement")[0]
            .content
            .confidence
            .as_deref(),
        Some("likely")
    );
    assert_eq!(
        by_type("analysis.note")[0].content.value.as_deref(),
        Some("seen in the wild")
    );
    // Not a bundle at all.
    assert!(stix::import(&case, &json!({ "type": "indicator" }), "x").is_err());
    assert!(stix::import(&case, &json!({ "type": "bundle" }), "x").is_err());
}

#[test]
fn the_graph_is_two_csv_tables_that_quote_what_needs_it() {
    let (_d, case) = case(None);
    analysis(&case);
    graph::apply(
        case.db(),
        Batch::new(Author::analyst()).put(Put {
            content: Content::value("a, \"quoted\"\nnote"),
            ..Put::new(Kind::Annotation, "analysis.note")
        }),
    )
    .unwrap();
    let read = |text: &str| {
        csv::Reader::from_reader(text.as_bytes())
            .records()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>()
    };
    let elements = tables::elements(&case, false).unwrap();
    let rows = read(&elements);
    let (all, _) = query::snapshot(case.db(), false).unwrap();
    assert_eq!(rows.len(), all.len());
    assert!(
        rows.iter()
            .any(|r| &r[3] == "a, \"quoted\"\nnote" && &r[2] == "analysis.note")
    );
    let header = csv::Reader::from_reader(elements.as_bytes())
        .headers()
        .unwrap()
        .clone();
    assert_eq!(&header[0], "id");
    assert!(header.iter().any(|h| h == "time_as_written"));
    let edges = tables::edges(&case, false).unwrap();
    let e = read(&edges);
    let (_, graph_edges) = query::snapshot(case.db(), false).unwrap();
    assert_eq!(e.len(), graph_edges.len());
    assert!(
        e.iter()
            .any(|r| &r[4] == "actor" && &r[7] == "jdoe" && &r[6] == "account.user")
    );
    assert!(
        e.iter()
            .any(|r| &r[4] == "source" && &r[9] == "row=1204" && &r[10] == "f1")
    );
}

#[test]
fn the_projection_joins_entities_by_the_claims_they_share_and_leaves_out_what_distorts_it() {
    let (_d, case) = case(None);
    analysis(&case);
    let db = case.db();
    let p = projection::project(db, &Options::default()).unwrap();
    let id = |t: &str, v: &str| query::entity_id(db, t, v, None).unwrap();
    let weight = |a: &str, b: &str| {
        p.edges
            .iter()
            .find(|e| (e.a == a && e.b == b) || (e.a == b && e.b == a))
            .map(|e| e.weight)
    };
    let (jdoe, ws, ip, dom) = (
        id("account.user", "jdoe"),
        id("host.name", "ws-042"),
        id("net.ipv4", "198.51.100.7"),
        id("net.domain", "cdn-sync.example"),
    );
    assert_eq!(weight(&jdoe, &ws), Some(1));
    assert_eq!(weight(&ws, &ip), Some(1));
    assert_eq!(weight(&dom, &ip), Some(1));
    assert_eq!(
        weight(&jdoe, &ip),
        None,
        "no claim joins them: the projection does not invent one"
    );
    // The three-way process claim becomes three edges among its three entities.
    let (word, ps) = (
        id("process.name", "winword.exe"),
        id("process.name", "powershell.exe"),
    );
    assert_eq!(
        (weight(&word, &ps), weight(&word, &jdoe), weight(&ps, &jdoe)),
        (Some(1), Some(1), Some(1))
    );
    // Annotations and sources are no part of it.
    assert!(
        p.nodes
            .iter()
            .all(|n| !n.type_.starts_with("analysis.") && !n.type_.starts_with("external"))
    );

    // A claim with too many participants is skipped, and said so.
    let narrow = projection::project(
        db,
        &Options {
            max_participants: 2,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(narrow.skipped_large, 1);
    assert_eq!(weight_in(&narrow, &word, &ps), None);
    // An entity in many claims can be left out: jdoe is in two, ws-042 in two.
    let lean = projection::project(
        db,
        &Options {
            max_prevalence: Some(1),
            ..Options::default()
        },
    )
    .unwrap();
    assert!(
        lean.skipped_prevalent.contains(&jdoe)
            && lean.skipped_prevalent.contains(&ws)
            && lean.skipped_prevalent.contains(&ip)
    );
    assert_eq!(weight_in(&lean, &dom, &ip), None, "ip is prevalent too");
    // Only some claims.
    let net = projection::project(
        db,
        &Options {
            claim_prefix: Some("net.".into()),
            ..Options::default()
        },
    )
    .unwrap();
    assert!(
        net.edges
            .iter()
            .all(|e| e.types.keys().all(|t| t.starts_with("net.")))
    );
    assert!(weight_in(&net, &jdoe, &ws).is_none());

    let csv_text = projection::edge_list_csv(&p);
    assert!(
        csv_text.starts_with("source,target,weight,claim_types\n")
            && csv_text.contains("net.connection:1")
    );
    let xml = projection::graphml(&p);
    assert!(
        xml.contains("<graphml")
            && xml.contains("edgedefault=\"undirected\"")
            && xml.matches("<node ").count() == p.nodes.len()
    );
}

fn weight_in(p: &projection::Projection, a: &str, b: &str) -> Option<usize> {
    p.edges
        .iter()
        .find(|e| (e.a == a && e.b == b) || (e.a == b && e.b == a))
        .map(|e| e.weight)
}

#[test]
fn candidate_roles_are_uncertain_and_left_out_unless_asked_for() {
    let (_d, case) = case(None);
    let db = case.db();
    graph::apply(
        db,
        Batch::new(Author::analyst()).put(claim(
            Kind::Observation,
            "net.origin",
            vec![
                Link::entity("subject", "net.domain", "bad.example"),
                Link::entity("candidate", "host.name", "ws-001"),
                Link::entity("candidate", "host.name", "ws-002"),
                Link::source("notes.csv"),
            ],
        )),
    )
    .unwrap();
    let plain = projection::project(db, &Options::default()).unwrap();
    assert!(plain.edges.is_empty(), "{:?}", plain.edges);
    let with = projection::project(
        db,
        &Options {
            include_candidates: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(with.edges.len(), 3);
}

/// For checking a bundle with tools this repository does not contain: with
/// `REACTOR_WRITE_BUNDLE=<path>` set, write the rich graph's bundle there. (The OASIS
/// JSON schemas were run over it by hand; see the plan, section 10.5.)
#[test]
fn a_bundle_can_be_written_for_validation_outside() {
    let Ok(path) = std::env::var("REACTOR_WRITE_BUNDLE") else {
        return;
    };
    let (_d, case) = case(Some("Schema check"));
    analysis(&case);
    let bundle = stix::export(&case, &Default::default()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(&bundle).unwrap()).unwrap();
}

#[test]
fn a_hash_that_is_not_one_stays_an_entity_but_is_not_called_a_file() {
    let (_d, case) = case(None);
    graph::apply(
        case.db(),
        Batch::new(Author::analyst()).put(claim(
            Kind::Observation,
            "file.seen",
            vec![
                Link::entity("subject", "file.hash.sha256", "abc123"),
                Link::entity(
                    "object",
                    "file.hash.md5",
                    "d41d8cd98f00b204e9800998ecf8427e",
                ),
                Link::source("notes.csv"),
            ],
        )),
    )
    .unwrap();
    let bundle = stix::export(&case, &Default::default()).unwrap();
    let objects = bundle["objects"].as_array().unwrap();
    // The 32-digit MD5 is a file; the six-digit "SHA-256" is a custom entity, with its value.
    assert!(
        objects.iter().any(
            |o| o["type"] == "file" && o["hashes"]["MD5"] == "d41d8cd98f00b204e9800998ecf8427e"
        )
    );
    assert!(objects.iter().any(|o| o["type"] == "x-reactor-entity"
        && o["entity_type"] == "file.hash.sha256"
        && o["value"] == "abc123"));
    assert!(stix::validate(&bundle).is_empty());
    // And it comes back as what it was.
    let (_e, other) = self::case(None);
    stix::import(&other, &bundle, "x").unwrap();
    assert!(
        query::get(
            other.db(),
            &query::entity_id(other.db(), "file.hash.sha256", "abc123", None).unwrap()
        )
        .unwrap()
        .is_some()
    );
}
