//! The session store, the history readers, and the projection over reductions.

mod common;

use std::io::Write;

use common::*;
use reactor_agent::context::{self, Msg, Source, active_reductions, hidden_by, project};
use reactor_agent::entry::{Kind, Mode, Reduction, Stub, Trigger};
use reactor_agent::history;
use reactor_agent::store::Store;
use serde_json::json;

// -- the log -----------------------------------------------------------------------

#[test]
fn a_session_is_an_append_only_log_that_reopens_identically() {
    let (dir, mut s) = store();
    user(&mut s, "look at a.out");
    assistant(&mut s, "on it");
    bash(&mut s, "c1", "file a.out", "ELF 64-bit");
    let before: Vec<_> = s.all().to_vec();
    let head = s.head();
    let path = s.dir().to_path_buf();
    drop(s);

    let s = Store::open(&path).unwrap();
    assert_eq!(s.all(), before.as_slice());
    assert_eq!(s.head(), head);
    // One entry per line, nothing rewritten.
    let text = std::fs::read_to_string(path.join("session.jsonl")).unwrap();
    assert_eq!(text.lines().count(), before.len());
    drop(dir);
}

#[test]
fn a_second_session_cannot_be_created_over_an_existing_one() {
    let (dir, s) = store();
    assert!(Store::create(s.dir(), "again", dir.path()).is_err());
}

#[test]
fn a_torn_last_line_is_dropped_and_the_next_append_starts_clean() {
    // The crash story: a killed process loses at most the entry being written.
    let (_dir, mut s) = store();
    user(&mut s, "one");
    user(&mut s, "two");
    let path = s.dir().to_path_buf();
    drop(s);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path.join("session.jsonl"))
        .unwrap();
    f.write_all(b"{\"id\":3,\"parent\":2,\"ts\":1,\"type\":\"user\",\"te")
        .unwrap();
    drop(f);

    let mut s = Store::open(&path).unwrap();
    assert_eq!(
        s.len(),
        3,
        "session header + two entries; the torn one is gone"
    );
    user(&mut s, "three");
    let text = std::fs::read_to_string(path.join("session.jsonl")).unwrap();
    assert!(
        text.lines()
            .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()),
        "every line parses"
    );
    assert_eq!(Store::open(&path).unwrap().len(), 4);
}

#[test]
fn corruption_in_the_middle_is_an_error_not_a_silent_gap() {
    let (_dir, mut s) = store();
    user(&mut s, "one");
    user(&mut s, "two");
    let path = s.dir().join("session.jsonl");
    drop(s);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines[1] = "{ not json";
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    assert!(Store::open(path.parent().unwrap()).is_err());
}

// -- branches -------------------------------------------------------------------------

#[test]
fn a_branch_is_a_parent_pointer_and_forking_rewrites_nothing() {
    let (_dir, mut s) = store();
    let u = user(&mut s, "question");
    let a1 = assistant(&mut s, "first answer");
    let bytes_before = std::fs::metadata(s.dir().join("session.jsonl"))
        .unwrap()
        .len();

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
    // The original branch's bytes are untouched: the log only grew.
    let after = std::fs::read(s.dir().join("session.jsonl")).unwrap();
    assert!(after.len() as u64 > bytes_before);
    assert!(s.set_head(999).is_err());
}

// -- blobs and the index ---------------------------------------------------------------

#[test]
fn a_blob_keeps_every_byte_and_names_itself_after_its_label() {
    let (_dir, mut s) = store();
    let a = s.write_blob("bash 12", b"whole output").unwrap();
    let b = s.write_blob("bash 12", b"another").unwrap();
    assert_ne!(a.file, b.file, "no overwriting");
    assert_eq!(s.read_blob(&a).unwrap(), b"whole output");
    assert_eq!(a.bytes, 12);
    assert!(a.file.starts_with("bash_12-"), "{}", a.file);
}

#[test]
fn the_index_is_derived_and_rebuilt_when_stale() {
    let (_dir, mut s) = store();
    user(&mut s, "hello there");
    let idx = s.index().unwrap();
    assert_eq!(idx.entries.len(), 2);
    assert_eq!(idx.entries[1].kind, "user");
    assert_eq!(idx.entries[1].preview, "hello there");

    // Delete it: it comes back.
    std::fs::remove_file(s.dir().join("index.json")).unwrap();
    assert_eq!(s.index().unwrap(), idx);

    // Corrupt it: it comes back.
    std::fs::write(s.dir().join("index.json"), "junk").unwrap();
    assert_eq!(s.index().unwrap(), idx);

    // Grow the log: the stale index is not believed.
    user(&mut s, "and another");
    assert_eq!(s.index().unwrap().entries.len(), 3);
}

// -- history -----------------------------------------------------------------------------

#[test]
fn history_reads_a_truncated_result_back_from_its_blob() {
    let (_dir, mut s) = store();
    let full = (1..=100)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let blob = s.write_blob("strings", full.as_bytes()).unwrap();
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
    let blob = s
        .write_blob("dump", b"aaa\nthe key is 0xDEADBEEF\nzzz")
        .unwrap();
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
    assert_eq!(st.manifest.goal.as_deref(), Some("two"));
    assert_eq!(st.identity.active.as_deref(), Some("publisher"));
    assert_eq!(st.scenario.as_ref().unwrap().step_index, 1);
    assert!(st.reporting.is_enabled());

    // A cleared scenario is stored as null and reads back as none.
    save(&mut s, "scenario", json!(null));
    assert!(context::SessionState::load(&s).scenario.is_none());
}
