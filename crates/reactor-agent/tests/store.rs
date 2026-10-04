//! The session store, the history readers, and the projection over reductions.

mod common;

use common::*;
use reactor_agent::context::{self, Msg, Source, active_reductions, hidden_by, project};
use reactor_agent::entry::{Kind, Mode, Reduction, Stub, Trigger};
use reactor_agent::history;
use reactor_agent::store::Store;
use serde_json::json;

// -- the log -----------------------------------------------------------------------

#[test]
fn a_session_is_an_append_only_log_that_reopens_identically() {
    let (_dir, mut s) = store();
    user(&mut s, "look at a.out");
    assistant(&mut s, "on it");
    bash(&mut s, "c1", "file a.out", "ELF 64-bit");
    let before: Vec<_> = s.all().to_vec();
    let head = s.head();
    let case = s.case().clone();
    drop(s);

    let s = Store::open(&case, "s1").unwrap();
    assert_eq!(s.all(), before.as_slice());
    assert_eq!(s.head(), head);
    // One entry per row, and the export is what the log file used to be.
    let mut out = Vec::new();
    let n = reactor_db::sessions::export_jsonl(case.db(), "s1", &mut out).unwrap();
    assert_eq!(n, before.len());
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.lines()
            .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()),
        "every line parses"
    );
}

#[test]
fn a_second_session_cannot_be_created_over_an_existing_one() {
    let (_dir, s) = store();
    let case = s.case().clone();
    assert!(Store::create(&case, "s1").is_err(), "while it is open");
    drop(s);
    assert!(Store::create(&case, "s1").is_err(), "nor afterwards");
    assert!(Store::open(&case, "nope").is_err());
}

#[test]
fn a_session_is_open_in_one_place_at_a_time() {
    // The contract SPEC §3 stated without a lock is now held by one.
    let (_dir, s) = store();
    let case = s.case().clone();
    let err = Store::open(&case, "s1").err().expect("already open");
    assert!(err.to_string().contains("another window"), "{err}");
    drop(s);
    Store::open(&case, "s1").unwrap();
}

#[test]
fn a_written_entry_is_never_changed_or_removed() {
    // The crash story: an append is one transaction, and the database refuses to
    // rewrite history -- for this code and for anything else that opens the file.
    let (_dir, mut s) = store();
    user(&mut s, "one");
    for sql in ["UPDATE entry SET body = '{}'", "DELETE FROM entry"] {
        let err = s
            .case()
            .db()
            .write(move |tx| Ok(tx.execute(sql, [])?))
            .unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
    }
    let case = s.case().clone();
    drop(s);
    assert_eq!(Store::open(&case, "s1").unwrap().len(), 2);
}

#[test]
fn the_first_entry_names_the_session_and_no_directory() {
    // ADR-0045: a case is moved as a unit, so nothing in it says where it is.
    let (dir, s) = store();
    let body = serde_json::to_string(s.get(0).unwrap()).unwrap();
    assert!(body.contains("\"session\":\"s1\""), "{body}");
    assert!(!body.contains(&*dir.path().to_string_lossy()), "{body}");
}

#[test]
fn two_sessions_of_one_case_keep_their_own_dense_ids() {
    let (_dir, mut a) = store();
    let mut b = Store::create(&a.case().clone(), "s2").unwrap();
    assert_eq!(user(&mut a, "in a"), 1);
    assert_eq!(user(&mut b, "in b"), 1);
    assert_eq!(user(&mut a, "more a"), 2);
    assert_eq!((a.len(), b.len()), (3, 2));
    assert_ne!(a.dir(), b.dir());
}

// -- branches -------------------------------------------------------------------------

#[test]
fn a_branch_is_a_parent_pointer_and_forking_rewrites_nothing() {
    let (_dir, mut s) = store();
    let u = user(&mut s, "question");
    let a1 = assistant(&mut s, "first answer");
    let before = s.get(a1).unwrap().clone();

    s.set_head(u).unwrap();
    let a2 = assistant(&mut s, "second answer");

    assert_eq!(s.get(a2).unwrap().parent, Some(u));
    assert_eq!(
        s.branch().iter().map(|e| e.id).collect::<Vec<_>>(),
        [0, u, a2]
    );
    assert_eq!(
        s.path_to(a1).iter().map(|e| e.id).collect::<Vec<_>>(),
        [0, u, a1]
    );
    assert_eq!(s.children(u), [a1, a2]);
    assert_eq!(s.leaves(), [a1, a2]);
    // The original branch is untouched: the log only grew.
    assert_eq!(s.get(a1).unwrap(), &before);
    assert_eq!(s.len(), 4);
    assert!(s.set_head(999).is_err());
}

// -- blobs and the index ---------------------------------------------------------------

#[test]
fn a_blob_keeps_every_byte_and_is_named_by_its_content() {
    let (_dir, mut s) = store();
    let a = s.write_blob(b"whole output").unwrap();
    let b = s.write_blob(b"another").unwrap();
    assert_ne!(a.sha256, b.sha256);
    assert_eq!(s.read_blob(&a).unwrap(), b"whole output");
    assert_eq!(a.bytes, 12);
    assert_eq!(
        s.write_blob(b"whole output").unwrap(),
        a,
        "the same bytes are one blob"
    );
    assert!(s.blob_path(&a).is_file());
    assert!(s.blob_path(&a).starts_with(s.case().dir().join("blobs")));
}

// -- history -----------------------------------------------------------------------------

#[test]
fn history_reads_a_truncated_result_back_from_its_blob() {
    let (_dir, mut s) = store();
    let full = (1..=100)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let blob = s.write_blob(full.as_bytes()).unwrap();
    let id = s
        .append(Kind::ToolResult {
            call_id: "c".into(),
            name: "bash".into(),
            content: "line 1\n… cut …\nline 100".into(),
            is_error: false,
            blob: Some(blob),
        })
        .unwrap();

    let all = history::read(&s, id, None, None).unwrap();
    assert!(all.starts_with("line 1\nline 2"));
    assert!(all.ends_with("line 100"));
    assert_eq!(
        history::read(&s, id, Some(50), Some(2)).unwrap(),
        "line 50\nline 51\n… [49 more line(s); continue with offset 52]"
    );
    assert!(history::read(&s, id, Some(500), None).is_err());
    assert!(history::read(&s, 12345, None, None).is_err());
}

#[test]
fn history_search_reaches_blobs_and_every_kind_of_entry() {
    let (_dir, mut s) = store();
    user(&mut s, "find the license check");
    let blob = s.write_blob(b"aaa\nthe key is 0xDEADBEEF\nzzz").unwrap();
    s.append(Kind::ToolResult {
        call_id: "c".into(),
        name: "bash".into(),
        content: "aaa".into(),
        is_error: false,
        blob: Some(blob),
    })
    .unwrap();

    let hits = history::search(&s, "DEADBEEF", &[], 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].kind, hits[0].line), ("tool_result", 2));
    assert_eq!(
        history::search(&s, "license", &["user".into()], 10)
            .unwrap()
            .len(),
        1
    );
    assert!(
        history::search(&s, "license", &["tool_result".into()], 10)
            .unwrap()
            .is_empty()
    );
    assert!(history::search(&s, "(unclosed", &[], 10).is_err());
    assert_eq!(
        history::search(&s, ".", &[], 2).unwrap().len(),
        2,
        "the limit holds"
    );
}

// -- projection and reductions -------------------------------------------------------------

fn reduction(covers: Vec<u64>, summary: Option<&str>, stubs: Vec<Stub>) -> Kind {
    Kind::Reduction(Reduction {
        mode: Mode::Auto,
        trigger: Trigger::Manual,
        covers,
        summary: summary.map(str::to_string),
        stubs,
        before_tokens: 100,
        after_tokens: 10,
    })
}

#[test]
fn an_unreduced_branch_projects_to_its_messages_and_skips_state_entries() {
    let (_dir, mut s) = store();
    user(&mut s, "q");
    s.append(Kind::Custom {
        key: "manifest".into(),
        data: json!({"steps": []}),
    })
    .unwrap();
    s.append(Kind::Label {
        text: "note".into(),
    })
    .unwrap();
    bash(&mut s, "c1", "ls", "a b");
    let items = project(&s);
    assert_eq!(items.len(), 3, "user, assistant call, tool result");
    assert!(matches!(items[0].msg, Msg::User { .. }));
    assert!(matches!(items[2].msg, Msg::ToolResult { .. }));
}

#[test]
fn a_reduction_replaces_its_range_with_one_message_at_the_first_hidden_position() {
    let (_dir, mut s) = store();
    let u1 = user(&mut s, "old question");
    let (a, r) = bash(&mut s, "c1", "strings x", "lots of output");
    let u2 = user(&mut s, "current question");
    let red = s
        .append(reduction(
            vec![u1, a, r],
            Some("looked at strings; nothing"),
            vec![Stub {
                entry: r,
                what: "tool result".into(),
                detail: "bash strings x".into(),
                bytes: 14,
            }],
        ))
        .unwrap();

    let items = project(&s);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].source, Source::Reduction(red));
    let Msg::User { text } = &items[0].msg else {
        panic!()
    };
    assert!(text.contains("looked at strings; nothing"));
    assert!(text.contains(&format!("#{r} tool result: bash strings x (14 bytes)")));
    assert_eq!(items[1].source, Source::Entry(u2));
    assert_eq!(hidden_by(&s).get(&r), Some(&red));
}

#[test]
fn the_rendering_of_a_reduction_is_deterministic() {
    let r = Reduction {
        mode: Mode::Auto,
        trigger: Trigger::Budget,
        covers: vec![3, 4, 5],
        summary: Some("  notes  ".into()),
        stubs: vec![Stub {
            entry: 5,
            what: "tool result".into(),
            detail: "bash ls".into(),
            bytes: 9,
        }],
        before_tokens: 0,
        after_tokens: 0,
    };
    assert_eq!(
        context::render_reduction(&r),
        "## Earlier in this session (context reduced)\n\nnotes\n\nDropped from view (the originals are kept; read one back with `history_read` and its #address):\n- #5 tool result: bash ls (9 bytes)"
    );
}

#[test]
fn restoring_a_reduction_brings_the_originals_back() {
    // ADR-0036: originals survive, so undo is an append, not a rewrite.
    let (_dir, mut s) = store();
    let u1 = user(&mut s, "old");
    let a1 = assistant(&mut s, "older answer");
    user(&mut s, "new");
    let red = s
        .append(reduction(vec![u1, a1], Some("summary"), vec![]))
        .unwrap();
    assert_eq!(project(&s).len(), 2);

    s.append(Kind::Restore { reduction: red }).unwrap();
    let items = project(&s);
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| matches!(i.source, Source::Entry(_))));
    assert!(active_reductions(&s).is_empty());
}

#[test]
fn a_later_reduction_supersedes_an_earlier_one_and_restoring_it_revives_the_earlier() {
    let (_dir, mut s) = store();
    let u1 = user(&mut s, "one");
    let a1 = assistant(&mut s, "two");
    let r1 = s
        .append(reduction(vec![u1, a1], Some("first"), vec![]))
        .unwrap();
    let u2 = user(&mut s, "three");
    let a2 = assistant(&mut s, "four");
    user(&mut s, "five");
    let r2 = s
        .append(reduction(
            vec![r1, u2, a2],
            Some("second, containing the first"),
            vec![],
        ))
        .unwrap();

    assert_eq!(active_reductions(&s), [r2]);
    let items = project(&s);
    assert_eq!(items.len(), 2, "one stand-in, then the latest message");
    assert_eq!(items[0].source, Source::Reduction(r2));

    s.append(Kind::Restore { reduction: r2 }).unwrap();
    assert_eq!(active_reductions(&s), [r1]);
    let items = project(&s);
    assert_eq!(items[0].source, Source::Reduction(r1));
    assert_eq!(items.len(), 4, "r1's stand-in, then three, four, five");
}

#[test]
fn a_reduction_only_applies_on_the_branch_that_made_it() {
    let (_dir, mut s) = store();
    let u = user(&mut s, "shared start");
    let a = assistant(&mut s, "answer on branch one");
    user(&mut s, "follow-up");
    s.append(reduction(vec![u, a], Some("branch one only"), vec![]))
        .unwrap();

    // Fork from the first entry: the other branch never saw that reduction.
    s.set_head(u).unwrap();
    assistant(&mut s, "answer on branch two");
    let items = project(&s);
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| matches!(i.source, Source::Entry(_))));
}

#[test]
fn session_state_is_the_latest_custom_entry_per_key_on_the_branch() {
    let (_dir, mut s) = store();
    let save = |s: &mut Store, key: &str, data| context::save_state(s, key, data).unwrap();
    save(&mut s, "manifest", json!({"goal": "one", "steps": []}));
    save(&mut s, "manifest", json!({"goal": "two", "steps": []}));
    save(&mut s, "identity", json!({"active": "publisher"}));
    save(
        &mut s,
        "scenario",
        json!({"scenarioId": "investigation", "stepIndex": 1, "summaries": ["x"]}),
    );
    save(&mut s, "reporting", json!({"enabled": true, "level": 2}));

    let st = context::SessionState::load(&s);
    // The goal is the case's (ADR-0045): one stored in a session entry is not read.
    assert_eq!(st.manifest.goal, None);
    assert_eq!(st.identity.active.as_deref(), Some("publisher"));
    assert_eq!(st.scenario.as_ref().unwrap().step_index, 1);
    assert!(st.reporting.is_enabled());

    // A cleared scenario is stored as null and reads back as none.
    save(&mut s, "scenario", json!(null));
    assert!(context::SessionState::load(&s).scenario.is_none());
}

// -- the case: strategy and the other sessions ---------------------------------------------

#[test]
fn the_goal_and_guidelines_are_the_cases_and_the_steps_are_the_sessions() {
    use reactor_context::manifest::{State, Step};
    use reactor_db::manifest::Author;
    let (_dir, mut a) = store();
    let mut b = Store::create(&a.case().clone(), "s2").unwrap();

    let step = |t: &str| Step {
        summary: t.into(),
        status: "in progress".into(),
    };
    let in_a = State {
        goal: Some("find the C2".into()),
        guidelines: Some("static first".into()),
        steps: vec![step("triage")],
        ..State::default()
    };
    context::save_manifest(&mut a, &in_a, &Author::analyst()).unwrap();

    // The other session sees the strategy at once, and none of the steps.
    let seen = context::SessionState::load(&b).manifest;
    assert_eq!(seen.goal.as_deref(), Some("find the C2"));
    assert_eq!(seen.guidelines.as_deref(), Some("static first"));
    assert!(seen.steps.is_empty());

    // It changes the goal; the first session's next load has it, and keeps its steps.
    let in_b = State {
        goal: Some("find the C2 and the dropper".into()),
        steps: vec![step("unpack")],
        ..seen
    };
    context::save_manifest(&mut b, &in_b, &Author::analyst()).unwrap();
    let back = context::SessionState::load(&a).manifest;
    assert_eq!(back.goal.as_deref(), Some("find the C2 and the dropper"));
    assert_eq!(back.steps, vec![step("triage")]);

    // Branching a session does not branch the goal.
    a.set_head(0).unwrap();
    user(&mut a, "another way");
    let forked = context::SessionState::load(&a).manifest;
    assert_eq!(forked.goal.as_deref(), Some("find the C2 and the dropper"));
    assert!(
        forked.steps.is_empty(),
        "the steps were on the other branch"
    );

    // Saving what is already stored writes nothing.
    let n = a.len();
    context::save_manifest(&mut a, &forked, &Author::analyst()).unwrap();
    assert_eq!(a.len(), n);
}

#[test]
fn history_reaches_the_other_sessions_of_the_case() {
    let (_dir, mut a) = store();
    let mut b = Store::create(&a.case().clone(), "s2").unwrap();
    user(&mut a, "the mutex is Global/Zx9");
    let blob = b
        .write_blob(b"aaa\nmutex Global/Zx9 seen in sample\nzzz")
        .unwrap();
    b.append(Kind::ToolResult {
        call_id: "c".into(),
        name: "bash".into(),
        content: "aaa".into(),
        is_error: false,
        blob: Some(blob),
    })
    .unwrap();

    // On its own a session searches itself.
    assert_eq!(history::search(&a, "Zx9", &[], 10).unwrap().len(), 1);
    // Across the case: its own hits first, then the others', each saying where it is.
    let hits = history::search_case(&a, "Zx9", &[], 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].session.as_deref(), hits[0].id), (None, 1));
    assert_eq!(
        (hits[1].session.as_deref(), hits[1].id, hits[1].line),
        (Some("s2"), 1, 2)
    );
    assert!(history::render_hits(&hits, 10).contains("session s2 #1:2 (tool_result)"));
    assert_eq!(
        history::search_case(&a, "Zx9", &[], 1).unwrap().len(),
        1,
        "the limit holds"
    );

    // And an entry of the other session reads back whole, blob included.
    let other = history::Log::other(&a, "s2").unwrap();
    assert!(
        history::read_from(&other, 1, None, None)
            .unwrap()
            .contains("seen in sample")
    );
    assert!(history::index_of(&other, None, None).starts_with("#0 session"));
    assert!(history::Log::other(&a, "nope").is_err());
    let listing = history::sessions_listing(&a).unwrap();
    assert!(
        listing.contains("s1 (this session)") && listing.contains("s2 "),
        "{listing}"
    );
}

#[test]
fn sessions_are_numbered_one_two_three_in_a_case_and_older_ids_are_not_counted() {
    let dir = tempfile::tempdir().unwrap();
    let case = reactor_db::Case::create(dir.path(), None).unwrap();
    // A session made before numbering, with the old kind of id: not a number, not counted.
    let old = Store::create(&case, "1759581234-4242").unwrap();
    drop(old);
    let a = Store::create_next(&case).unwrap();
    let b = Store::create_next(&case).unwrap();
    let c = Store::create_next(&case).unwrap();
    assert_eq!((a.session(), b.session(), c.session()), ("1", "2", "3"));
    // A gap is not filled: the number after the highest.
    drop(b);
    let d = Store::create_next(&case).unwrap();
    assert_eq!(d.session(), "4");
    // The number is the session's, and is in its first entry.
    assert!(matches!(
        &d.branch()[0].kind,
        reactor_agent::entry::Kind::Session { session, .. } if session == "4"
    ));
}
