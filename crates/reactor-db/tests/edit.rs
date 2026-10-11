//! What a person does by hand in the graph view (ADR-0047).

use reactor_db::graph::edit::{self, At};
use reactor_db::graph::{Kind, query};
use reactor_db::{Case, Error};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn at() -> At {
    At::new("s1", 12)
}

fn hypothesis(case: &Case) -> String {
    use reactor_db::graph::{Batch, Content, Put};
    use reactor_db::manifest::Author;
    let put = Put {
        content: Content::value("jdoe is compromised"),
        ..Put::new(Kind::Annotation, "analysis.hypothesis")
    };
    reactor_db::graph::apply(case.db(), Batch::new(Author::analyst()).put(put))
        .unwrap()
        .ids
        .remove(0)
}

#[test]
fn an_entity_added_by_hand_is_the_same_node_as_one_found_anywhere() {
    let (_d, case) = case();
    let db = case.db();
    let a = edit::add_entity(db, &at(), "ipv4", " 198.51.100.7 ").unwrap();
    assert_eq!(
        a,
        query::entity_id(db, "net.ipv4", "198.51.100.7", None).unwrap()
    );
    assert_eq!(
        edit::add_entity(db, &at(), "net.ipv4", "198.51.100.7").unwrap(),
        a
    );
    let e = query::get(db, &a).unwrap().unwrap();
    assert_eq!(
        (e.type_.as_str(), e.author.as_str()),
        ("net.ipv4", "analyst:user")
    );
    assert!(matches!(
        edit::add_entity(db, &at(), "net.ipv4", "  "),
        Err(Error::Invalid(_))
    ));
    assert!(edit::add_entity(db, &at(), "", "x").is_err());
}

#[test]
fn a_relation_stated_by_hand_is_sourced_to_the_entry_it_was_made_in() {
    let (_d, case) = case();
    let db = case.db();
    let host = edit::add_entity(db, &at(), "host.name", "ws-042").unwrap();
    let ip = edit::add_entity(db, &at(), "net.ipv4", "198.51.100.7").unwrap();
    let claim = edit::relate(db, &at(), &host, &ip, "net.connection", "", "").unwrap();

    let source = query::edges_from(db, &claim)
        .unwrap()
        .into_iter()
        .find(|e| e.role == "source")
        .unwrap();
    let external = query::get(db, &source.to_id).unwrap().unwrap();
    assert_eq!(
        external.content.value.as_deref(),
        Some("session:s1#entry=12")
    );
    // Drawn from the source to the destination, because that is what the roles say.
    let rel = query::relations(db, None).unwrap();
    assert_eq!(
        (rel[0].from_id.as_str(), rel[0].to_id.as_str()),
        (host.as_str(), ip.as_str())
    );

    for (from, to, ty, fr, tr) in [
        (&host, &host, "x.y", "src", "dst"),
        (&host, &ip, " ", "src", "dst"),
        (&host, &ip, "x.y", "src", "SRC"),
    ] {
        assert!(matches!(
            edit::relate(db, &at(), from, to, ty, fr, tr),
            Err(Error::Invalid(_))
        ));
    }
    assert!(edit::relate(db, &at(), &host, "entity--nope", "x.y", "src", "dst").is_err());
}

#[test]
fn confidence_tags_and_status_change_each_selected_element_and_say_nothing_twice() {
    let (_d, case) = case();
    let db = case.db();
    let a = edit::add_entity(db, &at(), "net.domain", "a.example").unwrap();
    let b = edit::add_entity(db, &at(), "net.domain", "b.example").unwrap();
    let both = [a.clone(), b.clone()];

    assert_eq!(
        edit::scale(db, "confidence").unwrap(),
        [
            "almost no chance",
            "very unlikely",
            "unlikely",
            "roughly even",
            "likely",
            "very likely",
            "almost certain"
        ]
    );
    let done = edit::set_confidence(db, &at(), &both, Some("likely")).unwrap();
    assert_eq!(done.versioned, 2);
    assert_eq!(
        edit::set_confidence(db, &at(), &both, Some("likely"))
            .unwrap()
            .event_seq,
        None
    );
    edit::set_confidence(db, &at(), std::slice::from_ref(&a), None).unwrap();
    assert_eq!(
        query::get(db, &a).unwrap().unwrap().content.confidence,
        None
    );
    assert_eq!(
        query::get(db, &b)
            .unwrap()
            .unwrap()
            .content
            .confidence
            .as_deref(),
        Some("likely")
    );

    edit::tag(
        db,
        &at(),
        &both,
        &["Phase.Recon".into(), "todo".into()],
        &[],
    )
    .unwrap();
    edit::tag(db, &at(), &both, &[], &["TODO".into()]).unwrap();
    assert_eq!(
        query::get(db, &a).unwrap().unwrap().content.tags,
        ["phase.recon"]
    );

    edit::set_status(db, &at(), std::slice::from_ref(&b), "conflicting").unwrap();
    assert_eq!(
        query::get(db, &b).unwrap().unwrap().content.status,
        "conflicting"
    );
    assert!(edit::set_status(db, &at(), &[b], "deleted").is_err());
    assert!(edit::set_status(db, &at(), &[], "active").is_err());
}

#[test]
fn only_a_hypothesis_has_a_hypothesis_status() {
    let (_d, case) = case();
    let db = case.db();
    let h = hypothesis(&case);
    edit::set_hypothesis_status(db, &at(), &h, "supported").unwrap();
    let e = query::get(db, &h).unwrap().unwrap();
    assert_eq!(e.content.props["hypothesis_status"], "supported");
    assert_eq!(e.ver, 2);
    assert!(edit::set_hypothesis_status(db, &at(), &h, "maybe").is_err());
    let host = edit::add_entity(db, &at(), "host.name", "ws-042").unwrap();
    let err = edit::set_hypothesis_status(db, &at(), &host, "open").unwrap_err();
    assert!(err.to_string().contains("not a hypothesis"), "{err}");
}

#[test]
fn undo_takes_back_the_latest_version_with_a_preview_and_refuses_what_is_built_on() {
    let (_d, case) = case();
    let db = case.db();
    let a = edit::add_entity(db, &at(), "net.domain", "a.example").unwrap();
    assert!(
        edit::undo_preview(db, &a)
            .unwrap_err()
            .to_string()
            .contains("first version")
    );

    edit::set_confidence(db, &at(), std::slice::from_ref(&a), Some("likely")).unwrap();
    let plan = edit::undo_preview(db, &a).unwrap();
    assert_eq!((plan.elements.len(), plan.elements[0].back_to), (1, 1));
    assert_eq!(
        query::get(db, &a).unwrap().unwrap().ver,
        2,
        "a preview changes nothing"
    );
    edit::undo(db, &at(), &a).unwrap();
    assert_eq!(
        query::get(db, &a).unwrap().unwrap().content.confidence,
        None
    );

    // A note that judged the corrected version stands in the way.
    use reactor_db::graph::{Batch, Content, Link, Put, Target};
    use reactor_db::manifest::Author;
    edit::set_confidence(db, &at(), std::slice::from_ref(&a), Some("unlikely")).unwrap();
    reactor_db::graph::apply(
        db,
        Batch::new(Author::analyst()).put(Put {
            content: Content::value("this is the CDN"),
            links: vec![Link::new("about", Target::Id(a.clone())).pinned()],
            ..Put::new(Kind::Annotation, "analysis.judgement")
        }),
    )
    .unwrap();
    let err = edit::undo(db, &at(), &a).unwrap_err();
    assert!(err.to_string().contains("builds on"), "{err}");
    // Version 2 again, after the undo and a new change: still there.
    assert_eq!(query::get(db, &a).unwrap().unwrap().ver, 2);
}

#[test]
fn a_note_is_about_everything_selected() {
    let (_d, case) = case();
    let db = case.db();
    let a = edit::add_entity(db, &at(), "net.domain", "a.example").unwrap();
    let b = edit::add_entity(db, &at(), "net.domain", "b.example").unwrap();
    let note = edit::add_note(db, &at(), &[a.clone(), b.clone()], "  same registrar  ").unwrap();
    assert_eq!(
        query::get(db, &note)
            .unwrap()
            .unwrap()
            .content
            .value
            .as_deref(),
        Some("same registrar")
    );
    assert_eq!(query::edges_to(db, &a).unwrap().len(), 1);
    assert_eq!(query::edges_to(db, &b).unwrap().len(), 1);
    assert!(edit::add_note(db, &at(), &[a], " ",).is_err());
    assert!(edit::add_note(db, &at(), &[], "x").is_err());
    assert!(
        edit::known_types(db, Kind::Entity)
            .unwrap()
            .contains(&"net.ipv4".to_string())
    );
}

#[test]
fn a_note_may_be_about_nothing_and_a_connection_may_link_many() {
    let (_d, case) = case();
    let db = case.db();
    let free = edit::note(db, &at(), &[], "  check the registry later ").unwrap();
    assert_eq!(
        query::get(db, &free)
            .unwrap()
            .unwrap()
            .content
            .value
            .as_deref(),
        Some("check the registry later")
    );
    assert!(query::edges_from(db, &free).unwrap().is_empty());
    assert!(edit::note(db, &at(), &[], " ").is_err());
    assert!(
        edit::add_note(db, &at(), &[], "x").is_err(),
        "a note on a selection needs one"
    );

    let hosts: Vec<String> = ["ws-1", "ws-2", "ws-3"]
        .iter()
        .map(|h| edit::add_entity(db, &at(), "host.name", h).unwrap())
        .collect();
    let claim = edit::relate_many(db, &at(), &hosts, "analysis.same_campaign", "").unwrap();
    let links = query::edges_from(db, &claim).unwrap();
    assert_eq!(links.iter().filter(|g| g.role == "member").count(), 3);
    assert_eq!(links.iter().filter(|g| g.role == "source").count(), 1);
    // Three participants: not a relation line, and not refused.
    assert!(query::relations(db, None).unwrap().is_empty());
    assert!(
        edit::relate_many(
            db,
            &at(),
            &[hosts[0].clone(), hosts[0].clone()],
            "x.y",
            "member"
        )
        .is_err()
    );
    assert!(edit::relate_many(db, &at(), &hosts, "  ", "member").is_err());
}

#[test]
fn types_are_offered_as_they_are_typed_most_used_first_and_found_by_alias() {
    let (_d, case) = case();
    let db = case.db();
    edit::add_entity(db, &at(), "net.domain", "a.example").unwrap();
    edit::add_entity(db, &at(), "net.domain", "b.example").unwrap();
    edit::add_entity(db, &at(), "net.ipv4", "198.51.100.7").unwrap();
    let choices = edit::type_choices(db, &[Kind::Entity]).unwrap();
    let uses = |n: &str| choices.iter().find(|c| c.name == n).unwrap().uses;
    assert_eq!(
        (uses("net.domain"), uses("net.ipv4"), uses("net.ipv6")),
        (2, 1, 0)
    );
    assert!(
        choices
            .iter()
            .find(|c| c.name == "net.ipv4")
            .unwrap()
            .aliases
            .contains(&"ip".to_string())
    );
    assert!(
        choices.iter().all(|c| !c.name.starts_with("analysis.")),
        "annotations are not entity types"
    );

    let s = |typed: &str| edit::suggest_types(&choices, typed, 5);
    // The ones in use first, then the rest by name.
    assert_eq!(s("net.")[..3], ["net.domain", "net.ipv4", "net.asn"]);
    assert_eq!(s("ipv"), ["net.ipv4", "net.ipv6"]);
    // What begins with it, then what has a part that does.
    assert_eq!(s("mail")[0], "mail.message");
    assert!(s("mail").contains(&"net.email".to_string()));
    assert!(s("hash")[0].starts_with("file.hash."));
    assert_eq!(s("ip")[0], "net.ipv4");
    assert!(s("net.ipv4").is_empty());
    assert!(s("zzzz").is_empty());
    assert_eq!(s("")[..2], ["net.domain", "net.ipv4"]);
    assert_eq!(edit::suggest_types(&choices, "", 3).len(), 3);
    let claims = edit::type_choices(db, &[Kind::Event, Kind::Observation]).unwrap();
    assert!(claims.iter().any(|c| c.name == "analysis.pull"));
    assert!(claims.iter().any(|c| c.name == "net.connection"));
    assert!(claims.iter().all(|c| c.name != "net.ipv4"));
}

#[test]
fn what_a_person_stated_has_the_session_entry_as_its_source() {
    let (_d, case) = case();
    let db = case.db();
    let at = At::new("3", 7);
    let a = edit::add_entity(db, &at, "net.ipv4", "10.0.0.5").unwrap();
    let b = edit::add_entity(db, &at, "net.domain", "evil.example").unwrap();
    let claim = edit::relate(db, &at, &a, &b, "net.resolution", "query", "answer").unwrap();
    let sources = query::sources_of(db, &claim).unwrap();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(sources[0].uri, "session:3#entry=7");
    // An entity states nothing and so has no source of its own.
    assert!(query::sources_of(db, &a).unwrap().is_empty());
}

#[test]
fn each_kind_of_annotation_can_be_added_by_hand_with_what_it_needs() {
    use reactor_db::graph::edit::{Annotate, AnnotationKind::*};
    let (_d, case) = case();
    let db = case.db();
    let a = edit::add_entity(db, &at(), "host.name", "ws-042").unwrap();
    let b = edit::add_entity(db, &at(), "host.name", "WS042.corp").unwrap();
    let c = edit::add_entity(db, &at(), "account.user", "jdoe").unwrap();
    let ids = |v: &[&String]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let none: &[String] = &[];

    // A hypothesis needs only its text, and starts open.
    assert!(edit::annotate(db, &at(), Hypothesis, &Annotate::default()).is_err());
    let h = edit::annotate(
        db,
        &at(),
        Hypothesis,
        &Annotate {
            text: "jdoe was phished",
            ..Default::default()
        },
    )
    .unwrap();
    let e = query::get(db, &h).unwrap().unwrap();
    assert_eq!(
        (e.type_.as_str(), e.kind),
        ("analysis.hypothesis", Kind::Annotation)
    );
    assert_eq!(e.content.props["hypothesis_status"], "open");

    // An assessment is about something, and may support a hypothesis, with a confidence.
    let about = ids(&[&c]);
    let supports = vec![h.clone()];
    assert!(
        edit::annotate(
            db,
            &at(),
            Judgement,
            &Annotate {
                text: "x",
                ..Default::default()
            }
        )
        .is_err(),
        "about nothing"
    );
    let j = edit::annotate(
        db,
        &at(),
        Judgement,
        &Annotate {
            text: "the logon came from the phishing IP",
            about: &about,
            supports: &supports,
            confidence: Some("likely"),
            ..Default::default()
        },
    )
    .unwrap();
    let je = query::get(db, &j).unwrap().unwrap();
    assert_eq!(je.content.confidence.as_deref(), Some("likely"));
    let out = query::edges_from(db, &j).unwrap();
    let roles: Vec<(&str, &str)> = out
        .iter()
        .map(|g| (g.role.as_str(), g.to_id.as_str()))
        .collect();
    assert!(
        roles.contains(&("about", c.as_str())) && roles.contains(&("supports", h.as_str())),
        "{roles:?}"
    );
    // About something is pinned to the version it judged.
    assert!(
        out.iter()
            .find(|g| g.role == "about")
            .unwrap()
            .pinned_ver
            .is_some()
    );

    // A cluster needs two members; a same-as needs exactly two entities.
    assert!(
        edit::annotate(
            db,
            &at(),
            Cluster,
            &Annotate {
                members: &ids(&[&a]),
                ..Default::default()
            }
        )
        .is_err()
    );
    let cl = edit::annotate(
        db,
        &at(),
        Cluster,
        &Annotate {
            text: "the workstation",
            members: &ids(&[&a, &b]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        query::edges_from(db, &cl)
            .unwrap()
            .iter()
            .filter(|g| g.role == "member")
            .count(),
        2
    );
    assert!(
        edit::annotate(
            db,
            &at(),
            SameAs,
            &Annotate {
                about: &ids(&[&a, &b, &c]),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        edit::annotate(
            db,
            &at(),
            SameAs,
            &Annotate {
                about: &ids(&[&a]),
                ..Default::default()
            }
        )
        .is_err()
    );
    let same = edit::annotate(
        db,
        &at(),
        SameAs,
        &Annotate {
            about: &ids(&[&a, &b]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        query::get(db, &same).unwrap().unwrap().type_,
        "analysis.same_as"
    );

    // A search that found nothing says what for and where.
    assert!(edit::annotate(db, &at(), SearchNegative, &Annotate::default()).is_err());
    let sn = edit::annotate(
        db,
        &at(),
        SearchNegative,
        &Annotate {
            text: "ws-042 in the proxy logs",
            sources: &["logs/proxy.csv".to_string(), "  ".to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    let from_sn = query::edges_from(db, &sn).unwrap();
    assert_eq!(
        from_sn.iter().filter(|g| g.role == "source").count(),
        1,
        "a blank is not a place"
    );
    let _ = none;
}

#[test]
fn the_text_of_a_hypothesis_can_be_rewritten_and_the_old_text_is_kept() {
    let (_d, case) = case();
    let db = case.db();
    let h = hypothesis(&case);

    let done = edit::set_text(
        db,
        &at(),
        &h,
        "  jdoe is compromised, and since March\n\nSee the logon events.  ",
    )
    .unwrap();
    assert_eq!(done.versioned, 1);
    let now = query::get(db, &h).unwrap().unwrap();
    assert_eq!(now.ver, 2);
    assert_eq!(
        now.content.value.as_deref(),
        Some("jdoe is compromised, and since March\n\nSee the logon events.")
    );
    let history = query::history(db, &h).unwrap();
    assert_eq!(
        history[0].content.value.as_deref(),
        Some("jdoe is compromised")
    );

    // The same text again changes nothing.
    let again = edit::set_text(
        db,
        &at(),
        &h,
        "jdoe is compromised, and since March\n\nSee the logon events.",
    )
    .unwrap();
    assert!(again.event_seq.is_none());
    assert_eq!(query::get(db, &h).unwrap().unwrap().ver, 2);
}

#[test]
fn an_empty_text_and_an_entitys_value_are_refused() {
    let (_d, case) = case();
    let db = case.db();
    let h = hypothesis(&case);
    assert!(matches!(
        edit::set_text(db, &at(), &h, "   "),
        Err(Error::Invalid(m)) if m.contains("soft-delete")
    ));
    let ip = edit::add_entity(db, &at(), "net.ipv4", "198.51.100.7").unwrap();
    assert!(matches!(
        edit::set_text(db, &at(), &ip, "198.51.100.8"),
        Err(Error::Invalid(m)) if m.contains("identifies")
    ));
    assert!(edit::set_text(db, &at(), "nope", "x").is_err());
}

#[test]
fn a_node_of_any_kind_can_be_added_by_hand() {
    let (_d, case) = case();
    let db = case.db();

    let agent =
        edit::add_element(db, &at(), Kind::Entity, "actor.agent", " Triage agent ").unwrap();
    let e = query::get(db, &agent).unwrap().unwrap();
    assert_eq!(
        (e.kind, e.type_.as_str(), e.content.value.as_deref()),
        (Kind::Entity, "actor.agent", Some("Triage agent"))
    );

    // An event and an observation are statements, so they are sourced to this entry.
    for kind in [Kind::Event, Kind::Observation] {
        let id = edit::add_element(db, &at(), kind, "misc.event", "Disk was imaged").unwrap();
        let got = query::get(db, &id).unwrap().unwrap();
        assert_eq!(got.kind, kind);
        assert!(
            query::edges_from(db, &id)
                .unwrap()
                .iter()
                .any(|e| e.role == "source")
        );
    }

    // An annotation of any type; a hypothesis starts open.
    let hyp = edit::add_element(
        db,
        &at(),
        Kind::Annotation,
        "analysis.hypothesis",
        "It was the cron job",
    )
    .unwrap();
    let h = query::get(db, &hyp).unwrap().unwrap();
    assert_eq!(
        h.content
            .props
            .get("hypothesis_status")
            .and_then(|v| v.as_str()),
        Some("open")
    );
    let custom = edit::add_element(
        db,
        &at(),
        Kind::Annotation,
        "analysis.question",
        "Who owns it?",
    )
    .unwrap();
    assert_eq!(
        query::get(db, &custom).unwrap().unwrap().type_,
        "analysis.question"
    );

    // An external is its URI.
    let ext = edit::add_element(
        db,
        &at(),
        Kind::External,
        "",
        "https://example.org/report.pdf",
    )
    .unwrap();
    let x = query::get(db, &ext).unwrap().unwrap();
    assert_eq!(
        (x.kind, x.type_.as_str()),
        (Kind::External, "external.https")
    );

    // What is missing is said, not guessed.
    assert!(edit::add_element(db, &at(), Kind::Entity, "actor.agent", " ").is_err());
    assert!(edit::add_element(db, &at(), Kind::Entity, "", "x").is_err());
    assert!(edit::add_element(db, &at(), Kind::Event, "misc.event", "").is_err());
    assert!(edit::add_element(db, &at(), Kind::Annotation, "", "x").is_err());
}
