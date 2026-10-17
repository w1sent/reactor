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
use crate::entry::{Block, EntryId, Kind, Mask, Mode, Reduction, Stub, Trigger};
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
    /// What the stand-in summary is allowed to cost, at least; it grows with what it covers.
    pub summary_tokens: u64,
    /// Mask old tool results until the context is down to this fraction of
    /// `window - reserve` (ADR-0058). Between the keep window and the trigger, so the next
    /// tool result does not bring a reduction straight back.
    pub mask_target: f64,
    /// The newest this fraction of the *window* is never masked.
    pub protect: f64,
    /// Ask the model what a summary left out, and take its second answer.
    pub verify: bool,
}

/// The most headroom a reply is ever kept: what a large window gets.
pub const MAX_RESERVE: u64 = 16_384;
/// The least, for a window big enough to have any.
pub const MIN_RESERVE: u64 = 1_024;

/// The headroom kept free for the reply when nothing says otherwise: an eighth of the
/// window, between a floor and 16k. A flat 16k leaves no usable budget at all in a window
/// of 16k, and leaves a 32k window with half of it (ADR-0058).
pub fn default_reserve(window: u64) -> u64 {
    (window / 8).clamp(MIN_RESERVE.min(window / 4), MAX_RESERVE)
}

/// The longest a reply may be when nothing says otherwise: a quarter of the window, at most
/// 32k. A thinking model spends part of this thinking.
pub fn default_output(window: u64) -> u64 {
    (window / 4).clamp(1_024, 32_768)
}

impl BudgetConfig {
    pub fn new(window: u64) -> Self {
        // Owning both strategies needs a target *below* the trigger, or a reduction that
        // lands just under it fires again on the next tool result. Eight other harnesses
        // keep 15 to 20% of the window verbatim; half of it spent a summarizer call on every
        // third of a window of new work (ADR-0058).
        BudgetConfig {
            window,
            reserve: default_reserve(window),
            pct: 0.85,
            keep: 0.28,
            mode: Mode::Auto,
            summary_tokens: 1_500,
            mask_target: 0.6,
            protect: 0.15,
            verify: false,
        }
    }

    /// The same budget for another window. A reserve that was only the default for the old
    /// window becomes the default for the new one; a chosen reserve stays.
    pub fn rebased(&self, window: u64) -> BudgetConfig {
        let reserve = if self.reserve == default_reserve(self.window) {
            default_reserve(window)
        } else {
            self.reserve
        };
        BudgetConfig {
            window,
            reserve,
            ..*self
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
    /// The person's last message, when the cut falls after it: it is not covered, and the
    /// stand-in goes right behind it. `None` when the cut falls before it, so it is in the
    /// kept part anyway.
    pub anchor: Option<EntryId>,
    /// What the summary may cost: it grows with what it covers.
    pub summary_tokens: u64,
    /// How much of the person's own words a fade keeps as they said them.
    pub verbatim_tokens: u64,
    /// What the person asked the summary to keep ("the C2 findings"); a fade has no summary.
    pub focus: Option<String>,
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

/// What a summary of `covered` tokens may cost: a sixth of it, between the configured
/// size and 5,000, and never more than an eighth of the window.
pub fn summary_size(covered: u64, cfg: &BudgetConfig) -> u64 {
    let hi = 5_000u64.min(cfg.window / 8).max(cfg.summary_tokens);
    let lo = cfg.summary_tokens.min(hi);
    (covered * 17 / 100).clamp(lo, hi)
}

/// Plan a reduction of `items` down to the keep window.
///
/// The cut may fall inside the current turn: one prompt can drive dozens of tool calls, and
/// a window that could not be reduced until the next prompt would fill up with nothing to
/// take out. The person's last message is kept in view whatever the cut.
pub fn plan(
    fixed: u64,
    items: &[Item],
    cfg: &BudgetConfig,
    mode: Mode,
) -> std::result::Result<Plan, NothingToReduce> {
    let tokens: Vec<u64> = items.iter().map(|i| msg_tokens(&i.msg)).collect();
    let suffix_budget = cfg.keep_budget().saturating_sub(fixed + cfg.summary_tokens);

    let last_user = items
        .iter()
        .rposition(|i| matches!(i.msg, Msg::User { .. }) && matches!(i.source, Source::Entry(_)));

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
    let mut k = k.unwrap_or_else(|| items.iter().rposition(is_boundary).unwrap_or(0));
    while k > 0 && !is_boundary(&items[k]) {
        k -= 1;
    }
    if k == 0 {
        return Err(NothingToReduce(
            "everything left is the latest work; there is nothing older to reduce",
        ));
    }
    // The person's last message stays in view even when the cut falls after it.
    let kept_user = last_user.filter(|&u| u < k);
    let (mut mechanical, mut conceptual) = (Vec::new(), Vec::new());
    let (mut mech_tokens, mut concept_tokens) = (0, 0);
    for i in (0..k).filter(|&i| Some(i) != kept_user) {
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
    let covers: Vec<Source> = (0..k)
        .filter(|&i| Some(i) != kept_user)
        .map(|i| items[i].source)
        .collect();
    if covers.is_empty() {
        return Err(NothingToReduce(
            "everything left is the latest work; there is nothing older to reduce",
        ));
    }
    // What is older is only what an earlier reduction left (it may sit behind the person's
    // last message, which is kept in front of it): reducing that again frees next to nothing,
    // and would be repeated until the attempts ran out.
    if covers.iter().all(|c| matches!(c, Source::Reduction(_))) {
        return Err(NothingToReduce("everything older is already reduced"));
    }
    let anchor = kept_user.and_then(|u| match items[u].source {
        Source::Entry(id) => Some(id),
        Source::Reduction(_) => None,
    });
    let covered = mech_tokens + concept_tokens;
    let summary_tokens = summary_size(covered, cfg);
    let before = fixed + tokens.iter().sum::<u64>();
    let after = fixed
        + summary_tokens.min(covered)
        + tokens[k..].iter().sum::<u64>()
        + kept_user.map_or(0, |u| tokens[u]);
    Ok(Plan {
        mode,
        keep_from: k,
        covers,
        mechanical,
        conceptual,
        before_tokens: before,
        estimated_after_tokens: after,
        mechanical_tokens: mech_tokens,
        conceptual_tokens: concept_tokens,
        anchor,
        summary_tokens,
        verbatim_tokens: cfg.window / 20,
        focus: None,
    })
}

// -- summarizing ----------------------------------------------------------------

/// What a summarizer is asked.
#[derive(Debug, Clone, Default)]
pub struct SummaryRequest {
    pub system: String,
    /// The range as one text: [`parts`](Self::parts) at their usual size.
    pub transcript: String,
    pub max_tokens: u64,
    /// The context window the summarizer runs in, in tokens (0: not known). A server that
    /// sizes its window per request is told it.
    pub window: u64,
    /// The most transcript to send, in estimated tokens (0: no limit). Set by the loop from
    /// the budget; a transcript over it is shrunk before it is sent.
    pub input_cap: u64,
    /// The transcript taken apart, so it can be shrunk without cutting through an entry.
    pub parts: Vec<TranscriptPart>,
    /// The notes are asked to follow [`SUMMARY_HEADINGS`]; a reply that does not is asked
    /// again once.
    pub template: bool,
    /// The main request's own start, so a summarizer that is the main model can be asked in a
    /// way the provider has already done most of the work for.
    pub replay: Option<Replay>,
    /// Once the notes are written, ask what they left out and take the answer.
    pub verify: bool,
}

/// What the main model was sent, up to the end of the range being summarized: the same system
/// prompt, the same tools, the same messages. A summary request that begins with exactly this
/// and then asks for the notes shares its start with the request before it, so a provider that
/// caches the start of a prompt (or a local server that keeps the last one) has the most of it
/// done already ([ADR-0058](../../../docs/adr/0058-reduction-is-a-ladder-and-the-budget-follows-the-window.md)).
#[derive(Debug, Clone)]
pub struct Replay {
    pub system: String,
    pub tools: Vec<crate::llm::ToolSpec>,
    pub messages: Vec<Msg>,
    /// The model it was sent to: only that model's cache holds it.
    pub model: String,
}

/// How an entry may be shrunk to make a transcript fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// A tool result: bulk whose body is cut first.
    Result,
    /// Prose, instructions, reasoning.
    Prose,
    /// What an earlier reduction left: the last thing to go.
    Earlier,
}

/// One entry of a transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptPart {
    /// `#12 tool result: bash`: what the summarizer may cite.
    pub label: String,
    pub body: String,
    pub kind: PartKind,
}

/// Anything that can turn a transcript into a summary. Usually a model call; a
/// test's is a function.
pub trait Summarizer: Send + Sync {
    fn summarize(&self, req: SummaryRequest) -> impl Future<Output = Result<String>> + Send;
}

pub const SUMMARY_SYSTEM: &str = "You compress the earlier part of a reverse-engineering session so the work can continue from the summary alone. Write it as notes for yourself, under exactly these headings:\n## Goal\nwhat was asked, and what the work is for.\n## Established\nwhat is known, with concrete names, addresses, hashes, paths and values, and the #id of the entry that shows it.\n## Ruled out\nwhat was tried or checked and found not to be so, and why.\n## Open\nquestions and leads not yet followed.\n## Next\nthe next steps, in order.\nKeep every finding a later step could depend on; drop narration. Tool output appears in the transcript so that findings in it can be captured -- record the conclusion, not the bytes. Entries are labelled #id; cite them as #id so the original can be read back. Reply with the notes only.";

/// Added when the range holds the notes of an earlier reduction.
pub const SUMMARY_UPDATE: &str = "Part of the transcript is marked <previous-summary>: those are your own earlier notes. Keep what is still true, let the newer entries replace what they contradict, and do not say anything twice.";

/// The headings the notes are asked to have.
pub const SUMMARY_HEADINGS: [&str; 5] = [
    "## Goal",
    "## Established",
    "## Ruled out",
    "## Open",
    "## Next",
];

/// Said once to a summarizer whose reply ignored the headings.
pub const SUMMARY_REMINDER: &str = "Your notes must be under these headings, in this order: ## Goal, ## Established, ## Ruled out, ## Open, ## Next. Write them again that way, and nothing else.";

/// What a verification pass asks of a model that has written notes: the second look at the
/// transcript that finds what the first left out.
pub const SUMMARY_VERIFY: &str = "Critically read the notes you just wrote against the transcript. Did you leave out a name, address, hash, path, value, constraint or instruction that a later step could depend on, or a lead that is still open? If so, write the final notes again under the same headings with it added. If not, write the same notes again exactly. Reply with the notes only.";

/// A warning for the person when the fixed part of every request (system prompt, tools and the
/// blocks generated each time) is most of what the window leaves, since little is then left for
/// the work. Not a refusal: a small window should work, only with less room.
pub fn fixed_warning(fixed: u64, cfg: &BudgetConfig) -> Option<String> {
    let hard = cfg.hard();
    if hard == 0 || fixed * 2 <= hard {
        return None;
    }
    Some(format!(
        "The fixed part of every request (system prompt, tools, case graph, manifest) is about {fixed} tokens, {}% of the {hard} the window leaves. Little is left for the work. Switch off tools or toolsets you do not need, detach the case graph, or use a model with a larger window.",
        fixed * 100 / hard
    ))
}

/// Whether notes use the headings: three of the five is enough (a small model may fold two
/// together), and a reply with none of them has ignored the request.
pub fn follows_template(notes: &str) -> bool {
    let lower = notes.to_lowercase();
    SUMMARY_HEADINGS
        .iter()
        .filter(|h| lower.contains(&h.to_lowercase()))
        .count()
        >= 3
}

/// The longest a single tool result may be in the transcript handed to a summarizer.
const TRANSCRIPT_TOOL_CAP: usize = 6_000;

pub(crate) fn cap(text: &str, max: usize) -> String {
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
    let _ = store;
    render_parts(&transcript_parts(items), TRANSCRIPT_TOOL_CAP)
}

/// The range as entries, each labelled with the `#id` the summarizer may cite.
///
/// Thinking is left out: it is the bulkiest part of a long session, scratch work that
/// may be kept as fact, and a conclusion that matters is in what the model said next.
pub fn transcript_parts(items: &[Item]) -> Vec<TranscriptPart> {
    items
        .iter()
        .filter_map(|item| {
            let (id, earlier) = match item.source {
                Source::Entry(id) => (format!("#{id}"), false),
                Source::Reduction(id) => (format!("#{id} (earlier reduction)"), true),
            };
            Some(match &item.msg {
                Msg::User { text } if earlier => TranscriptPart {
                    label: format!("{id} user"),
                    body: format!("<previous-summary>\n{text}\n</previous-summary>"),
                    kind: PartKind::Earlier,
                },
                Msg::User { text } => TranscriptPart {
                    label: format!("{id} user"),
                    body: text.clone(),
                    kind: PartKind::Prose,
                },
                Msg::Assistant { blocks } => {
                    let body = blocks
                        .iter()
                        .filter_map(|b| match b {
                            Block::Text { text } => Some(text.clone()),
                            Block::Thinking { .. } => None,
                            Block::ToolCall {
                                name, arguments, ..
                            } => Some(format!("-> {name} {arguments}")),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if body.trim().is_empty() {
                        return None;
                    }
                    TranscriptPart {
                        label: format!("{id} assistant"),
                        body,
                        kind: PartKind::Prose,
                    }
                }
                Msg::ToolResult { name, content, .. } => TranscriptPart {
                    label: format!("{id} tool result: {name}"),
                    body: content.clone(),
                    kind: PartKind::Result,
                },
            })
        })
        .collect()
}

/// The entries as one text, a tool result cut to `tool_cap` bytes (0: left out).
pub fn render_parts(parts: &[TranscriptPart], tool_cap: usize) -> String {
    parts
        .iter()
        .map(|p| {
            let body = match p.kind {
                PartKind::Result if tool_cap == 0 => {
                    format!("[{} bytes left out to fit]", p.body.len())
                }
                PartKind::Result => cap(&p.body, tool_cap),
                _ => p.body.clone(),
            };
            format!("[{}]\n{body}", p.label)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The entries as a text of at most about `target` estimated tokens: tool results are cut
/// shorter in steps, then the oldest entries are left out (an earlier reduction's last), and
/// what was left out is said. Always returns something, however small the target.
pub fn fit_parts(parts: &[TranscriptPart], target: u64) -> String {
    let fits = |t: &str| estimate_tokens(t) <= target;
    for tool_cap in [TRANSCRIPT_TOOL_CAP, 1_500, 400, 0] {
        let text = render_parts(parts, tool_cap);
        if fits(&text) {
            return text;
        }
    }
    // Leave out the oldest first: the work nearest the cut is what the next step needs.
    let mut order: Vec<usize> = (0..parts.len())
        .filter(|&i| parts[i].kind != PartKind::Earlier)
        .collect();
    order.extend((0..parts.len()).filter(|&i| parts[i].kind == PartKind::Earlier));
    let mut dropped: Vec<usize> = Vec::new();
    let mut text = render_parts(parts, 0);
    for i in order {
        if parts.len() - dropped.len() <= 1 {
            break;
        }
        dropped.push(i);
        let kept: Vec<TranscriptPart> = parts
            .iter()
            .enumerate()
            .filter(|(j, _)| !dropped.contains(j))
            .map(|(_, p)| p.clone())
            .collect();
        text = format!(
            "[{} older entries left out to fit]\n\n{}",
            dropped.len(),
            render_parts(&kept, 0)
        );
        if fits(&text) {
            return text;
        }
    }
    // One entry left and still too big: cut it to the target.
    cap(&text, (target as usize).saturating_mul(4).max(256))
}

impl<T: Summarizer + ?Sized> Summarizer for std::sync::Arc<T> {
    fn summarize(&self, req: SummaryRequest) -> impl Future<Output = Result<String>> + Send {
        (**self).summarize(req)
    }
}

// -- stubs --------------------------------------------------------------------------

fn digest(v: &serde_json::Value, max: usize) -> String {
    let s = v.to_string();
    if s.chars().count() <= max {
        s
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
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

/// The longest a person's message is kept as they wrote it.
const USER_TEXT_MAX_CHARS: usize = 4_000;

/// What a dropped entry leaves behind. `args_len` is how much of a call's arguments the stub
/// keeps; `verbatim` keeps what a person said in full.
fn stub_for(
    store: &Store,
    id: EntryId,
    calls: &HashMap<String, (String, serde_json::Value)>,
    args_len: usize,
    verbatim: bool,
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
                .map(|(_, a)| digest(a, args_len))
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
                text: None,
            }
        }
        Kind::User { text } => Stub {
            entry: id,
            what: "user message".into(),
            detail: first_words(text, 60),
            bytes: text.len() as u64,
            text: verbatim.then(|| cap(text, USER_TEXT_MAX_CHARS)),
        },
        Kind::Assistant { .. } => {
            let text = crate::history::text_of(&e.kind);
            Stub {
                entry: id,
                what: "assistant message".into(),
                detail: first_words(&text, 60),
                bytes: text.len() as u64,
                text: None,
            }
        }
        _ => return None,
    })
}

/// Keep what people said as they said it, newest first, within `budget` tokens; an older
/// message that does not fit is left as its first words. A fade repeated many times would
/// otherwise carry every message that was ever dropped.
fn limit_verbatim(stubs: &mut [Stub], budget: u64) {
    let mut order: Vec<usize> = (0..stubs.len())
        .filter(|&i| stubs[i].text.is_some())
        .collect();
    order.sort_by_key(|&i| std::cmp::Reverse(stubs[i].entry));
    let mut left = budget;
    for i in order {
        let text = stubs[i].text.take().unwrap_or_default();
        let tokens = estimate_tokens(&text) + 4;
        if tokens <= left {
            left -= tokens;
            stubs[i].text = Some(text);
        } else if left >= 64 {
            // The newest that does not fit whole is cut, rather than lost.
            let keep = (left as usize).saturating_mul(4).saturating_sub(16);
            stubs[i].text = Some(format!(
                "{} …(cut)",
                text.chars().take(keep).collect::<String>()
            ));
            left = 0;
        }
    }
}

// -- masking ------------------------------------------------------------------------

/// A tool result under this is not worth a stub.
pub const MASK_MIN_BYTES: usize = 1_024;
/// What a masked result's stub begins with.
pub const MASK_MARK: &str = "[masked: ";

/// How much of the start and of the end of a result its stub keeps, for a window of
/// `window` tokens: a fiftieth of it, between 120 and 400 characters.
pub fn preview_chars(window: u64) -> usize {
    ((window.saturating_mul(4) / 200) as usize).clamp(120, 400)
}

/// What a masking pass is asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskOptions {
    /// Mask until the context is down to this many tokens.
    pub target_tokens: u64,
    /// The newest this many tokens are never masked.
    pub protect_tokens: u64,
    pub preview_chars: usize,
    /// Mask nothing unless that reaches the target: a partial mask would change the start of
    /// the request, and so the provider's cache, and still leave a summary to be written.
    pub require_target: bool,
    /// Leave alone the results the model has not answered yet: it has not read them.
    pub spare_unread: bool,
}

impl MaskOptions {
    /// The first stage of the ladder: down to the mask target, sparing the newest work.
    pub fn proactive(cfg: &BudgetConfig) -> Self {
        MaskOptions {
            target_tokens: (cfg.mask_target * cfg.hard() as f64) as u64,
            protect_tokens: (cfg.protect * cfg.window as f64) as u64,
            preview_chars: preview_chars(cfg.window),
            require_target: true,
            spare_unread: true,
        }
    }

    /// The last stage: nothing older is left to reduce, so whatever is big is cut, the
    /// newest included, as far as it helps.
    pub fn last_resort(cfg: &BudgetConfig) -> Self {
        MaskOptions {
            target_tokens: cfg.trigger(),
            protect_tokens: 0,
            preview_chars: preview_chars(cfg.window),
            require_target: false,
            spare_unread: false,
        }
    }
}

/// What a masking pass would do.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskPlan {
    pub masks: Vec<Mask>,
    pub before_tokens: u64,
    pub after_tokens: u64,
}

fn mask_stub(
    id: EntryId,
    name: &str,
    args: &str,
    bytes: u64,
    content: &str,
    preview: usize,
) -> String {
    let what = if args.is_empty() {
        name.to_string()
    } else {
        format!("{name} {args}")
    };
    let chars = content.chars().count();
    let head: String = content.chars().take(preview).collect();
    let tail: String = content
        .chars()
        .skip(chars.saturating_sub(preview))
        .collect();
    format!(
        "{MASK_MARK}#{id} {what} -- {bytes} bytes. The output is kept; read it back with `history_read` and #{id}.]\nstart:\n{head}\n...\nend:\n{tail}"
    )
}

/// Plan which tool results to show as stubs: the oldest first, outside the protected newest
/// part, until the context is at the target. A result that was read back with `history_read`
/// goes last, since the model asked for it; one that is small, or already a stub, is left.
/// `None` when nothing would be masked, or (if the target is required) when that is not
/// enough.
pub fn plan_mask(
    store: &Store,
    fixed: u64,
    items: &[Item],
    opts: &MaskOptions,
) -> Option<MaskPlan> {
    let tokens: Vec<u64> = items.iter().map(|i| msg_tokens(&i.msg)).collect();
    let total = fixed + tokens.iter().sum::<u64>();
    if total <= opts.target_tokens {
        return None;
    }
    let need = total - opts.target_tokens;

    let mut protected_from = items.len();
    let mut seen = 0u64;
    for i in (0..items.len()).rev() {
        seen += tokens[i];
        if seen > opts.protect_tokens {
            break;
        }
        protected_from = i;
    }
    // Results the model has not answered yet are never masked first: it has not read them.
    // (Only when nothing else is left, in the last resort, are they cut, and then to what fits.)
    if opts.spare_unread {
        let unread = items
            .iter()
            .rposition(|i| matches!(i.msg, Msg::Assistant { .. }))
            .map_or(items.len(), |a| a + 1);
        protected_from = protected_from.min(unread);
    }

    let calls: HashMap<String, (String, serde_json::Value)> = items
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

    let mut candidates: Vec<usize> = (0..protected_from)
        .filter(|&i| match (&items[i].msg, items[i].source) {
            (Msg::ToolResult { content, .. }, Source::Entry(_)) => {
                content.len() >= MASK_MIN_BYTES && !content.starts_with(MASK_MARK)
            }
            _ => false,
        })
        .collect();
    let read_back =
        |i: usize| matches!(&items[i].msg, Msg::ToolResult { name, .. } if name == "history_read");
    candidates.sort_by_key(|&i| (read_back(i), i));

    // In the last resort the results left are what the model has to work with, so each keeps
    // as much of its start and end as the room allows, not a token's worth.
    let preview_for = {
        let cand: u64 = candidates.iter().map(|&i| tokens[i]).sum();
        let room = opts.target_tokens.saturating_sub(total - cand);
        let per = room / candidates.len().max(1) as u64;
        let roomy = (per.saturating_mul(4).saturating_sub(300) / 2).clamp(0, 6_000) as usize;
        if opts.require_target {
            opts.preview_chars
        } else {
            roomy.max(opts.preview_chars)
        }
    };

    let mut masks = Vec::new();
    let mut saved = 0u64;
    for i in candidates {
        let (
            Msg::ToolResult {
                call_id,
                name,
                content,
                ..
            },
            Source::Entry(id),
        ) = (&items[i].msg, items[i].source)
        else {
            continue;
        };
        let bytes = match store.get(id).map(|e| &e.kind) {
            Some(Kind::ToolResult { blob: Some(b), .. }) => b.bytes,
            _ => content.len() as u64,
        };
        let args = calls
            .get(call_id)
            .map(|(_, a)| digest(a, 160))
            .unwrap_or_default();
        let stub = mask_stub(id, name, &args, bytes, content, preview_for);
        let after = 8 + estimate_tokens(&stub);
        if after >= tokens[i] {
            continue;
        }
        saved += tokens[i] - after;
        masks.push(Mask {
            entry: id,
            stub,
            bytes,
        });
        if saved >= need {
            break;
        }
    }
    if masks.is_empty() || (opts.require_target && saved < need) {
        return None;
    }
    Some(MaskPlan {
        masks,
        before_tokens: total,
        after_tokens: total - saved,
    })
}

/// Record the masking. Appends one entry; the results stay in the log, whole.
pub fn commit_mask(store: &mut Store, plan: MaskPlan, trigger: Trigger) -> Result<EntryId> {
    store.append(Kind::Reduction(Reduction {
        mode: Mode::Fade,
        trigger,
        covers: Vec::new(),
        summary: None,
        stubs: Vec::new(),
        before_tokens: plan.before_tokens,
        after_tokens: plan.after_tokens,
        masks: plan.masks,
        after: None,
    }))
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
    after: Option<EntryId>,
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
    let mut has_earlier = false;
    for src in &plan.covers {
        match src {
            Source::Entry(id) => covers.push(*id),
            Source::Reduction(id) => {
                covers.push(*id);
                has_earlier = true;
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
    // A fade keeps what the person said, and more of what was asked of the tools.
    let fade = plan.mode == Mode::Fade;
    for i in dropped {
        if let Source::Entry(id) = prefix[i].source
            && let Some(stub) = stub_for(store, id, &calls, if fade { 160 } else { 80 }, fade)
        {
            stubs.push(stub);
        }
    }
    if fade {
        limit_verbatim(&mut stubs, plan.verbatim_tokens);
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
            .sum::<u64>()
            + plan.anchor.map_or(0, |a| {
                items
                    .iter()
                    .find(|i| i.source == Source::Entry(a))
                    .map_or(0, |i| msg_tokens(&i.msg))
            }),
        after: plan.anchor,
        // Summarize with everything still in view, so a finding in a dump is written
        // down before its bytes leave the context.
        request: plan.needs_summarizer().then(|| {
            let parts = transcript_parts(prefix);
            SummaryRequest {
                system: {
                    let mut system = SUMMARY_SYSTEM.to_string();
                    if has_earlier {
                        system = format!("{system} {SUMMARY_UPDATE}");
                    }
                    if let Some(focus) = plan.focus.as_deref().filter(|f| !f.trim().is_empty()) {
                        system = format!(
                            "{system} The person asked you to keep this in particular, in full: {}",
                            focus.trim()
                        );
                    }
                    system
                },
                transcript: render_parts(&parts, TRANSCRIPT_TOOL_CAP),
                max_tokens: summary_tokens,
                parts,
                template: true,
                ..Default::default()
            }
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
        masks: Vec::new(),
        after: p.after,
    };
    reduction.after_tokens =
        p.fixed_tokens + estimate_tokens(&render_reduction(&reduction)) + 8 + p.suffix_tokens;
    store.append(Kind::Reduction(reduction))
}

/// Ask for the summary the request describes, and refuse an empty one, or one that is no
/// shorter than what it covers (a summary that restates its source frees nothing and the
/// reduction would be for nothing).
pub async fn summarize(summarizer: &impl Summarizer, request: SummaryRequest) -> Result<String> {
    let covered = estimate_tokens(&request.transcript);
    let text = summarizer
        .summarize(request)
        .await
        .map_err(|e| Error::Reduction(format!("summarizing failed: {e}")))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(Error::Reduction("the summarizer returned nothing".into()));
    }
    if covered >= 400 && estimate_tokens(&text) >= covered {
        return Err(Error::Reduction(
            "the summary is no shorter than what it covers, so summarizing it frees nothing".into(),
        ));
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
