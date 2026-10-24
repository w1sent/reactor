//! Revert and staleness (ADR-0047).

use reactor_db::graph::revert::{self, What};
use reactor_db::graph::{self, Batch, Change, Content, Kind, Link, Put, Target, query};
use reactor_db::manifest::Author;
use reactor_db::{Case, Db};

fn case() -> (tempfile::TempDir, Case) {
    let dir = tempfile::tempdir().unwrap();
    let case = Case::create(dir.path(), None).unwrap();
    (dir, case)
}

fn me() -> Batch {
    Batch::new(Author::analyst())
}

fn count(db: &Db, table: &'static str) -> i64 {
    db.read(|c| Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?))
        .unwrap()
}

fn resolution(db: &Db, domain: &str, n: u32) -> graph::Applied {
    let put = Put {
        id: Some(format!("observation--{n:032}")),
        links: vec![
            Link::entity("query", "net.domain", domain),
            Link::entity("answer", "net.ipv4", "198.51.100.7"),
            Link::source("artifact:data/pdns.csv").at(&format!("row={n}"), None),
        ],
        ..Put::new(Kind::Observation, "net.resolution")
    };
    graph::apply(db, me().put(put)).unwrap()
}

fn judge(db: &Db, about: &str, author: Author) -> String {
    let put = Put {
        content: Content {
            value: Some("looks like the beacon".into()),
            confidence: Some("likely".into()),
            ..Content::default()
        },
        links: vec![Link::new("about", Target::Id(about.to_string())).pinned()],
        ..Put::new(Kind::Annotation, "analysis.judgement")
    };
    graph::apply(db, Batch::new(author).put(put)).unwrap().ids[0].clone()
}

#[test]
fn a_batch_nothing_builds_on_is_taken_back_whole() {
    let (_d, case) = case();
    let db = case.db();
    let keep = resolution(db, "keep.example", 1);
    let wrong = resolution(db, "wrong.example", 2);
    let seq = wrong.event_seq.unwrap();

    let plan = revert::plan(db, &What::Event(seq), false).unwrap();
    assert!(plan.blocked_by.is_empty());
    // The claim and the domain only it named go; the address and the external were
    // there before and stay.
    let going: Vec<(&str, u32)> = plan
        .elements
        .iter()
        .map(|e| (e.type_.as_str(), e.back_to))
        .collect();
    assert_eq!(going, [("net.domain", 0), ("net.resolution", 0)]);
    assert_eq!(plan.elements[0].authors, ["analyst:user"]);
    // A plan changes nothing.
    assert_eq!(count(db, "element"), 6);

    revert::apply(db, What::Event(seq), false, &Author::analyst(), Some("s1")).unwrap();
    assert_eq!(count(db, "element"), 4);
    assert!(query::get(db, &wrong.ids[0]).unwrap().is_none());
    assert!(query::get(db, &keep.ids[0]).unwrap().is_some());
    assert_eq!(query::relations(db, None).unwrap().len(), 1);
    let ip = query::entity_id(db, "net.ipv4", "198.51.100.7", None).unwrap();
    assert_eq!(query::edges_to(db, &ip).unwrap().len(), 1);
    // The change log says a revert happened; taking it back twice is refused.
    let reverts: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM event WHERE type = 'revert'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(reverts, 1);
    assert!(revert::apply(db, What::Event(seq), false, &Author::analyst(), None).is_err());
}

#[test]
fn what_builds_on_a_version_blocks_its_revert_until_forced() {
    let (_d, case) = case();
    let db = case.db();
    let obs = resolution(db, "cdn-sync.example", 1).ids[0].clone();
    // A correction, then an agent judges the corrected version.
    let fix = graph::apply(
        db,
        me().change(Change {
            id: obs.clone(),
            confidence: Some(Some("very likely".into())),
            ..Change::default()
        }),
    )
    .unwrap();
    let judgement = judge(db, &obs, Author::agent("model-x"));
    assert_eq!(
        query::edges_from(db, &judgement).unwrap()[0].pinned_ver,
        Some(2)
    );

    let back = What::Element {
        id: obs.clone(),
        to: 1,
    };
    let plan = revert::plan(db, &back, false).unwrap();
    assert_eq!(plan.blocked_by.len(), 1);
    assert!(
        plan.blocked_by[0].contains("analysis.judgement"),
        "{:?}",
        plan.blocked_by
    );
    let e = revert::apply(db, back.clone(), false, &Author::analyst(), None).unwrap_err();
    assert!(e.to_string().contains("builds on"), "{e}");
    assert_eq!(
        query::get(db, &obs).unwrap().unwrap().ver,
        2,
        "nothing changed"
    );

    // Forced: the preview names what goes along, and whose it is.
    let forced = revert::plan(db, &back, true).unwrap();
    assert!(forced.blocked_by.is_empty());
    let along = forced.elements.iter().find(|e| e.id == judgement).unwrap();
    assert_eq!(along.back_to, 0);
    assert_eq!(along.authors, ["agent:reactor@model-x"]);
    assert!(along.why.contains("built on"), "{}", along.why);

    revert::apply(db, back, true, &Author::analyst(), None).unwrap();
    assert_eq!(query::get(db, &obs).unwrap().unwrap().ver, 1);
    assert!(query::get(db, &judgement).unwrap().is_none());
    // Reverting by the change's own number does the same thing as naming the element.
    assert!(
        revert::apply(
            db,
            What::Event(fix.event_seq.unwrap()),
            false,
            &Author::analyst(),
            None
        )
        .is_err()
    );
}

#[test]
fn an_unpinned_note_follows_the_element_and_is_not_in_the_way() {
    let (_d, case) = case();
    let db = case.db();
    let obs = resolution(db, "cdn-sync.example", 1).ids[0].clone();
    let note = graph::apply(
        db,
        me().put(Put {
            content: Content::value("ask the SOC about this"),
            links: vec![Link::new("about", Target::Id(obs.clone()))],
            ..Put::new(Kind::Annotation, "analysis.note")
        }),
    )
    .unwrap()
    .ids[0]
        .clone();
    graph::apply(
        db,
        me().change(Change {
            id: obs.clone(),
            status: Some("conflicting".into()),
            ..Change::default()
        }),
    )
    .unwrap();
    let plan = revert::apply(
        db,
        What::Element {
            id: obs.clone(),
            to: 1,
        },
        false,
        &Author::analyst(),
        None,
    )
    .unwrap();
    assert_eq!(plan.elements.len(), 1);
    assert!(query::get(db, &note).unwrap().is_some());

    // But taking the observation out of the graph takes what was made about it along.
    let plan = revert::plan(db, &What::Element { id: obs, to: 0 }, false).unwrap();
    assert!(plan.elements.iter().any(|e| e.id == note && e.back_to == 0));
}

#[test]
fn what_may_no_longer_hold_is_flagged_not_resolved() {
    let (_d, case) = case();
    let db = case.db();
    let obs = resolution(db, "cdn-sync.example", 1).ids[0].clone();
    let judgement = judge(db, &obs, Author::analyst());
    assert!(query::stale(db).unwrap().is_empty());

    // The judged claim moves on, then is disputed.
    graph::apply(
        db,
        me().change(Change {
            id: obs.clone(),
            status: Some("conflicting".into()),
            ..Change::default()
        }),
    )
    .unwrap();
    let stale = query::stale(db).unwrap();
    assert_eq!(stale.len(), 2);
    assert!(stale.iter().all(|s| s.id == judgement));
    assert!(
        stale[0].reason.contains("judged version 1")
            && stale[0].reason.contains("now at version 2")
    );
    assert!(stale[1].reason.contains("now conflicting"));
    // Nothing was done about it: the judgement is as it was.
    assert_eq!(query::get(db, &judgement).unwrap().unwrap().ver, 1);

    // An age limit on a type is a row; a claim older than it is flagged.
    db.write(|tx| {
        tx.execute(
            "INSERT INTO known_type(name, kind, stale_after_days) VALUES ('net.resolution', 'observation', -1) ON CONFLICT(name) DO UPDATE SET stale_after_days = -1",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        query::stale(db)
            .unwrap()
            .iter()
            .any(|s| s.id == obs && s.reason.contains("checked again"))
    );
}
