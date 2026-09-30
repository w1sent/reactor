//! The budget manager: classification, planning, and the three modes.

mod common;

use std::sync::Mutex;

use common::*;
use reactor_agent::budget::{self, BudgetConfig, Class, Summarizer, SummaryRequest, classify, context_tokens, needs_reduction, plan};
use reactor_agent::context::{Msg, Source, msg_tokens, project};
use reactor_agent::entry::{Kind, Mode, Trigger};
use reactor_agent::store::Store;
use reactor_agent::{Error, Result};

/// Records what it was asked, answers with a canned summary or fails.
struct Scripted {
    answer: std::result::Result<String, String>,
    seen: Mutex<Vec<SummaryRequest>>,
}

impl Scripted {
    fn says(text: &str) -> Self {
        Scripted { answer: Ok(text.into()), seen: Mutex::new(vec![]) }
    }
    fn fails(why: &str) -> Self {
        Scripted { answer: Err(why.into()), seen: Mutex::new(vec![]) }
    }
    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl Summarizer for Scripted {
    async fn summarize(&self, req: SummaryRequest) -> Result<String> {
        self.seen.lock().unwrap().push(req);
        self.answer.clone().map_err(Error::Model)
    }
}

fn big(tag: &str) -> String {
    format!("{tag}: {}", "x".repeat(2_000))
}

/// `n` rounds of (user, assistant→bash, result), then a final user turn.
fn session(n: usize) -> (tempfile::TempDir, Store) {
    let (dir, mut s) = store();
    for i in 0..n {
        user(&mut s, &format!("step {i}: what does function {i} do?"));
        bash(&mut s, &format!("c{i}"), &format!("objdump -d f{i}"), &big(&format!("dump{i}")));
        assistant(&mut s, &format!("function {i} checks the key against a table"));
    }
    user(&mut s, "now summarize everything you found");
    (dir, s)
}

fn cfg(window: u64) -> BudgetConfig {
    BudgetConfig { reserve: 1_000, ..BudgetConfig::new(window) }
}

// -- thresholds ---------------------------------------------------------------------

#[test]
fn the_thresholds_derive_from_window_reserve_and_fractions() {
    let c = BudgetConfig { reserve: 1_000, pct: 0.9, keep: 0.5, ..BudgetConfig::new(11_000) };
    assert_eq!(c.hard(), 10_000);
    assert_eq!(c.trigger(), 9_000);
    assert_eq!(c.keep_budget(), 5_000);
    assert_eq!(BudgetConfig::new(100).hard(), 0, "a window smaller than the reserve saturates");
    assert_eq!(BudgetConfig::new(200_000).mode, Mode::Auto, "auto is the default");
}

#[test]
fn reduction_is_needed_once_the_context_passes_the_trigger() {
    let (_d, s) = session(3);
    let items = project(&s);
    let used = context_tokens(0, &items);
    assert!(!needs_reduction(0, &items, &cfg(used * 2)));
    assert!(needs_reduction(0, &items, &cfg(used / 2 + 1_000)));
    // The fixed part (system prompt, tool specs) counts.
    assert!(needs_reduction(used, &items, &cfg(used + 1_000 + 100)));
}

#[test]
fn tool_results_are_mechanical_and_everything_else_conceptual() {
    assert_eq!(classify(&Msg::ToolResult { call_id: "c".into(), name: "bash".into(), content: "x".into(), is_error: false }), Class::Mechanical);
    assert_eq!(classify(&Msg::User { text: "x".into() }), Class::Conceptual);
    assert_eq!(classify(&Msg::Assistant { blocks: vec![] }), Class::Conceptual);
}

// -- planning -----------------------------------------------------------------------

#[test]
fn a_plan_keeps_a_suffix_that_fits_and_starts_on_a_group_boundary() {
    let (_d, s) = session(10);
    let items = project(&s);
    let c = cfg(8_000);
    let p = plan(0, &items, &c, Mode::Auto).unwrap();

    assert!(p.keep_from > 0 && p.keep_from < items.len());
    assert!(!matches!(items[p.keep_from].msg, Msg::ToolResult { .. }), "the kept part never starts with an orphaned result");
    let suffix: u64 = items[p.keep_from..].iter().map(|i| msg_tokens(&i.msg)).sum();
    assert!(suffix <= c.keep_budget(), "{suffix} > {}", c.keep_budget());
    assert_eq!(p.covers.len(), p.keep_from);
    assert_eq!(p.mechanical.len() + p.conceptual.len(), p.keep_from);
    assert!(p.reclaimed_tokens() > 0);
    assert!(p.needs_summarizer());
    assert!(!plan(0, &items, &c, Mode::Fade).unwrap().needs_summarizer(), "fade never asks a model");
}

#[test]
fn a_plan_never_cuts_away_the_current_instruction() {
    // The last user message is in the keep window even when everything after it is huge.
    let (_d, mut s) = store();
    user(&mut s, "old");
    assistant(&mut s, "older");
    user(&mut s, "THE CURRENT TASK");
    for i in 0..6 {
        bash(&mut s, &format!("c{i}"), "dump", &big("dump"));
    }
    let items = project(&s);
    let p = plan(0, &items, &cfg(3_000), Mode::Auto).unwrap();
    let Source::Entry(id) = items[p.keep_from].source else { panic!() };
    let task = items.iter().position(|i| matches!(&i.msg, Msg::User { text } if text == "THE CURRENT TASK")).unwrap();
    assert!(p.keep_from <= task, "kept from #{id}, but the task is item {task}");
}

#[test]
fn nothing_older_means_no_plan() {
    let (_d, mut s) = store();
    user(&mut s, "only message");
    assert!(plan(0, &project(&s), &cfg(4_000), Mode::Auto).is_err());

    let (_d, mut s) = store();
    user(&mut s, "task");
    bash(&mut s, "c", "ls", "out");
    assert!(plan(0, &project(&s), &cfg(4_000), Mode::Auto).is_err(), "a single group is the latest work");
}

#[test]
fn a_range_that_is_already_reduced_is_not_reduced_again_for_nothing() {
    let (_d, mut s) = session(6);
    let c = cfg(6_000);
    let items = project(&s);
    let p = plan(0, &items, &c, Mode::Fade).unwrap();
    futures_block(budget::reduce(&mut s, &items, &p, Trigger::Budget, &Scripted::says("x"), 100)).unwrap();

    let again = project(&s);
    // Everything older than the keep window is one stand-in now.
    if let Ok(p2) = plan(0, &again, &c, Mode::Fade) {
        assert!(p2.keep_from > 1, "a second pass must reduce something new, not just the stand-in");
    }
}

// -- reducing -------------------------------------------------------------------------

/// Drive one async call to completion without a runtime macro on every test.
fn futures_block<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
}

#[tokio::test]
async fn fade_drops_both_kinds_leaves_stubs_and_never_calls_a_model() {
    let (_d, mut s) = session(8);
    let items = project(&s);
    let p = plan(0, &items, &cfg(8_000), Mode::Fade).unwrap();
    let model = Scripted::says("unused");
    let before = s.len();

    let id = budget::reduce(&mut s, &items, &p, Trigger::Budget, &model, 500).await.unwrap();

    assert_eq!(model.calls(), 0);
    assert_eq!(s.len(), before + 1, "one entry appended, nothing rewritten");
    let Kind::Reduction(r) = &s.get(id).unwrap().kind else { panic!() };
    assert_eq!(r.mode, Mode::Fade);
    assert_eq!(r.summary, None);
    assert_eq!(r.stubs.len(), p.keep_from, "every reduced entry leaves a stub, mechanical and conceptual");
    assert!(r.stubs.iter().any(|st| st.what == "tool result" && st.detail.starts_with("bash {\"command\":\"objdump -d f0\"}")));
    assert!(r.stubs.iter().any(|st| st.what == "user message" && st.detail.starts_with("step 0")));
    assert!(r.after_tokens < r.before_tokens);

    let after = project(&s);
    assert_eq!(after[0].source, Source::Reduction(id));
    assert_eq!(after.len(), 1 + items.len() - p.keep_from);
}

#[tokio::test]
async fn auto_summarizes_with_everything_visible_then_drops_only_mechanical_entries() {
    let (_d, mut s) = session(8);
    let items = project(&s);
    let p = plan(0, &items, &cfg(8_000), Mode::Auto).unwrap();
    let model = Scripted::says("Functions 0..n check the key against a table. Key at 0x4010.");

    let id = budget::reduce(&mut s, &items, &p, Trigger::Budget, &model, 500).await.unwrap();

    // One pass, and the summarizer saw the tool output it is summarizing.
    let seen = model.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].transcript.contains("dump0:"), "the mechanical entries must be visible while summarizing");
    assert!(seen[0].transcript.contains("[#1 user]"), "entries are addressable");
    assert_eq!(seen[0].system, budget::SUMMARY_SYSTEM);

    let Kind::Reduction(r) = &s.get(id).unwrap().kind else { panic!() };
    assert_eq!(r.summary.as_deref(), Some("Functions 0..n check the key against a table. Key at 0x4010."));
    assert!(r.stubs.iter().all(|st| st.what == "tool result"), "conceptual entries are summarized, not stubbed");
    assert_eq!(r.stubs.len(), p.mechanical.len());
    assert!(r.stubs.iter().all(|st| st.bytes >= 2_000));
}

#[tokio::test]
async fn compact_summarizes_everything_and_leaves_no_stubs() {
    let (_d, mut s) = session(8);
    let items = project(&s);
    let p = plan(0, &items, &cfg(8_000), Mode::Compact).unwrap();
    let id = budget::reduce(&mut s, &items, &p, Trigger::Manual, &Scripted::says("notes"), 500).await.unwrap();
    let Kind::Reduction(r) = &s.get(id).unwrap().kind else { panic!() };
    assert!(r.stubs.is_empty());
    assert_eq!(r.trigger, Trigger::Manual);
    // The stand-in still tells the model where the originals are.
    let stand_in = &project(&s)[0];
    let Msg::User { text } = &stand_in.msg else { panic!() };
    assert!(text.contains("history_read"), "{text}");
}

#[tokio::test]
async fn a_failed_summary_appends_nothing_and_says_the_context_did_not_shrink() {
    // ADR-0037 invariant: never continue after a failed reduction.
    let (_d, mut s) = session(8);
    let items = project(&s);
    let p = plan(0, &items, &cfg(8_000), Mode::Auto).unwrap();
    let before = s.len();

    let err = budget::reduce(&mut s, &items, &p, Trigger::Budget, &Scripted::fails("429 rate limited"), 500).await.unwrap_err();

    assert!(matches!(err, Error::Reduction(_)), "{err}");
    assert!(err.to_string().contains("429 rate limited"));
    assert_eq!(s.len(), before, "nothing was written");
    assert_eq!(project(&s).len(), items.len(), "and the projection is unchanged");
}

#[tokio::test]
async fn an_empty_summary_is_a_failure_too() {
    let (_d, mut s) = session(8);
    let items = project(&s);
    let p = plan(0, &items, &cfg(8_000), Mode::Auto).unwrap();
    let err = budget::reduce(&mut s, &items, &p, Trigger::Budget, &Scripted::says("   \n"), 500).await.unwrap_err();
    assert!(err.to_string().contains("returned nothing"));
}

#[tokio::test]
async fn stacked_reductions_carry_earlier_stubs_and_keep_originals_hidden() {
    let (_d, mut s) = session(10);
    let c = cfg(8_000);

    let items = project(&s);
    let p1 = plan(0, &items, &c, Mode::Fade).unwrap();
    let r1 = budget::reduce(&mut s, &items, &p1, Trigger::Budget, &Scripted::says("-"), 500).await.unwrap();
    let Kind::Reduction(first) = &s.get(r1).unwrap().kind else { panic!() };
    let first_stubs = first.stubs.len();

    // More work arrives, then a second reduction over a wider range.
    for i in 10..16 {
        user(&mut s, &format!("step {i}"));
        bash(&mut s, &format!("c{i}"), "objdump", &big("later"));
        assistant(&mut s, "noted");
    }
    user(&mut s, "and now?");
    let items = project(&s);
    let p2 = plan(0, &items, &c, Mode::Fade).unwrap();
    assert!(matches!(items[0].source, Source::Reduction(id) if id == r1));
    let r2 = budget::reduce(&mut s, &items, &p2, Trigger::Budget, &Scripted::says("-"), 500).await.unwrap();

    let Kind::Reduction(second) = &s.get(r2).unwrap().kind else { panic!() };
    assert!(second.covers.contains(&r1), "it supersedes the first");
    assert!(second.stubs.len() > first_stubs, "and carries its stubs forward, so no address is lost");
    let out = project(&s);
    assert!(out.iter().filter(|i| matches!(i.source, Source::Reduction(_))).count() == 1, "one stand-in, not two");
    // Every original is still readable.
    assert!(reactor_agent::history::read(&s, 3, None, None).unwrap().contains("dump0"));
}

#[tokio::test]
async fn a_summary_from_an_earlier_reduction_is_offered_to_the_next_summarizer() {
    let (_d, mut s) = session(10);
    let c = cfg(8_000);
    let items = project(&s);
    let p1 = plan(0, &items, &c, Mode::Auto).unwrap();
    budget::reduce(&mut s, &items, &p1, Trigger::Budget, &Scripted::says("FIRST SUMMARY: key is at 0x4010"), 500).await.unwrap();

    for i in 10..16 {
        user(&mut s, &format!("step {i}"));
        bash(&mut s, &format!("c{i}"), "objdump", &big("later"));
        assistant(&mut s, "noted");
    }
    user(&mut s, "and now?");
    let items = project(&s);
    let p2 = plan(0, &items, &c, Mode::Auto).unwrap();
    let model = Scripted::says("SECOND SUMMARY");
    budget::reduce(&mut s, &items, &p2, Trigger::Budget, &model, 500).await.unwrap();

    let seen = model.seen.lock().unwrap();
    assert!(seen[0].transcript.contains("FIRST SUMMARY: key is at 0x4010"), "findings must survive a second pass");
    assert!(seen[0].transcript.contains("earlier reduction"));
}

#[tokio::test]
async fn the_transcript_caps_a_huge_tool_result_but_keeps_both_ends() {
    let (_d, mut s) = store();
    user(&mut s, "task");
    let huge = format!("HEAD-MARKER\n{}\nTAIL-MARKER", "middle ".repeat(5_000));
    bash(&mut s, "c1", "dump", &huge);
    assistant(&mut s, "ok");
    user(&mut s, "next");
    let t = budget::transcript(&s, &project(&s));
    assert!(t.contains("HEAD-MARKER") && t.contains("TAIL-MARKER"));
    assert!(t.contains("bytes elided"));
    assert!(t.len() < huge.len(), "the summarizer's input is bounded");
}
