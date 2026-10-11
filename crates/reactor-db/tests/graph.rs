//! The analysis graph: identity, the source rule, versions, and the invariants the
//! database itself holds (ADR-0047).

use reactor_db::graph::{self, Batch, Change, Content, Filter, Kind, Link, Put, Target, query};
use reactor_db::manifest::Author;
use reactor_db::{Case, Db, Error};
use serde_json::json;

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn me() -> Batch {
    Batch::new(Author::analyst())
}

fn claim(kind: Kind, type_: &str, links: Vec<Link>) -> Put {
    Put {
        links,
        ..Put::new(kind, type_)
    }
}

fn count(db: &Db, table: &'static str) -> i64 {
    db.read(|c| Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?))
        .unwrap()
}

/// The graph concept's worked example: a VPN logon, a firewall connection and a passive
/// DNS record, each from its own source, joined only by the entities they share.
fn incident() -> Batch {
    me().put(claim(
        Kind::Event,
        "auth.logon",
        vec![
            Link::entity("actor", "account.user", "jdoe"),
            Link::entity("target", "host.name", "WS-042"),
            Link::source("sqlite:reactor_data.sqlite?table=vpn_auth").at("row=1204", Some("f1")),
        ],
    ))
    .put(claim(
        Kind::Event,
        "net.connection",
        vec![
            Link::entity("src", "host.name", "ws-042"),
            Link::entity("dst", "net.ipv4", "198.51.100.7"),
            Link::source("sqlite:reactor_data.sqlite?table=fw_conn"),
        ],
    ))
    .put(claim(
        Kind::Observation,
        "net.resolution",
        vec![
            Link::entity("query", "net.domain", "CDN-Sync.Example."),
            Link::entity("answer", "ipv4", "::ffff:198.51.100.7"),
            Link::source("csv:data/pdns.csv").at("row=7", None),
        ],
    ))
}

#[test]
fn claims_from_three_sources_join_on_the_entities_they_share() {
    let (_d, case) = case();
    let db = case.db();
    let applied = graph::apply(db, incident()).unwrap();
    assert_eq!(applied.ids.len(), 3);
    assert!(applied.event_seq.is_some());
    // 3 claims + 4 entities (jdoe, ws-042, the address, the domain) + 3 externals.
    assert_eq!(applied.created, 10);
    assert_eq!(count(db, "element"), 10);
    assert_eq!(count(db, "edge"), 9);

    // "WS-042" and "ws-042" are one host; "ipv4" is an alias; the mapped IPv6 address
    // is the IPv4 one; the domain lost its case and its trailing dot.
    let host = query::entity_id(db, "host.name", "Ws-042", None).unwrap();
    let ip = query::entity_id(db, "net.ipv4", "198.51.100.7", None).unwrap();
    let domain = query::entity_id(db, "domain", "cdn-sync.example", None).unwrap();
    let user = query::entity_id(db, "account.user", "jdoe", None).unwrap();
    assert_eq!(
        query::get(db, &domain)
            .unwrap()
            .unwrap()
            .content
            .value
            .as_deref(),
        Some("cdn-sync.example")
    );
    assert_eq!(query::get(db, &ip).unwrap().unwrap().type_, "net.ipv4");
    assert_eq!(query::edges_to(db, &host).unwrap().len(), 2);
    assert_eq!(query::edges_to(db, &ip).unwrap().len(), 2);

    // As relations, drawn in the direction the roles give: jdoe → host → address, and
    // domain → address.
    let rel = query::relations(db, None).unwrap();
    let pairs: Vec<(&str, &str, &str)> = rel
        .iter()
        .map(|r| (r.from_id.as_str(), r.to_id.as_str(), r.type_.as_str()))
        .collect();
    assert_eq!(pairs.len(), 3);
    assert!(pairs.contains(&(&user, &host, "auth.logon")));
    assert!(pairs.contains(&(&host, &ip, "net.connection")));
    assert!(pairs.contains(&(&domain, &ip, "net.resolution")));
    assert_eq!(query::relations(db, Some(&user)).unwrap().len(), 1);

    // The source edge carries where in the external the claim came from.
    let logon = &applied.ids[0];
    let source = query::edges_from(db, logon)
        .unwrap()
        .into_iter()
        .find(|e| e.role == "source")
        .unwrap();
    assert_eq!(
        (source.locator.as_deref(), source.fingerprint.as_deref()),
        (Some("row=1204"), Some("f1"))
    );
    let external = query::get(db, &source.to_id).unwrap().unwrap();
    assert_eq!(external.kind, Kind::External);
    assert_eq!(external.type_, "external.sqlite");
}

#[test]
fn saying_it_again_writes_nothing() {
    let (_d, case) = case();
    let db = case.db();
    // A pull gives its rows ids from where they came from, so a re-pull lands on them.
    let pulled = || {
        let mut b = incident();
        for (i, p) in b.puts.iter_mut().enumerate() {
            p.id = Some(graph::identity::derived_id(
                p.kind,
                &["pull", &i.to_string()],
            ));
        }
        b
    };
    let first = graph::apply(db, pulled()).unwrap();
    let before = (
        count(db, "element_version"),
        count(db, "edge_version"),
        count(db, "event"),
    );
    let again = graph::apply(db, pulled()).unwrap();
    assert_eq!(again.ids, first.ids);
    assert_eq!(
        (again.event_seq, again.created, again.versioned),
        (None, 0, 0)
    );
    assert_eq!(
        before,
        (
            count(db, "element_version"),
            count(db, "edge_version"),
            count(db, "event")
        )
    );
}

#[test]
fn an_event_or_observation_without_a_source_is_refused_whole() {
    let (_d, case) = case();
    let db = case.db();
    let batch = me().put(Put::entity("net.domain", "ok.example")).put(claim(
        Kind::Observation,
        "net.resolution",
        vec![Link::entity("query", "net.domain", "ok.example")],
    ));
    let err = graph::apply(db, batch).unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err}");
    assert!(err.to_string().contains("needs a source"), "{err}");
    // Nothing of the batch stayed, not even the entity that was fine on its own.
    assert_eq!(count(db, "element"), 0);
    assert_eq!(count(db, "event"), 0);

    // What someone concludes is an annotation, and needs none.
    let ok = me().put(claim(
        Kind::Annotation,
        "analysis.hypothesis",
        vec![Link::entity("about", "account.user", "jdoe")],
    ));
    graph::apply(db, ok).unwrap();

    // Taking the source away later is refused too.
    let applied = graph::apply(db, incident()).unwrap();
    let logon = applied.ids[0].clone();
    let source = query::edges_from(db, &logon)
        .unwrap()
        .into_iter()
        .find(|e| e.role == "source")
        .unwrap();
    let strip = me().change(Change {
        id: logon,
        unlink: vec![("source".into(), source.to_id)],
        ..Change::default()
    });
    assert!(graph::apply(db, strip).is_err());
}

#[test]
fn the_database_itself_refuses_what_the_model_forbids() {
    // For this code and for anything else that opens the file.
    let (_d, case) = case();
    let db = case.db();
    let applied = graph::apply(db, incident()).unwrap();
    let user = query::entity_id(db, "account.user", "jdoe", None).unwrap();
    let host = query::entity_id(db, "host.name", "ws-042", None).unwrap();
    let logon = applied.ids[0].clone();

    let raw = |sql: String| {
        db.write(move |tx| Ok(tx.execute(&sql, [])?))
            .unwrap_err()
            .to_string()
    };
    let e = raw(format!(
        "INSERT INTO edge(id, from_id, to_id) VALUES ('edge--x', '{user}', '{host}')"
    ));
    assert!(e.contains("no edges between entities"), "{e}");
    let e = raw(format!(
        "INSERT INTO edge(id, from_id, to_id) VALUES ('edge--y', '{user}', '{logon}')"
    ));
    assert!(e.contains("only outgoing edge is to its source"), "{e}");
    let e = raw("UPDATE element_version SET value = 'rewritten'".into());
    assert!(e.contains("never overwritten"), "{e}");
    let e = raw("UPDATE edge_version SET role = 'dst'".into());
    assert!(e.contains("never overwritten"), "{e}");
    let e = raw("UPDATE element SET type = 'other'".into());
    assert!(e.contains("never change"), "{e}");

    // The same rules through the front door.
    let e = graph::apply(
        db,
        me().put(Put::entity("account.user", "jdoe").link(Link::new("knows", Target::Id(host)))),
    )
    .unwrap_err();
    assert!(e.to_string().contains("no edges between entities"), "{e}");
    let e = graph::apply(
        db,
        me().put(claim(
            Kind::Annotation,
            "analysis.note",
            vec![Link::new("source", Target::Id(user))],
        )),
    )
    .unwrap_err();
    assert!(e.to_string().contains("a source is an external"), "{e}");
}

#[test]
fn a_change_is_a_new_version_and_the_past_stays_readable() {
    let (_d, case) = case();
    let db = case.db();
    let applied = graph::apply(db, incident()).unwrap();
    let logon = applied.ids[0].clone();
    let v1 = query::get(db, &logon).unwrap().unwrap();
    assert_eq!((v1.ver, v1.author.as_str()), (1, "analyst:user"));
    std::thread::sleep(std::time::Duration::from_millis(3));

    let agent = Batch::new(Author::agent("anthropic/claude")).change(Change {
        id: logon.clone(),
        status: Some("conflicting".into()),
        confidence: Some(Some("unlikely".into())),
        set_props: json!({"note": "clock skew on the VPN"})
            .as_object()
            .unwrap()
            .clone(),
        add_tags: vec!["Phase.Initial-Access".into(), "todo".into()],
        ..Change::default()
    });
    let done = graph::apply(db, agent).unwrap();
    assert_eq!((done.created, done.versioned), (0, 1));

    let v2 = query::get(db, &logon).unwrap().unwrap();
    assert_eq!(v2.ver, 2);
    assert_eq!(v2.author, "agent:reactor@anthropic/claude");
    assert_eq!(v2.content.status, "conflicting");
    assert_eq!(v2.content.confidence.as_deref(), Some("unlikely"));
    assert_eq!(v2.content.tags, ["phase.initial-access", "todo"]);
    assert!(v2.recorded_at > v1.recorded_at);

    // History, and the state as of before the change.
    let history = query::history(db, &logon).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0], v1);
    assert_eq!(
        query::get_as_of(db, &logon, &v1.recorded_at)
            .unwrap()
            .unwrap(),
        v1
    );
    assert_eq!(
        query::get_as_of(db, &logon, &v2.recorded_at)
            .unwrap()
            .unwrap(),
        v2
    );
    assert!(
        query::get_as_of(db, &logon, "2000-01-01T00:00:00.000Z")
            .unwrap()
            .is_none()
    );

    // The same change again is no change; an unknown status is refused.
    let same = me().change(Change {
        id: logon.clone(),
        status: Some("conflicting".into()),
        ..Change::default()
    });
    assert_eq!(graph::apply(db, same).unwrap().event_seq, None);
    let bad = me().change(Change {
        id: logon.clone(),
        status: Some("deleted".into()),
        ..Change::default()
    });
    assert!(graph::apply(db, bad).is_err());

    // Soft-delete hides; nothing is lost.
    let hide = me().change(Change {
        id: logon.clone(),
        status: Some("soft-deleted".into()),
        ..Change::default()
    });
    graph::apply(db, hide).unwrap();
    assert_eq!(query::relations(db, None).unwrap().len(), 2);
    let events = Filter {
        kind: Some(Kind::Event),
        ..Filter::default()
    };
    assert_eq!(query::find(db, &events).unwrap().len(), 1);
    let all = Filter {
        include_deleted: true,
        ..events
    };
    assert_eq!(query::find(db, &all).unwrap().len(), 2);
    assert_eq!(query::history(db, &logon).unwrap().len(), 3);
}

#[test]
fn an_entitys_key_never_changes_and_scope_makes_a_different_entity() {
    let (_d, case) = case();
    let db = case.db();
    let applied = graph::apply(
        db,
        me().put(Put::entity("net.ipv4", "10.0.0.5"))
            .put(Put {
                scope: Some("site-a".into()),
                ..Put::entity("net.ipv4", "10.0.0.5")
            })
            .put(Put {
                scope: Some("site-b".into()),
                ..Put::entity("net.ipv4", "10.0.0.5")
            }),
    )
    .unwrap();
    assert_eq!(
        applied.created, 3,
        "the same private address in three contexts"
    );
    assert_eq!(
        applied.ids[1],
        query::entity_id(db, "net.ipv4", "10.0.0.5", Some("site-a")).unwrap()
    );

    for change in [
        Change {
            id: applied.ids[0].clone(),
            value: Some(Some("10.0.0.6".into())),
            ..Change::default()
        },
        Change {
            id: applied.ids[1].clone(),
            set_props: json!({"scope": "site-c"}).as_object().unwrap().clone(),
            ..Change::default()
        },
    ] {
        let e = graph::apply(db, me().change(change)).unwrap_err();
        assert!(e.to_string().contains("never changes"), "{e}");
    }
    // Something that is not a key may change.
    let label = Change {
        id: applied.ids[0].clone(),
        set_props: json!({"label": "jump host"}).as_object().unwrap().clone(),
        ..Change::default()
    };
    assert_eq!(graph::apply(db, me().change(label)).unwrap().versioned, 1);

    // A type with several key fields is keyed on all of them; without them it is a
    // one-off, typed in by hand.
    let instance = |pid: u32| Put {
        content: Content {
            props: json!({"host": "ws-042", "pid": pid, "start": "2026-10-01T03:12:07Z"})
                .as_object()
                .unwrap()
                .clone(),
            ..Content::default()
        },
        ..Put::new(Kind::Entity, "process.instance")
    };
    let a = graph::apply(
        db,
        me().put(instance(4312))
            .put(instance(4312))
            .put(instance(77)),
    )
    .unwrap();
    assert_eq!(a.ids[0], a.ids[1]);
    assert_ne!(a.ids[0], a.ids[2]);
    let loose = graph::apply(
        db,
        me().put(Put::new(Kind::Entity, "process.instance"))
            .put(Put::new(Kind::Entity, "process.instance")),
    )
    .unwrap();
    assert_ne!(loose.ids[0], loose.ids[1]);
}

#[test]
fn an_annotation_can_pin_what_it_judged_and_edges_can_be_taken_away() {
    let (_d, case) = case();
    let db = case.db();
    let applied = graph::apply(db, incident()).unwrap();
    let resolution = applied.ids[2].clone();

    let judged = graph::apply(
        db,
        me().put(Put {
            content: Content {
                value: Some("Account jdoe is compromised".into()),
                ..Content::default()
            },
            ..Put::new(Kind::Annotation, "analysis.hypothesis")
        })
        .put(Put {
            content: Content {
                value: Some("Resolution overlaps the beacon window".into()),
                confidence: Some("likely".into()),
                ..Content::default()
            },
            links: vec![Link::new("about", Target::Id(resolution.clone())).pinned()],
            ..Put::new(Kind::Annotation, "analysis.judgement")
        }),
    )
    .unwrap();
    let (hypothesis, judgement) = (judged.ids[0].clone(), judged.ids[1].clone());
    let about = &query::edges_from(db, &judgement).unwrap()[0];
    assert_eq!((about.role.as_str(), about.pinned_ver), ("about", Some(1)));

    // Link it to the hypothesis afterwards, then take that back.
    let support = me().change(Change {
        id: judgement.clone(),
        links: vec![Link::new("supports", Target::Id(hypothesis.clone()))],
        ..Change::default()
    });
    assert_eq!(graph::apply(db, support).unwrap().versioned, 1);
    assert_eq!(query::edges_to(db, &hypothesis).unwrap().len(), 1);
    let retract = me().change(Change {
        id: judgement.clone(),
        unlink: vec![("supports".into(), hypothesis.clone())],
        ..Change::default()
    });
    graph::apply(db, retract).unwrap();
    assert!(query::edges_to(db, &hypothesis).unwrap().is_empty());
    assert_eq!(query::edges_from(db, &judgement).unwrap().len(), 1);
    // The edge's own history is kept: made, then removed.
    assert_eq!(count(db, "edge_version") - count(db, "edge"), 1);

    let missing = me().change(Change {
        id: judgement,
        unlink: vec![("refutes".into(), hypothesis)],
        ..Change::default()
    });
    assert!(graph::apply(db, missing).is_err());
}

#[test]
fn a_claim_with_three_participants_is_not_a_relation_and_orphans_are_found() {
    let (_d, case) = case();
    let db = case.db();
    let applied = graph::apply(
        db,
        me().put(claim(
            Kind::Event,
            "process.start",
            vec![
                Link::entity("parent", "process.name", "winword.exe"),
                Link::entity("child", "process.name", "powershell.exe"),
                Link::entity("actor", "account.user", "jdoe"),
                Link::source("samples/edr-export.json"),
            ],
        ))
        .put(Put::entity("net.domain", "never-mentioned.example")),
    )
    .unwrap();
    assert!(query::relations(db, None).unwrap().is_empty());
    let orphans = query::orphans(db).unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].id, applied.ids[1]);

    let types = query::types_in_use(db).unwrap();
    let unknown: Vec<&str> = types
        .iter()
        .filter(|t| !t.known)
        .map(|t| t.type_.as_str())
        .collect();
    assert_eq!(unknown, ["external.file"]);
    assert_eq!(
        types
            .iter()
            .find(|t| t.type_ == "process.name")
            .unwrap()
            .count,
        2
    );

    let by_prefix = Filter {
        type_prefix: Some("process.".into()),
        ..Filter::default()
    };
    assert_eq!(query::find(db, &by_prefix).unwrap().len(), 3);
    let by_text = Filter {
        text: Some("powershell".into()),
        ..Filter::default()
    };
    assert_eq!(query::find(db, &by_text).unwrap().len(), 1);

    let (elements, edges) = query::snapshot(db, false).unwrap();
    assert_eq!((elements.len(), edges.len()), (6, 4));
}

#[test]
fn an_external_stays_inside_the_case() {
    let (_d, case) = case();
    for uri in [
        "file:///etc/passwd",
        "../elsewhere/x.csv",
        "https://u:p@host/x",
    ] {
        let e = graph::apply(
            case.db(),
            me().put(claim(Kind::Event, "x.seen", vec![Link::source(uri)])),
        )
        .unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{uri}: {e}");
    }
    assert_eq!(count(case.db(), "element"), 0);
}

#[test]
fn the_vocabulary_is_seeded_as_rows() {
    let (_d, case) = case();
    let db = case.db();
    assert!(count(db, "known_type") > 150);
    assert!(count(db, "type_namespace") > 20);
    assert_eq!(count(db, "known_role"), 19);
    let scale: Vec<String> = db
        .read(|c| {
            let mut s =
                c.prepare("SELECT name FROM scale_value WHERE scale = 'confidence' ORDER BY ord")?;
            let rows = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(scale.first().map(String::as_str), Some("almost no chance"));
    assert_eq!(scale.last().map(String::as_str), Some("almost certain"));
    // And it can grow without a release.
    db.write(|tx| {
        tx.execute(
            "INSERT INTO known_type(name, kind, definition, normalizer) VALUES ('ot.plc', 'entity', 'A programmable logic controller.', 'lowercase')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let a = query::entity_id(db, "ot.plc", "Line-3", None).unwrap();
    assert_eq!(a, query::entity_id(db, "ot.plc", "line-3", None).unwrap());
}

#[test]
fn node_positions_are_kept_with_the_case_and_replaced_by_the_next() {
    use reactor_db::graph::layout;
    let dir = tempfile::tempdir().unwrap();
    let case = reactor_db::Case::create(dir.path(), None).unwrap();
    let db = case.db();
    assert!(layout::load(db).unwrap().is_empty());

    layout::save(db, &[("a".into(), (1.5, -2.0)), ("b".into(), (10.0, 20.0))]).unwrap();
    // A later position replaces the earlier; one that cannot be drawn is not stored.
    layout::save(
        db,
        &[("a".into(), (3.0, 4.0)), ("c".into(), (f32::NAN, 0.0))],
    )
    .unwrap();
    let got = layout::load(db).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got["a"], (3.0, 4.0));
    assert_eq!(got["b"], (10.0, 20.0));

    // They are there when the case is opened again, and are not part of the graph's history.
    let root = case.root().to_path_buf();
    drop(case);
    let again = reactor_db::Case::open(&root).unwrap();
    assert_eq!(layout::load(again.db()).unwrap().len(), 2);
    layout::clear(again.db()).unwrap();
    assert!(layout::load(again.db()).unwrap().is_empty());
}
