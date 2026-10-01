//! The context budget manager: one policy, three modes, checked at every
//! tool-result boundary ([ADR-0037](../../../docs/adr/0037-context-reduction-is-one-budget-manager.md)).
//!
//! ```text
//! keep window     the newest slice of the budget — always sent verbatim
//! --------------  ------------------------------------------------------
//! reclaimable     mechanical entries (tool results)      → fade
//!   range         conceptual entries (user turns,         → summarize
//!                 assistant prose, thinking)
//!
//! mode      mechanical   conceptual
//! fade      drop         drop
//! compact   summarize    summarize
//! auto      drop         summarize          ← the default
//! ```
//!
//! What is *dropped* leaves a stub — tool name, arguments, size, address — never a
//! hole, and the originals stay in the log, so nothing here is destructive. What is
//! *summarized* is summarized in one pass **with everything still visible** to the
//! summarizer: two independent passes would produce a summary missing exactly what
//! it most needed.
//!
//! Planning is pure and costs nothing — classification needs only an entry's kind
//! and size — so a frontend can show what a reduction would do before it runs.

use std::collections::HashMap;
use std::future::Future;

use crate::context::{Item, Msg, Source, estimate_tokens, msg_tokens, render_reduction};
use crate::entry::{Block, EntryId, Kind, Mode, Reduction, Stub, Trigger};
use crate::error::{Error, Result};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetConfig {
    /// The model's context window, in tokens.
    pub window: u64,
    /// Headroom kept free for the reply.
    pub reserve: u64,
    /// Reduce once the context passes this fraction of `window - reserve`.
    pub pct: f64,
    /// Reduce down to this fraction of `window - reserve`: the keep window.
    pub keep: f64,
    pub mode: Mode,
    /// What the stand-in summary is allowed to cost.
    pub summary_tokens: u64,
}

impl BudgetConfig {
    pub fn new(window: u64) -> Self {
        // `pct` is the fade's own default (0.9). `keep` is new: owning both
        // strategies needs a target *below* the trigger, or a reduction that lands
        // just under it fires again on the next tool result.
        BudgetConfig {
            window,
            reserve: 16_384,
            pct: 0.9,
            keep: 0.5,
            mode: Mode::Auto,
            summary_tokens: 1_500,
        }
    }
    pub fn hard(&self) -> u64 {
        self.window.saturating_sub(self.reserve)
    }
    pub fn trigger(&self) -> u64 {
        (self.pct * self.hard() as f64) as u64
    }
    pub fn keep_budget(&self) -> u64 {
        (self.keep * self.hard() as f64) as u64
    }

    /// The same thresholds, in units of `ratio` real tokens per estimated token — the
    /// estimate is four characters to a token, and a provider's tokenizer disagrees
    /// (hex and code run denser), so the loop learns the ratio from reported usage.
    pub fn scaled(&self, ratio: f64) -> BudgetConfig {
        let r = ratio.clamp(0.25, 4.0);
        BudgetConfig {
            window: (self.window as f64 / r) as u64,
            reserve: (self.reserve as f64 / r) as u64,
            summary_tokens: self.summary_tokens,
            ..*self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A tool result: bulk whose conclusion already lives in the next assistant message.
    Mechanical,
    /// Prose, instructions, reasoning: dropping it loses the *why*.
    Conceptual,
}

pub fn classify(msg: &Msg) -> Class {
    match msg {
        Msg::ToolResult { .. } => Class::Mechanical,
        Msg::User { .. } | Msg::Assistant { .. } => Class::Conceptual,
    }
}

/// What the whole request costs: the fixed part (system prompt, tool specs) plus the
/// projected messages.
pub fn context_tokens(fixed: u64, items: &[Item]) -> u64 {
    fixed + items.iter().map(|i| msg_tokens(&i.msg)).sum::<u64>()
}

pub fn needs_reduction(fixed: u64, items: &[Item], cfg: &BudgetConfig) -> bool {
    context_tokens(fixed, items) > cfg.trigger()
}

/// A safe place to cut: the suffix that starts here holds whole tool groups only.
fn is_boundary(item: &Item) -> bool {
    !matches!(item.msg, Msg::ToolResult { .. })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NothingToReduce(pub &'static str);

impl std::fmt::Display for NothingToReduce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// What a reduction would do.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub mode: Mode,
    /// Items `..keep_from` are reduced; `keep_from..` are sent verbatim.
    pub keep_from: usize,
    pub covers: Vec<Source>,
    pub mechanical: Vec<usize>,
    pub conceptual: Vec<usize>,
    pub before_tokens: u64,
    pub estimated_after_tokens: u64,
    pub mechanical_tokens: u64,
    pub conceptual_tokens: u64,
}

impl Plan {
    /// Whether carrying it out needs a model.
    pub fn needs_summarizer(&self) -> bool {
        self.mode != Mode::Fade && !self.covers.is_empty()
    }
    pub fn reclaimed_tokens(&self) -> u64 {
        self.before_tokens
            .saturating_sub(self.estimated_after_tokens)
    }
}

/// Plan a reduction of `items` down to the keep window.
pub fn plan(
    fixed: u64,
    items: &[Item],
    cfg: &BudgetConfig,
    mode: Mode,
) -> std::result::Result<Plan, NothingToReduce> {
    let tokens: Vec<u64> = items.iter().map(|i| msg_tokens(&i.msg)).collect();
    let suffix_budget = cfg.keep_budget().saturating_sub(fixed + cfg.summary_tokens);

    // Never cut away the person's current instruction.
    let last_user = items
        .iter()
        .rposition(|i| matches!(i.msg, Msg::User { .. }) && matches!(i.source, Source::Entry(_)));
    let ceiling = last_user.unwrap_or(items.len().saturating_sub(1));

    // The smallest k (largest suffix) that fits the keep budget from a safe boundary.
    let mut k = None;
    let mut suffix = 0u64;
    for i in (0..items.len()).rev() {
        suffix += tokens[i];
        if suffix > suffix_budget {
            break;
        }
        if is_boundary(&items[i]) {
            k = Some(i);
        }
    }
    // Nothing fits: keep the last group at least, which starts at the last boundary.
    let mut k = k
        .unwrap_or_else(|| items.iter().rposition(is_boundary).unwrap_or(0))
        .min(ceiling);
    while k > 0 && !is_boundary(&items[k]) {
        k -= 1;
    }
    if k == 0 {
        return Err(NothingToReduce(
            "everything left is the latest work; there is nothing older to reduce",
        ));
    }
    if k == 1 && matches!(items[0].source, Source::Reduction(_)) {
        return Err(NothingToReduce("everything older is already reduced"));
    }

    let (mut mechanical, mut conceptual) = (Vec::new(), Vec::new());
    let (mut mech_tokens, mut concept_tokens) = (0, 0);
    for i in 0..k {
        match classify(&items[i].msg) {
            Class::Mechanical => {
                mechanical.push(i);
                mech_tokens += tokens[i];
            }
            Class::Conceptual => {
                conceptual.push(i);
                concept_tokens += tokens[i];
            }
        }
    }
    let before = fixed + tokens.iter().sum::<u64>();
    let after = fixed
        + cfg.summary_tokens.min(concept_tokens + mech_tokens)
        + tokens[k..].iter().sum::<u64>();
    Ok(Plan {
        mode,
        keep_from: k,
        covers: items[..k].iter().map(|i| i.source).collect(),
        mechanical,
        conceptual,
        before_tokens: before,
        estimated_after_tokens: after,
        mechanical_tokens: mech_tokens,
        conceptual_tokens: concept_tokens,
    })
}

// -- summarizing ----------------------------------------------------------------

/// What a summarizer is asked.
#[derive(Debug, Clone)]
pub struct SummaryRequest {
    pub system: String,
    pub transcript: String,
    pub max_tokens: u64,
}

/// Anything that can turn a transcript into a summary. Usually a model call; a
/// test's is a function.
pub trait Summarizer: Send + Sync {
    fn summarize(&self, req: SummaryRequest) -> impl Future<Output = Result<String>> + Send;
}

pub const SUMMARY_SYSTEM: &str = "You compress the earlier part of a reverse-engineering session so the work can continue from the summary alone. Write it as notes for yourself: what was asked, what was established (with concrete names, addresses, hashes, paths, values), what was ruled out and why, what is still open. Keep every finding a later step could depend on; drop narration. Tool output appears in the transcript so that findings in it can be captured -- record the conclusion, not the bytes; entries are labelled #id, and you may cite them as #id so the original can be read back. Reply with the notes only.";

/// The longest a single tool result may be in the transcript handed to a summarizer.
const TRANSCRIPT_TOOL_CAP: usize = 6_000;

fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max / 3).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(max * 2 / 3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "{head}\n… [{} bytes elided] …\n{tail}",
        text.len().saturating_sub(head.len() + tail.len())
    )
}

/// The range as a transcript, every entry visible and addressable.
pub fn transcript(store: &Store, items: &[Item]) -> String {
    let mut out = Vec::new();
    for item in items {
        let label = match item.source {
            Source::Entry(id) => format!("#{id}"),
            Source::Reduction(id) => format!("#{id} (earlier reduction)"),
        };
        out.push(match &item.msg {
            Msg::User { text } => format!("[{label} user]\n{text}"),
            Msg::Assistant { blocks } => {
                let body: Vec<String> = blocks
                    .iter()
                    .map(|b| match b {
                        Block::Text { text } => text.clone(),
                        Block::Thinking { text, .. } => format!("(thinking) {text}"),
                        Block::ToolCall {
                            name, arguments, ..
                        } => format!("-> {name} {arguments}"),
                    })
                    .collect();
                format!("[{label} assistant]\n{}", body.join("\n"))
            }
            Msg::ToolResult { name, content, .. } => {
                // If it was truncated for the model, the summarizer sees the same.
                let _ = store;
                format!(
                    "[{label} tool result: {name}]\n{}",
                    cap(content, TRANSCRIPT_TOOL_CAP)
                )
            }
        });
    }
    out.join("\n\n")
}

impl<T: Summarizer + ?Sized> Summarizer for std::sync::Arc<T> {
    fn summarize(&self, req: SummaryRequest) -> impl Future<Output = Result<String>> + Send {
        (**self).summarize(req)
    }
}

// -- stubs --------------------------------------------------------------------------

fn digest(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 80 {
        s
    } else {
        format!("{}…", s.chars().take(79).collect::<String>())
    }
}

fn first_words(text: &str, n: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= n {
        line.to_string()
    } else {
        format!(
            "{}…",
            line.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    }
}

/// What a dropped entry leaves behind.
fn stub_for(
    store: &Store,
    id: EntryId,
    calls: &HashMap<String, (String, serde_json::Value)>,
) -> Option<Stub> {
    let e = store.get(id)?;
    Some(match &e.kind {
        Kind::ToolResult {
            call_id,
            name,
            content,
            blob,
            ..
        } => {
            let args = calls
                .get(call_id)
                .map(|(_, a)| digest(a))
                .unwrap_or_default();
            Stub {
                entry: id,
                what: "tool result".into(),
                detail: if args.is_empty() {
                    name.clone()
                } else {
                    format!("{name} {args}")
                },
                bytes: blob
                    .as_ref()
                    .map(|b| b.bytes)
                    .unwrap_or(content.len() as u64),
            }
        }
        Kind::User { text } => Stub {
            entry: id,
            what: "user message".into(),
            detail: first_words(text, 60),
            bytes: text.len() as u64,
        },
        Kind::Assistant { .. } => {
            let text = crate::history::text_of(&e.kind);
            Stub {
                entry: id,
                what: "assistant message".into(),
                detail: first_words(&text, 60),
                bytes: text.len() as u64,
            }
        }
        _ => return None,
    })
}

// -- applying -----------------------------------------------------------------------

/// Everything a reduction needs decided before a model is asked anything. Split from
/// the commit so a caller holding the store behind a lock can release it across the
/// summarizer's await.
#[derive(Debug, Clone)]
pub struct Prepared {
    mode: Mode,
    trigger: Trigger,
    covers: Vec<EntryId>,
    carried_summary: Option<String>,
    stubs: Vec<Stub>,
    before_tokens: u64,
    fixed_tokens: u64,
    suffix_tokens: u64,
    /// What to ask the summarizer, when the mode summarizes.
    pub request: Option<SummaryRequest>,
}

/// Decide what a reduction covers, what it drops and what the summarizer must see.
pub fn prepare(
    store: &Store,
    items: &[Item],
    plan: &Plan,
    trigger: Trigger,
    summary_tokens: u64,
) -> Result<Prepared> {
    if plan.covers.is_empty() {
        return Err(Error::Reduction("nothing to reduce".into()));
    }
    let prefix = &items[..plan.keep_from];
    let calls: HashMap<String, (String, serde_json::Value)> = prefix
        .iter()
        .filter_map(|i| match &i.msg {
            Msg::Assistant { blocks } => Some(blocks),
            _ => None,
        })
        .flatten()
        .filter_map(|b| match b {
            Block::ToolCall {
                id,
                name,
                arguments,
            } => Some((id.clone(), (name.clone(), arguments.clone()))),
            _ => None,
        })
        .collect();

    let mut covers: Vec<EntryId> = Vec::new();
    let mut carried_summary: Option<String> = None;
    let mut stubs: Vec<Stub> = Vec::new();
    for src in &plan.covers {
        match src {
            Source::Entry(id) => covers.push(*id),
            Source::Reduction(id) => {
                covers.push(*id);
                if let Some(Kind::Reduction(prev)) = store.get(*id).map(|e| &e.kind) {
                    // Everything the earlier reduction hid stays hidden.
                    covers.extend(prev.covers.iter().copied());
                    carried_summary = prev.summary.clone().or(carried_summary);
                    stubs.extend(prev.stubs.iter().cloned());
                }
            }
        }
    }
    covers.sort_unstable();
    covers.dedup();

    let mut dropped: Vec<usize> = match plan.mode {
        Mode::Fade => plan
            .mechanical
            .iter()
            .chain(plan.conceptual.iter())
            .copied()
            .collect(),
        Mode::Auto => plan.mechanical.clone(),
        Mode::Compact => Vec::new(),
    };
    dropped.sort_unstable();
    for i in dropped {
        if let Source::Entry(id) = prefix[i].source
            && let Some(stub) = stub_for(store, id, &calls)
        {
            stubs.push(stub);
        }
    }

    let all: u64 = items.iter().map(|i| msg_tokens(&i.msg)).sum();
    Ok(Prepared {
        mode: plan.mode,
        trigger,
        covers,
        carried_summary,
        stubs,
        before_tokens: plan.before_tokens,
        fixed_tokens: plan.before_tokens.saturating_sub(all),
        suffix_tokens: items[plan.keep_from..]
            .iter()
            .map(|i| msg_tokens(&i.msg))
            .sum(),
        // Summarize with everything still in view, so a finding in a dump is written
        // down before its bytes leave the context.
        request: plan.needs_summarizer().then(|| SummaryRequest {
            system: SUMMARY_SYSTEM.to_string(),
            transcript: transcript(store, prefix),
            max_tokens: summary_tokens,
        }),
    })
}

/// Record the reduction. Appends one entry and rewrites nothing.
pub fn commit(store: &mut Store, p: Prepared, summary: Option<String>) -> Result<EntryId> {
    // Fade keeps whatever summary an earlier reduction already wrote.
    let summary = summary.or(p.carried_summary);
    let mut reduction = Reduction {
        mode: p.mode,
        trigger: p.trigger,
        covers: p.covers,
        summary,
        stubs: p.stubs,
        before_tokens: p.before_tokens,
        after_tokens: 0,
    };
    reduction.after_tokens =
        p.fixed_tokens + estimate_tokens(&render_reduction(&reduction)) + 8 + p.suffix_tokens;
    store.append(Kind::Reduction(reduction))
}

/// Ask for the summary the request describes, and refuse an empty one.
pub async fn summarize(summarizer: &impl Summarizer, request: SummaryRequest) -> Result<String> {
    let text = summarizer
        .summarize(request)
        .await
        .map_err(|e| Error::Reduction(format!("summarizing failed: {e}")))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(Error::Reduction("the summarizer returned nothing".into()));
    }
    Ok(text)
}

/// Carry out `plan`: summarize (if the mode does), then drop what the mode drops, and
/// record it as a reduction entry. The log gains one entry and loses nothing. On any
/// failure nothing is appended and the error says so — the context did not shrink, so
/// the caller must not continue on it (ADR-0037).
pub async fn reduce(
    store: &mut Store,
    items: &[Item],
    plan: &Plan,
    trigger: Trigger,
    summarizer: &impl Summarizer,
    summary_tokens: u64,
) -> Result<EntryId> {
    let prepared = prepare(store, items, plan, trigger, summary_tokens)?;
    let summary = match &prepared.request {
        Some(req) => Some(summarize(summarizer, req.clone()).await?),
        None => None,
    };
    commit(store, prepared, summary)
}
