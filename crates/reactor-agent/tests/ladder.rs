//! The ladder's parts (ADR-0058): masking tool results in place, the cut that may fall inside
//! the current turn, a fade that keeps what the person said, and the summary's shape.

mod common;

use common::*;
use reactor_agent::budget::{
    self, BudgetConfig, MASK_MARK, MaskOptions, follows_template, plan, plan_mask, summary_size,
};
use reactor_agent::context::{
    self, Msg, Source, active_reductions, hidden_by, msg_tokens, project,
};
use reactor_agent::entry::{Kind, Mask, Mode, Reduction, Trigger};
use reactor_agent::store::Store;

fn dump(tag: &str) -> String {
    format!(
        "{tag} start\n{}\n{tag} end",
        "0123456789abcdef ".repeat(300)
    )
}

fn cfg(window: u64) -> BudgetConfig {
    BudgetConfig {
        reserve: 100,
        ..BudgetConfig::new(window)
    }
}

/// A session of `n` rounds of (user question, bash, big result, assistant), then a final
/// question and an unanswered tool call whose result has just arrived.
fn worked(n: usize) -> (tempfile::TempDir, Store, Vec<u64>) {
    let (dir, mut s) = store();
    let mut results = Vec::new();
    for i in 0..n {
        user(&mut s, &format!("question {i}"));
        let (_, r) = bash(
            &mut s,
            &format!("c{i}"),
            &format!("strings f{i}"),
            &dump(&format!("dump{i}")),
        );
        results.push(r);
        assistant(&mut s, &format!("f{i} is a table lookup"));
    }
    (dir, s, results)
}

/// A pass that must reach `target`, sparing the newest `protect` tokens and what is unread.
fn opts(target: u64, protect: u64) -> MaskOptions {
    MaskOptions {
        target_tokens: target,
        protect_tokens: protect,
        preview_chars: 150,
        require_target: true,
        spare_unread: true,
    }
}

/// A pass that masks all it may and need not reach any target, sparing the unread.
fn all(protect: u64) -> MaskOptions {
    MaskOptions {
        target_tokens: 1,
        require_target: false,
        ..opts(1, protect)
    }
}

// -- masking: which results, and what they leave -------------------------------------------

#[test]
fn masking_takes_the_oldest_results_first_and_stops_at_the_target() {
    let (_d, s, results) = worked(6);
    let items = project(&s);
    let total = budget::context_tokens(0, &items);
    // Ask for about a third of the context to go.
    let p = plan_mask(&s, 0, &items, &opts(total * 2 / 3, 0)).unwrap();
    let masked: Vec<u64> = p.masks.iter().map(|m| m.entry).collect();
    assert!(
        !masked.is_empty() && masked.len() < results.len(),
        "some, not all: {masked:?}"
    );
    assert_eq!(masked, results[..masked.len()], "oldest first");
    assert!(
        p.after_tokens <= total * 2 / 3,
        "{} <= {}",
        p.after_tokens,
        total * 2 / 3
    );
}

#[test]
fn a_stub_says_what_it_was_where_to_read_it_and_keeps_the_start_and_the_end() {
    let (_d, s, results) = worked(3);
    let items = project(&s);
    let p = plan_mask(&s, 0, &items, &all(0)).unwrap();
    let m = p.masks.iter().find(|m| m.entry == results[0]).unwrap();
    assert!(m.stub.starts_with(MASK_MARK), "{}", m.stub);
    assert!(m.stub.contains(&format!("#{}", results[0])));
    assert!(m.stub.contains("strings f0"), "what was run: {}", m.stub);
    assert!(
        m.stub.contains("history_read"),
        "how to read it: {}",
        m.stub
    );
    assert!(m.stub.contains("dump0 start") && m.stub.contains("dump0 end"));
    assert_eq!(m.bytes as usize, dump("dump0").len());
    assert!(
        m.stub.len() < dump("dump0").len() / 4,
        "and it is much smaller: {} bytes",
        m.stub.len()
    );
}

#[test]
fn the_newest_work_is_never_masked_and_neither_is_a_result_the_model_has_not_read() {
    let (_d, mut s, results) = worked(4);
    // A result that has just arrived, with no assistant message after it yet.
    user(&mut s, "now check the last one");
    let (_, unread) = bash(&mut s, "c-new", "strings new", &dump("fresh"));
    let items = project(&s);
    let p = plan_mask(&s, 0, &items, &all(0));
    let masked: Vec<u64> = p
        .map(|p| p.masks.iter().map(|m| m.entry).collect())
        .unwrap_or_default();
    assert!(!masked.contains(&unread), "the model has not read it yet");
    for r in &results {
        assert!(masked.contains(r), "the older results are all fair game");
    }

    // And a protected slice spares the newest of what has been read.
    let newest_read = *results.last().unwrap();
    let at = items
        .iter()
        .position(|i| i.source == Source::Entry(newest_read))
        .unwrap();
    let room: u64 = items[at..].iter().map(|i| msg_tokens(&i.msg)).sum();
    let p = plan_mask(&s, 0, &items, &all(room)).unwrap();
    assert!(
        !p.masks.iter().any(|m| m.entry == newest_read),
        "the protected newest part is spared"
    );
}

#[test]
fn small_results_and_results_that_are_already_stubs_are_left_alone() {
    let (_d, mut s) = store();
    user(&mut s, "q");
    let (_, small) = bash(&mut s, "c1", "ls", "a.out\nb.out");
    let (_, big) = bash(&mut s, "c2", "strings x", &dump("big"));
    assistant(&mut s, "done reading");
    let items = project(&s);
    let p = plan_mask(&s, 0, &items, &all(0)).unwrap();
    let masked: Vec<u64> = p.masks.iter().map(|m| m.entry).collect();
    assert_eq!(masked, vec![big], "{small} is too small to be worth a stub");

    // Apply it, and ask again: what is a stub already is not masked twice.
    budget::commit_mask(&mut s, p, Trigger::Budget).unwrap();
    assert!(plan_mask(&s, 0, &project(&s), &all(0)).is_none());
}

#[test]
fn a_result_the_model_read_back_goes_last() {
    let (_d, mut s) = store();
    user(&mut s, "q");
    // Oldest: a read-back; then an ordinary dump, newer.
    let (_, readback) = call(
        &mut s,
        "h1",
        "history_read",
        serde_json::json!({"entry": 3}),
        &dump("readback"),
    );
    let (_, ordinary) = bash(&mut s, "c2", "strings x", &dump("ordinary"));
    assistant(&mut s, "ok");
    let items = project(&s);
    let one = plan_mask(
        &s,
        0,
        &items,
        &opts(budget::context_tokens(0, &items) - 700, 0),
    )
    .unwrap();
    let masked: Vec<u64> = one.masks.iter().map(|m| m.entry).collect();
    assert_eq!(
        masked,
        vec![ordinary],
        "the ordinary one first, though it is newer: {readback}"
    );
}

#[test]
fn nothing_is_masked_unless_that_alone_reaches_the_target() {
    let (_d, s, _) = worked(3);
    let items = project(&s);
    // The user messages and answers alone are over a target of 1 token: masking cannot get there.
    assert!(plan_mask(&s, 0, &items, &opts(1, 0)).is_none());
    // A target already met needs nothing.
    let total = budget::context_tokens(0, &items);
    assert!(plan_mask(&s, 0, &items, &opts(total + 10, 0)).is_none());
}

#[test]
fn the_last_resort_cuts_what_it_can_including_the_newest_and_keeps_what_the_room_allows() {
    let (_d, mut s) = store();
    user(&mut s, "THE TASK");
    let huge = format!("first line\n{}\nlast line", "9876543210 ".repeat(3_000));
    let (_, r) = bash(&mut s, "c1", "strings huge", &huge);
    let items = project(&s);
    let total = budget::context_tokens(0, &items);
    let target = total / 2;
    let lr = MaskOptions {
        target_tokens: target,
        protect_tokens: 0,
        preview_chars: 150,
        require_target: false,
        spare_unread: false,
    };
    let p =
        plan_mask(&s, 0, &items, &lr).expect("the newest, unread result is cut when it must be");
    assert_eq!(p.masks[0].entry, r);
    // It is cut to what the room allows, not to a token's worth: far more than the
    // fixed preview, so the model has something to work with.
    assert!(
        p.masks[0].stub.len() > 1_000,
        "{} bytes kept",
        p.masks[0].stub.len()
    );
    assert!(p.masks[0].stub.contains("first line") && p.masks[0].stub.contains("last line"));
    assert!(
        p.after_tokens <= target + 50,
        "{} <= {}",
        p.after_tokens,
        target
    );
}

// -- projection: a mask is in place, a restore undoes it ------------------------------------------

#[test]
fn a_masked_result_shows_its_stub_in_place_and_the_log_keeps_the_whole_of_it() {
    let (_d, mut s, results) = worked(3);
    let before = project(&s);
    let p = plan_mask(&s, 0, &before, &all(0)).unwrap();
    let red = budget::commit_mask(&mut s, p, Trigger::Budget).unwrap();
    let after = project(&s);

    assert_eq!(
        after.len(),
        before.len(),
        "nothing is hidden: the same messages"
    );
    let (Msg::ToolResult { content: was, .. }, Msg::ToolResult { content: now, .. }) = (
        &before
            .iter()
            .find(|i| i.source == Source::Entry(results[0]))
            .unwrap()
            .msg,
        &after
            .iter()
            .find(|i| i.source == Source::Entry(results[0]))
            .unwrap()
            .msg,
    ) else {
        panic!()
    };
    assert!(now.starts_with(MASK_MARK) && now.len() < was.len());
    // Calls and results still pair up, in the same order.
    let shape = |items: &[context::Item]| -> Vec<&'static str> {
        items
            .iter()
            .map(|i| match i.msg {
                Msg::User { .. } => "u",
                Msg::Assistant { .. } => "a",
                Msg::ToolResult { .. } => "r",
            })
            .collect()
    };
    assert_eq!(shape(&before), shape(&after));
    // The original is in the log, whole.
    let Kind::ToolResult { content, .. } = &s.get(results[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(content, was);
    assert!(active_reductions(&s).contains(&red));
    assert!(hidden_by(&s).is_empty(), "a mask hides nothing");
}

#[test]
fn restoring_a_mask_brings_the_results_back() {
    let (_d, mut s, _) = worked(3);
    let before = project(&s);
    let p = plan_mask(&s, 0, &before, &all(0)).unwrap();
    let red = budget::commit_mask(&mut s, p, Trigger::Budget).unwrap();
    s.append(Kind::Restore { reduction: red }).unwrap();
    assert_eq!(project(&s), before);
    assert!(active_reductions(&s).is_empty());
}

#[test]
fn a_mask_over_what_a_later_reduction_hides_is_harmless() {
    let (_d, mut s, results) = worked(3);
    let items = project(&s);
    let p = plan_mask(&s, 0, &items, &all(0)).unwrap();
    budget::commit_mask(&mut s, p, Trigger::Budget).unwrap();
    // A range reduction now covers the first round, masked result and all.
    let covered: Vec<u64> = (0..=results[0] + 1).collect();
    s.append(Kind::Reduction(Reduction {
        mode: Mode::Auto,
        trigger: Trigger::Budget,
        covers: covered,
        summary: Some("the first round".into()),
        stubs: vec![],
        before_tokens: 0,
        after_tokens: 0,
        masks: vec![],
        after: None,
    }))
    .unwrap();
    let items = project(&s);
    assert!(
        !items.iter().any(|i| i.source == Source::Entry(results[0])),
        "hidden, so not shown masked either"
    );
    assert!(
        items
            .iter()
            .any(|i| matches!(&i.msg, Msg::User { text } if text.contains("the first round")))
    );
}

#[test]
fn a_stand_in_with_an_anchor_goes_right_behind_the_message_it_is_anchored_to() {
    let (_d, mut s) = store();
    let old = user(&mut s, "an older question");
    let (a, r) = bash(&mut s, "c1", "strings x", &dump("old"));
    let task = user(&mut s, "THE CURRENT TASK");
    let (a2, r2) = bash(&mut s, "c2", "strings y", &dump("new"));
    // Everything but the task and the last group is covered; the stand-in is anchored.
    let red = s
        .append(Kind::Reduction(Reduction {
            mode: Mode::Compact,
            trigger: Trigger::Budget,
            covers: vec![old, a, r],
            summary: Some("what came before".into()),
            stubs: vec![],
            before_tokens: 0,
            after_tokens: 0,
            masks: vec![],
            after: Some(task),
        }))
        .unwrap();
    let items = project(&s);
    let order: Vec<Source> = items.iter().map(|i| i.source).collect();
    assert_eq!(
        order,
        vec![
            Source::Entry(task),
            Source::Reduction(red),
            Source::Entry(a2),
            Source::Entry(r2)
        ],
        "the task, then the stand-in, then the work that is kept"
    );
}

#[test]
fn an_anchor_that_is_hidden_falls_back_to_the_first_thing_the_reduction_covers() {
    let (_d, mut s) = store();
    let first = user(&mut s, "first");
    let task = user(&mut s, "task");
    let (a, _r) = bash(&mut s, "c1", "strings x", &dump("x"));
    let red = s
        .append(Kind::Reduction(Reduction {
            mode: Mode::Compact,
            trigger: Trigger::Budget,
            covers: vec![first, a],
            summary: Some("notes".into()),
            stubs: vec![],
            before_tokens: 0,
            after_tokens: 0,
            masks: vec![],
            after: Some(task),
        }))
        .unwrap();
    // Later, a reduction covers the task too (a new prompt came), but not the earlier one.
    let later = user(&mut s, "a newer task");
    s.append(Kind::Reduction(Reduction {
        mode: Mode::Fade,
        trigger: Trigger::Budget,
        covers: vec![task],
        summary: None,
        stubs: vec![],
        before_tokens: 0,
        after_tokens: 0,
        masks: vec![],
        after: None,
    }))
    .unwrap();
    let items = project(&s);
    assert!(
        items.iter().any(|i| i.source == Source::Reduction(red)),
        "the earlier stand-in is still shown"
    );
    assert!(items.iter().any(|i| i.source == Source::Entry(later)));
}

#[test]
fn a_log_written_before_masks_and_anchors_still_reads() {
    let old = r#"{"mode":"auto","trigger":"budget","covers":[1,2],"summary":"s","stubs":[{"entry":2,"what":"tool result","detail":"bash ls","bytes":9}],"before_tokens":10,"after_tokens":5}"#;
    let r: Reduction = serde_json::from_str(old).unwrap();
    assert!(r.masks.is_empty() && r.after.is_none() && r.stubs[0].text.is_none());
    assert!(!r.is_mask());
    // And what is written without them stays as it was.
    let again = serde_json::to_string(&r).unwrap();
    assert!(
        !again.contains("\"masks\"") && !again.contains("\"after\""),
        "{again}"
    );
    let _ = Mask {
        entry: 1,
        stub: String::new(),
        bytes: 0,
    };
}

// -- the cut and its anchor -------------------------------------------------------------------

#[test]
fn the_summarizer_is_given_the_task_too_but_does_not_cover_it() {
    let (_d, mut s) = store();
    user(&mut s, "an older question");
    assistant(&mut s, "an older answer");
    user(&mut s, "THE CURRENT TASK");
    for i in 0..6 {
        bash(&mut s, &format!("c{i}"), "dump", &dump(&format!("d{i}")));
    }
    let items = project(&s);
    let p = plan(0, &items, &cfg(3_000), Mode::Auto).unwrap();
    let prepared = budget::prepare(&s, &items, &p, Trigger::Budget, p.summary_tokens).unwrap();
    let req = prepared.request.clone().unwrap();
    assert!(
        req.transcript.contains("THE CURRENT TASK"),
        "context for the notes"
    );
    let id = budget::commit(&mut s, prepared, Some("notes".into())).unwrap();
    let Kind::Reduction(red) = &s.get(id).unwrap().kind else {
        panic!()
    };
    assert_eq!(red.after, p.anchor);
    assert!(
        !red.covers.contains(&p.anchor.unwrap()),
        "but the task is not covered"
    );
}

#[test]
fn what_is_left_when_only_a_stand_in_is_older_is_nothing_to_reduce() {
    // The task, the stand-in right behind it, and the work in progress: reducing the stand-in
    // again would free next to nothing and repeat until the attempts ran out.
    let (_d, mut s) = store();
    let old = user(&mut s, "older");
    let task = user(&mut s, "TASK");
    s.append(Kind::Reduction(Reduction {
        mode: Mode::Compact,
        trigger: Trigger::Budget,
        covers: vec![old],
        summary: Some("notes".into()),
        stubs: vec![],
        before_tokens: 0,
        after_tokens: 0,
        masks: vec![],
        after: Some(task),
    }))
    .unwrap();
    bash(&mut s, "c0", "dump", &dump("d"));
    let items = project(&s);
    let e = plan(0, &items, &cfg(2_000), Mode::Auto).unwrap_err();
    assert!(e.to_string().contains("already reduced"), "{e}");
}

// -- a fade keeps what the person said ------------------------------------------------------------

fn faded(s: &mut Store, window: u64) -> Reduction {
    let items = project(s);
    let p = plan(0, &items, &cfg(window), Mode::Fade).unwrap();
    let prepared = budget::prepare(s, &items, &p, Trigger::Manual, p.summary_tokens).unwrap();
    let id = budget::commit(s, prepared, None).unwrap();
    let Kind::Reduction(r) = &s.get(id).unwrap().kind else {
        panic!()
    };
    r.clone()
}

#[test]
fn a_fade_keeps_what_the_person_said_as_they_said_it_and_the_calls_with_their_arguments() {
    let (_d, mut s) = store();
    user(
        &mut s,
        "The sample beacons to 10.9.8.7 every 60 seconds; find out why.\nStart with the packer.",
    );
    bash(
        &mut s,
        "c1",
        "objdump -d --start-address=0x401000 ./sample.bin",
        &dump("dis"),
    );
    assistant(&mut s, "it unpacks in memory");
    user(&mut s, "now the current question");
    let r = faded(&mut s, 3_000);

    let said = r.stubs.iter().find(|s| s.what == "user message").unwrap();
    assert_eq!(
        said.text.as_deref(),
        Some(
            "The sample beacons to 10.9.8.7 every 60 seconds; find out why.\nStart with the packer."
        )
    );
    let call = r.stubs.iter().find(|s| s.what == "tool result").unwrap();
    assert!(
        call.detail.contains("--start-address=0x401000") && call.detail.contains("sample.bin"),
        "{}",
        call.detail
    );
    let shown = context::render_reduction(&r);
    assert!(
        shown.contains("> The sample beacons to 10.9.8.7"),
        "{shown}"
    );
    assert!(shown.contains("> Start with the packer."), "{shown}");
}

#[test]
fn what_people_said_is_kept_newest_first_within_a_budget_and_the_older_become_hints() {
    let (_d, mut s) = store();
    let long = |tag: &str| format!("{tag} {}", "words ".repeat(400));
    for i in 0..6 {
        user(&mut s, &long(&format!("MESSAGE{i}")));
        assistant(&mut s, "ok");
    }
    user(&mut s, "the current one");
    // A window of 6,000 tokens: a twentieth is 300 tokens, room for one of these (~600 tokens)
    // only if cut.
    let r = faded(&mut s, 6_000);
    let kept: Vec<&str> = r
        .stubs
        .iter()
        .filter(|s| s.what == "user message" && s.text.is_some())
        .map(|s| s.detail.as_str())
        .collect();
    assert!(!kept.is_empty() && kept.len() < 6, "{kept:?}");
    assert!(
        kept[0].starts_with("MESSAGE5") || kept.iter().any(|d| d.starts_with("MESSAGE5")),
        "the newest of what was dropped is the one kept: {kept:?}"
    );
    let total: usize = r
        .stubs
        .iter()
        .filter_map(|s| s.text.as_ref())
        .map(|t| t.len())
        .sum();
    assert!(
        total / 4 <= 6_000 / 20 + 80,
        "about the budget: {} tokens",
        total / 4
    );
}

#[test]
fn repeated_fades_do_not_carry_every_message_ever_dropped() {
    let (_d, mut s) = store();
    for round in 0..4 {
        for i in 0..3 {
            user(
                &mut s,
                &format!("round {round} message {i}: {}", "blah ".repeat(200)),
            );
            assistant(&mut s, "ok");
        }
        user(&mut s, "and now?");
        let _ = faded(&mut s, 6_000);
    }
    let items = project(&s);
    let Msg::User { text } = &items[0].msg else {
        panic!()
    };
    let kept = text.matches("  > ").count();
    assert!(
        text.len() / 4 < 6_000 / 20 * 3,
        "the stand-in stays small ({} tokens, {kept} quoted lines)",
        text.len() / 4
    );
}

// -- the summary's shape ----------------------------------------------------------------------------

#[test]
fn the_summarizer_is_not_shown_the_models_thinking() {
    let (_d, mut s) = store();
    user(&mut s, "q");
    s.append(Kind::Assistant {
        blocks: vec![
            reactor_agent::entry::Block::Thinking {
                text: "SCRATCHWORK that wanders".into(),
                signature: None,
            },
            reactor_agent::entry::Block::Text {
                text: "the key is XOR 0x5a".into(),
            },
        ],
        model: None,
        usage: None,
        stop: None,
    })
    .unwrap();
    s.append(Kind::Assistant {
        blocks: vec![reactor_agent::entry::Block::Thinking {
            text: "ONLY SCRATCHWORK".into(),
            signature: None,
        }],
        model: None,
        usage: None,
        stop: None,
    })
    .unwrap();
    let t = budget::transcript(&s, &project(&s));
    assert!(t.contains("the key is XOR 0x5a"));
    assert!(!t.contains("SCRATCHWORK"), "{t}");
}

#[test]
fn notes_from_an_earlier_reduction_are_marked_and_the_summarizer_is_told_to_update_them() {
    let (_d, mut s) = store();
    let old = user(&mut s, "older");
    let task = user(&mut s, "task");
    s.append(Kind::Reduction(Reduction {
        mode: Mode::Compact,
        trigger: Trigger::Budget,
        covers: vec![old],
        summary: Some("## Goal\nfind the C2".into()),
        stubs: vec![],
        before_tokens: 0,
        after_tokens: 0,
        masks: vec![],
        after: Some(task),
    }))
    .unwrap();
    for i in 0..4 {
        bash(&mut s, &format!("c{i}"), "dump", &dump("d"));
    }
    let items = project(&s);
    let p = plan(0, &items, &cfg(3_000), Mode::Compact).unwrap();
    assert!(p.covers.iter().any(|c| matches!(c, Source::Reduction(_))));
    let req = budget::prepare(&s, &items, &p, Trigger::Budget, 1_500)
        .unwrap()
        .request
        .unwrap();
    assert!(
        req.transcript.contains("<previous-summary>") && req.transcript.contains("find the C2")
    );
    assert!(
        req.system.contains(budget::SUMMARY_UPDATE),
        "told to update them"
    );

    // A first reduction has no such part and no such instruction.
    let (_d2, s2, _) = worked(6);
    let items = project(&s2);
    let p = plan(0, &items, &cfg(3_000), Mode::Compact).unwrap();
    let req = budget::prepare(&s2, &items, &p, Trigger::Budget, 1_500)
        .unwrap()
        .request
        .unwrap();
    assert!(!req.system.contains(budget::SUMMARY_UPDATE));
    assert!(req.template);
}

#[test]
fn the_notes_are_asked_for_under_headings_and_three_of_the_five_will_do() {
    for h in budget::SUMMARY_HEADINGS {
        assert!(budget::SUMMARY_SYSTEM.contains(h), "{h}");
    }
    assert!(follows_template(
        "## Goal\nx\n## Established\ny\n## Next\nz"
    ));
    assert!(
        follows_template("## goal\nx\n## ESTABLISHED\ny\n## open\nz"),
        "case does not matter"
    );
    assert!(!follows_template("## Goal\nonly this\n## Next\nand this"));
    assert!(!follows_template(
        "The sample is packed and talks to 10.0.0.1."
    ));
}

#[test]
fn the_summary_grows_with_what_it_covers_between_a_floor_and_a_ceiling() {
    let c = BudgetConfig::new(100_000);
    assert_eq!(summary_size(2_000, &c), 1_500, "the floor");
    assert_eq!(summary_size(20_000, &c), 3_400);
    assert_eq!(summary_size(90_000, &c), 5_000, "the ceiling");
    // In a small window the ceiling is an eighth of it.
    let small = BudgetConfig::new(16_000);
    assert_eq!(summary_size(40_000, &small), 2_000);
    // And never below the floor, even where an eighth of the window is less.
    assert_eq!(summary_size(40_000, &BudgetConfig::new(4_000)), 1_500);
}

#[tokio::test]
async fn a_summary_that_is_no_shorter_than_what_it_covers_is_refused() {
    struct Echo;
    impl budget::Summarizer for Echo {
        async fn summarize(&self, req: budget::SummaryRequest) -> reactor_agent::Result<String> {
            Ok(req.transcript)
        }
    }
    let req = budget::SummaryRequest {
        system: "s".into(),
        transcript: "word ".repeat(1_000),
        max_tokens: 1_500,
        ..Default::default()
    };
    let e = budget::summarize(&Echo, req).await.unwrap_err();
    assert!(
        e.to_string().contains("no shorter than what it covers"),
        "{e}"
    );

    // A short transcript is not held to it: there is little to save either way.
    let small = budget::SummaryRequest {
        transcript: "a short exchange".into(),
        ..Default::default()
    };
    assert!(budget::summarize(&Echo, small).await.is_ok());
}

#[test]
fn the_defaults_trigger_at_85_percent_and_keep_28_and_mask_down_to_60() {
    let c = BudgetConfig::new(100_000);
    assert_eq!(
        (c.pct, c.keep, c.mask_target, c.protect),
        (0.85, 0.28, 0.6, 0.15)
    );
    assert!(
        c.keep < c.mask_target && c.mask_target < c.pct,
        "a ladder of thresholds"
    );
    let o = MaskOptions::proactive(&c);
    assert_eq!(o.target_tokens, (0.6 * c.hard() as f64) as u64);
    assert_eq!(o.protect_tokens, 15_000);
    assert!(
        o.require_target && o.spare_unread,
        "the model reads what it asked for first"
    );
    assert!(!MaskOptions::last_resort(&c).require_target);
    assert!(
        !MaskOptions::last_resort(&c).spare_unread,
        "but when nothing else is left, it is cut"
    );
    assert_eq!(MaskOptions::last_resort(&c).protect_tokens, 0);
}

// -- counting from what the provider said (ADR-0058) ----------------------------------------------------

use reactor_agent::context::usage_anchor;
use reactor_agent::entry::{Block, Usage};

fn replied(s: &mut Store, model: &str, input: u64, output: u64) -> u64 {
    s.append(Kind::Assistant {
        blocks: vec![Block::Text {
            text: "an answer".into(),
        }],
        model: Some(model.into()),
        usage: Some(Usage {
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            fixed_estimate: 700,
        }),
        stop: Some("stop".into()),
    })
    .unwrap()
}

#[test]
fn the_newest_count_of_the_same_model_is_the_anchor_and_says_where_it_sits() {
    let (_d, mut s) = store();
    user(&mut s, "first");
    replied(&mut s, "m", 1_000, 50);
    user(&mut s, "second");
    let newest = replied(&mut s, "m", 2_000, 80);
    user(&mut s, "third");
    let items = project(&s);
    let a = usage_anchor(&s, &items, "m").unwrap();
    assert_eq!(a.tokens, 2_080, "what was sent and what came back");
    assert_eq!(a.fixed, 700);
    assert_eq!(items[a.index].source, Source::Entry(newest));
}

#[test]
fn a_count_in_another_models_tokens_is_not_used_and_an_older_one_of_this_model_is() {
    let (_d, mut s) = store();
    user(&mut s, "first");
    let mine = replied(&mut s, "mine", 1_000, 50);
    user(&mut s, "second");
    replied(&mut s, "other", 9_000, 90);
    let items = project(&s);
    let a = usage_anchor(&s, &items, "mine").unwrap();
    assert_eq!(items[a.index].source, Source::Entry(mine));
    assert!(usage_anchor(&s, &items, "nobody").is_none());
}

#[test]
fn a_reduction_or_a_restore_after_the_count_makes_it_stale() {
    let (_d, mut s) = store();
    user(&mut s, "first");
    replied(&mut s, "m", 1_000, 50);
    user(&mut s, "second");
    let items = project(&s);
    assert!(usage_anchor(&s, &items, "m").is_some());

    let (_d2, mut s2, _) = worked(3);
    replied(&mut s2, "m", 5_000, 50);
    let items = project(&s2);
    let p = plan_mask(&s2, 0, &items, &all(0)).unwrap();
    let red = budget::commit_mask(&mut s2, p, Trigger::Budget).unwrap();
    assert!(
        usage_anchor(&s2, &project(&s2), "m").is_none(),
        "the start of the request changed after the count"
    );
    // A reply after the change counts again.
    replied(&mut s2, "m", 3_000, 40);
    assert!(usage_anchor(&s2, &project(&s2), "m").is_some());
    // And undoing the change makes that one stale in its turn.
    s2.append(Kind::Restore { reduction: red }).unwrap();
    assert!(usage_anchor(&s2, &project(&s2), "m").is_none());
}

#[test]
fn an_older_log_with_no_fixed_estimate_still_anchors() {
    let json = r#"{"input_tokens": 100, "output_tokens": 5}"#;
    let u: Usage = serde_json::from_str(json).unwrap();
    assert_eq!(u.fixed_estimate, 0);
    assert!(
        !serde_json::to_string(&u)
            .unwrap()
            .contains("fixed_estimate")
    );
}

// -- the warning when the fixed part is most of the window ----------------------------------------------------

#[test]
fn the_fixed_part_being_most_of_what_the_window_leaves_is_worth_a_warning() {
    let c = BudgetConfig {
        reserve: 1_000,
        ..BudgetConfig::new(11_000)
    }; // 10,000 usable
    assert!(
        budget::fixed_warning(5_000, &c).is_none(),
        "half is not over half"
    );
    let w = budget::fixed_warning(5_001, &c).unwrap();
    assert!(
        w.contains("5001") && w.contains("10000") && w.contains("50%"),
        "{w}"
    );
    assert!(
        w.contains("larger window") && w.contains("toolsets"),
        "it says what to do: {w}"
    );
    assert!(budget::fixed_warning(100, &c).is_none());
    assert!(
        budget::fixed_warning(
            1,
            &BudgetConfig {
                reserve: 5_000,
                ..BudgetConfig::new(5_000)
            }
        )
        .is_none(),
        "no usable budget at all is the budget's to say, not this"
    );
}

// -- what the provider's cache did (ADR-0059) ---------------------------------------------------------------

use reactor_agent::context::cache_report_of;

fn sent(input: u64, read: u64, written: u64) -> Usage {
    Usage {
        input_tokens: input,
        cached_input_tokens: read,
        cache_write_tokens: written,
        ..Default::default()
    }
}

#[test]
fn a_cache_report_counts_reads_writes_and_the_requests_that_read_nothing() {
    const MIN: u64 = 60_000;
    let r = cache_report_of([
        (0, sent(10_000, 0, 10_000)),
        (MIN, sent(11_000, 10_000, 1_000)),
        // Nothing read, a minute later: the prefix changed, not the clock.
        (2 * MIN, sent(12_000, 0, 12_000)),
        // Nothing read after twenty minutes: the cache had expired.
        (22 * MIN, sent(12_500, 0, 12_500)),
    ]);
    assert_eq!((r.replies, r.input, r.read, r.written), (4, 45_500, 10_000, 35_500));
    assert_eq!((r.cold, r.cold_after_pause), (2, 1));
    assert!((r.hit_share() - 10_000.0 / 45_500.0).abs() < 1e-9);
}

#[test]
fn the_first_request_is_not_cold_and_a_reply_with_no_usage_is_not_counted() {
    let r = cache_report_of([(0, sent(5_000, 0, 5_000)), (1, sent(0, 0, 0))]);
    assert_eq!((r.replies, r.cold), (1, 0));
    let none = cache_report_of([]);
    assert!(none.as_text().contains("no reply"));
    assert!(
        cache_report_of([(0, sent(900, 0, 0))])
            .as_text()
            .contains("Nothing was cached"),
        "a report that read and wrote nothing says why that may be"
    );
}

#[test]
fn a_usage_without_a_write_count_reads_as_zero_and_does_not_write_one() {
    let old: Usage = serde_json::from_str(r#"{"input_tokens":10,"output_tokens":2}"#).unwrap();
    assert_eq!(old.cache_write_tokens, 0);
    assert!(!serde_json::to_string(&old).unwrap().contains("cache_write"));
}
