//! From the log to what the model is sent.
//!
//! The log holds everything; the *context* is a projection of the current branch in
//! which reductions have replaced ranges. [`project`] is that projection, and it is a
//! pure function of the log — nothing about a request is remembered anywhere else,
//! which is what makes a reduction undoable and a resumed session identical.
//!
//! A reduction hides the entries it covers and stands in for them with one message,
//! placed where the first hidden entry was. Because it covers whole tool groups
//! (the budget manager aligns it), the projection can never leave a tool call without
//! its result or a result without its call.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::entry::{Block, EntryId, Kind, Reduction};
use crate::store::Store;
use reactor_context::settings::{ContextSettings, MemorySettings};
use reactor_context::{identity, manifest, reporting, scenario};
use reactor_db::manifest::{Author, Field};

/// A message as the loop and the model seam handle it — ours, not rig's.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    User {
        text: String,
    },
    Assistant {
        blocks: Vec<Block>,
    },
    ToolResult {
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Entry(EntryId),
    /// The stand-in for a reduction's range.
    Reduction(EntryId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub source: Source,
    pub msg: Msg,
}

/// The reductions in force on the current branch, oldest first.
///
/// A reduction is in force unless it was restored, or a later reduction covers it
/// (its range grew). Restoring the later one brings the earlier one back.
pub fn active_reductions(store: &Store) -> Vec<EntryId> {
    let mut active: Vec<EntryId> = Vec::new();
    let mut superseded: HashMap<EntryId, Vec<EntryId>> = HashMap::new();
    for e in store.branch() {
        match &e.kind {
            Kind::Reduction(r) => {
                let covered: Vec<EntryId> = r
                    .covers
                    .iter()
                    .copied()
                    .filter(|c| active.contains(c))
                    .collect();
                active.retain(|a| !covered.contains(a));
                superseded.insert(e.id, covered);
                active.push(e.id);
            }
            Kind::Restore { reduction } => {
                if let Some(pos) = active.iter().position(|a| a == reduction) {
                    active.remove(pos);
                    active.extend(superseded.remove(reduction).unwrap_or_default());
                    active.sort_unstable();
                }
            }
            _ => {}
        }
    }
    active
}

/// Every entry a reduction in force hides → the reduction hiding it. A reduction that
/// covers an earlier one hides what that one hid too, however it was recorded.
pub fn hidden_by(store: &Store) -> HashMap<EntryId, EntryId> {
    fn cover(store: &Store, rid: EntryId, id: EntryId, into: &mut HashMap<EntryId, EntryId>) {
        if into.insert(id, rid).is_some() {
            return;
        }
        if let Some(Kind::Reduction(inner)) = store.get(id).map(|e| &e.kind) {
            for c in &inner.covers {
                cover(store, rid, *c, into);
            }
        }
    }
    let mut hidden = HashMap::new();
    for rid in active_reductions(store) {
        if let Some(Kind::Reduction(r)) = store.get(rid).map(|e| &e.kind) {
            for c in &r.covers {
                cover(store, rid, *c, &mut hidden);
            }
        }
    }
    hidden
}

/// The stand-in message of a reduction: what the model reads in place of what it hid.
fn stand_in(store: &Store, rid: EntryId) -> Option<Item> {
    match store.get(rid).map(|e| &e.kind) {
        Some(Kind::Reduction(r)) => Some(Item {
            source: Source::Reduction(rid),
            msg: Msg::User {
                text: render_reduction(r),
            },
        }),
        _ => None,
    }
}

/// The current branch as the model sees it.
pub fn project(store: &Store) -> Vec<Item> {
    let branch = store.branch();
    let active = active_reductions(store);
    let hidden = hidden_by(store);
    let on_branch: HashSet<EntryId> = branch.iter().map(|e| e.id).collect();

    // Tool results shown as stubs in place; the latest reduction to name one wins.
    let mut masked: HashMap<EntryId, &str> = HashMap::new();
    // Reductions whose stand-in goes right after an entry (the person's last message,
    // kept in view), when that entry is in view.
    let mut anchored: HashMap<EntryId, Vec<EntryId>> = HashMap::new();
    let mut anchored_ids: HashSet<EntryId> = HashSet::new();
    for rid in &active {
        let Some(Kind::Reduction(r)) = store.get(*rid).map(|e| &e.kind) else {
            continue;
        };
        for m in &r.masks {
            masked.insert(m.entry, m.stub.as_str());
        }
        if let Some(a) = r.after
            && !r.covers.is_empty()
            && on_branch.contains(&a)
            && !hidden.contains_key(&a)
        {
            anchored.entry(a).or_default().push(*rid);
            anchored_ids.insert(*rid);
        }
    }

    // Where each other reduction's stand-in goes: at its first covered entry on the branch.
    let mut first: HashMap<EntryId, EntryId> = HashMap::new();
    for e in &branch {
        if let Some(rid) = hidden.get(&e.id)
            && !anchored_ids.contains(rid)
        {
            first.entry(*rid).or_insert(e.id);
        }
    }
    let placed_at: HashMap<EntryId, EntryId> = first.iter().map(|(r, e)| (*e, *r)).collect();

    let detached: std::collections::HashSet<EntryId> = branch
        .iter()
        .filter_map(|e| match &e.kind {
            Kind::Detach { attachment } => Some(*attachment),
            _ => None,
        })
        .collect();

    let mut out = Vec::new();
    for e in &branch {
        if let Some(rid) = placed_at.get(&e.id)
            && active.contains(rid)
            && let Some(item) = stand_in(store, *rid)
        {
            out.push(item);
        }
        if hidden.contains_key(&e.id) {
            continue;
        }
        let msg = match &e.kind {
            Kind::User { text } => Msg::User { text: text.clone() },
            Kind::Assistant { blocks, .. } => Msg::Assistant {
                blocks: blocks.clone(),
            },
            Kind::ToolResult {
                call_id,
                name,
                content,
                is_error,
                ..
            } => Msg::ToolResult {
                call_id: call_id.clone(),
                name: name.clone(),
                content: masked
                    .get(&e.id)
                    .map_or_else(|| content.clone(), |stub| stub.to_string()),
                is_error: *is_error,
            },
            Kind::Attachment(_) if detached.contains(&e.id) => continue,
            Kind::Attachment(a) => Msg::User {
                text: render_attachment(a),
            },
            // State, markers and the reductions themselves are not messages.
            Kind::Detach { .. }
            | Kind::Session { .. }
            | Kind::Custom { .. }
            | Kind::Reduction(_)
            | Kind::Restore { .. }
            | Kind::Label { .. } => continue,
        };
        out.push(Item {
            source: Source::Entry(e.id),
            msg,
        });
        for rid in anchored.get(&e.id).into_iter().flatten() {
            if let Some(item) = stand_in(store, *rid) {
                out.push(item);
            }
        }
    }
    out
}

/// What the provider last said a request cost, and where in the projection that reply sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// The index in the projection of the assistant message the count came with.
    pub index: usize,
    /// Real tokens: what that request carried plus what the model replied, which is what the
    /// next request carries up to that point.
    pub tokens: u64,
    /// The loop's estimate of that request's fixed part, or zero if it was not recorded.
    pub fixed: u64,
}

/// The newest count a provider gave that still describes the start of the projection: the
/// latest assistant reply of `model` that is in view and that no reduction, mask or restore has
/// come after. Counting from it is closer than counting everything with an estimate, which is
/// off by whatever the tokenizer does to hex and code ([ADR-0058](../../../docs/adr/0058-reduction-is-a-ladder-and-the-budget-follows-the-window.md)).
pub fn usage_anchor(store: &Store, items: &[Item], model: &str) -> Option<Anchor> {
    let branch = store.branch();
    // Whatever changed the start of the request after a reply makes its count stale.
    let last_change = branch
        .iter()
        .rposition(|e| matches!(e.kind, Kind::Reduction(_) | Kind::Restore { .. }));
    for (pos, e) in branch.iter().enumerate().rev() {
        if last_change.is_some_and(|c| pos < c) {
            return None;
        }
        let Kind::Assistant {
            usage: Some(u),
            model: m,
            ..
        } = &e.kind
        else {
            continue;
        };
        // A count in another model's tokens is not this model's.
        if u.input_tokens == 0 || m.as_deref().is_some_and(|m| m != model) {
            continue;
        }
        if let Some(index) = items.iter().position(|i| i.source == Source::Entry(e.id)) {
            return Some(Anchor {
                index,
                tokens: u.input_tokens + u.output_tokens,
                fixed: u.fixed_estimate,
            });
        }
    }
    None
}

/// What the model reads of an attachment: where it comes from, then the text.
pub fn render_attachment(a: &crate::entry::Attachment) -> String {
    let mut out = format!("## Given to you by the user: {}", a.label);
    if !a.part.is_empty() {
        out.push_str(&format!(" ({})", a.part));
    }
    if a.source != a.label {
        out.push_str(&format!("\nSource: {}", a.source));
    }
    if a.truncated {
        out.push_str("\n(Only the start is shown: it was cut to fit.)");
    }
    out.push_str("\n\n");
    out.push_str(&a.text);
    out
}

/// What stands in for a reduced range. Deterministic: same reduction, same bytes.
pub fn render_reduction(r: &Reduction) -> String {
    if r.is_mask() {
        // Nothing is hidden: this is the list of results that were cut down in place, for a
        // person reading the log.
        let mut out = String::from(
            "## Tool results shown as stubs (the originals are kept; read one back with `history_read` and its #address)\n",
        );
        for m in &r.masks {
            out.push_str(&format!("- #{} ({} bytes)\n", m.entry, m.bytes));
        }
        return out.trim_end().to_string();
    }
    let mut out = String::from("## Earlier in this session (context reduced)\n");
    if let Some(s) = &r.summary {
        out.push('\n');
        out.push_str(s.trim());
        out.push('\n');
    }
    if !r.stubs.is_empty() {
        out.push_str("\nDropped from view (the originals are kept; read one back with `history_read` and its #address):\n");
        for s in &r.stubs {
            match &s.text {
                // What a person said is kept as they said it.
                Some(text) => {
                    out.push_str(&format!("- #{} {} (in full):\n", s.entry, s.what));
                    for line in text.lines() {
                        out.push_str(&format!("  > {line}\n"));
                    }
                }
                None => out.push_str(&format!(
                    "- #{} {}: {} ({} bytes)\n",
                    s.entry, s.what, s.detail, s.bytes
                )),
            }
        }
    } else if r.summary.is_some() {
        out.push_str(&format!(
            "\n(Entries #{}–#{} were summarized; the originals are kept and `history_read` reaches them.)\n",
            r.covers.first().copied().unwrap_or(0),
            r.covers.last().copied().unwrap_or(0)
        ));
    }
    out.trim_end().to_string()
}

// -- sizes ------------------------------------------------------------------------

/// A cheap, deterministic token estimate: four characters to a token. Good enough to
/// decide *when* to reduce; the provider's own count corrects it when it reports one.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// What a projection's messages cost, without the fixed part.
pub fn context_tokens_of(items: &[Item]) -> u64 {
    items.iter().map(|i| msg_tokens(&i.msg)).sum()
}

pub fn msg_tokens(msg: &Msg) -> u64 {
    8 + match msg {
        Msg::User { text } => estimate_tokens(text),
        Msg::Assistant { blocks } => blocks
            .iter()
            .map(|b| match b {
                Block::Text { text } | Block::Thinking { text, .. } => estimate_tokens(text),
                Block::ToolCall {
                    name, arguments, ..
                } => estimate_tokens(name) + estimate_tokens(&arguments.to_string()),
            })
            .sum(),
        Msg::ToolResult { content, name, .. } => estimate_tokens(content) + estimate_tokens(name),
    }
}

/// What an assistant message is made of, in the same estimate [`msg_tokens`] uses.
pub fn assistant_parts(blocks: &[Block]) -> crate::inspect::AssistantParts {
    let mut p = crate::inspect::AssistantParts::default();
    for b in blocks {
        match b {
            Block::Text { text } => p.reply += estimate_tokens(text),
            Block::Thinking { text, .. } => p.thinking += estimate_tokens(text),
            Block::ToolCall {
                name, arguments, ..
            } => p.calls += estimate_tokens(name) + estimate_tokens(&arguments.to_string()),
        }
    }
    p
}

// -- session state ------------------------------------------------------------------

/// The state modules of `reactor-context`, as this session's log holds them: the
/// latest `custom` entry per key on the branch wins.
///
/// One part does not come from the log. The manifest's **goal and guidelines belong to
/// the case** ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)):
/// they are read from it and are the same in every session and on every branch. The
/// manifest's steps and its switch stay the session's.
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub manifest: manifest::State,
    pub identity: identity::State,
    pub reporting: reporting::SessionState,
    pub scenario: Option<scenario::State>,
    /// This session's own layer of the context settings (ADR-0038).
    pub context: ContextSettings,
    /// This session's own layer of the memory switches (ADR-0047).
    pub memory: MemorySettings,
    /// Whether the case has reference documents to search: what makes `docs_search`
    /// appear, so that a case without any pays nothing for it.
    pub docs: bool,
    /// Whether those documents can be searched by meaning: an embedding model is chosen
    /// and has a vector for every passage. Not in the log: the agent sets it from the case
    /// and the chosen model when it gathers a request, and it decides whether the search
    /// tool offers the choice at all.
    pub meaning: bool,
    /// The deferred tools this session has loaded with `tool_search` (ADR-0055): advertised
    /// from then on, on this branch.
    pub loaded: std::collections::BTreeSet<String>,
}

pub const KEY_MANIFEST: &str = "manifest";
pub const KEY_IDENTITY: &str = "identity";
pub const KEY_REPORTING: &str = "reporting";
pub const KEY_SCENARIO: &str = "scenario";
pub const KEY_CONTEXT: &str = "context";
pub const KEY_MEMORY: &str = "memory";
pub const KEY_TOOLS: &str = "tools";

impl SessionState {
    pub fn load(store: &Store) -> SessionState {
        let mut s = SessionState::default();
        for e in store.branch() {
            let Kind::Custom { key, data } = &e.kind else {
                continue;
            };
            match key.as_str() {
                KEY_MANIFEST => s.manifest = manifest::State::normalize(data),
                KEY_IDENTITY => s.identity = identity::State::normalize(data),
                KEY_REPORTING => s.reporting = reporting::SessionState::normalize(data),
                KEY_CONTEXT => s.context = ContextSettings::normalize(data),
                KEY_MEMORY => s.memory = MemorySettings::normalize(data),
                KEY_TOOLS => {
                    s.loaded = data
                        .get("loaded")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default()
                }
                KEY_SCENARIO => {
                    s.scenario = serde_json::from_value(data.clone())
                        .ok()
                        .filter(|_: &scenario::State| !data.is_null())
                }
                _ => {}
            }
        }
        // The strategy is the case's. A database that cannot be read is treated as a
        // case with none set, like every other unreadable preference.
        let case = reactor_db::manifest::current(store.case().db()).unwrap_or_default();
        s.manifest.goal = case.goal;
        s.manifest.guidelines = case.guidelines;
        s.docs = reactor_db::corpus::has_documents(store.case().db()).unwrap_or(false);
        s
    }
}

/// Store the manifest: its goal and guidelines in the case (a new version each, only
/// where they changed), its steps and switch in this session.
pub fn save_manifest(
    store: &mut Store,
    state: &manifest::State,
    author: &Author,
) -> crate::error::Result<()> {
    let (db, session) = (store.case().db().clone(), store.session().to_string());
    for (field, value) in [
        (Field::Goal, state.goal.as_deref()),
        (Field::Guidelines, state.guidelines.as_deref()),
    ] {
        reactor_db::manifest::set(&db, field, value, author, Some(&session))?;
    }
    let session_part = manifest::State {
        goal: None,
        guidelines: None,
        ..state.clone()
    };
    let stored = store
        .branch()
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            Kind::Custom { key, data } if key == KEY_MANIFEST => {
                Some(manifest::State::normalize(data))
            }
            _ => None,
        })
        .unwrap_or_default();
    if stored != session_part {
        save_state(
            store,
            KEY_MANIFEST,
            serde_json::to_value(&session_part).unwrap(),
        )?;
    }
    Ok(())
}

/// Store a state module's value as this session's latest.
pub fn save_state(store: &mut Store, key: &str, data: Value) -> crate::error::Result<EntryId> {
    store.append(Kind::Custom {
        key: key.to_string(),
        data,
    })
}

/// Entry ids covered by reductions in force — handy for the budget manager.
pub fn hidden_ids(store: &Store) -> HashSet<EntryId> {
    hidden_by(store).into_keys().collect()
}

/// What the provider's cache did across the replies on a branch, from the usage each reply
/// recorded ([ADR-0059](../../../docs/adr/0059-the-cache-is-measured-before-it-is-tuned.md)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheReport {
    /// Replies that recorded a usage.
    pub replies: usize,
    /// Everything sent, summed.
    pub input: u64,
    /// The part that came from the cache.
    pub read: u64,
    /// The part written to it.
    pub written: u64,
    /// Replies after the first that read nothing from it: a full-price request.
    pub cold: usize,
    /// Of those, the ones that came more than the cache's lifetime after the reply before.
    pub cold_after_pause: usize,
}

/// How long the default cache holds a prefix (Anthropic's five minutes).
const CACHE_LIFETIME_MS: u64 = 5 * 60 * 1000;

impl CacheReport {
    /// A share of the input that came from the cache, 0 to 1.
    pub fn hit_share(&self) -> f64 {
        if self.input == 0 {
            0.0
        } else {
            self.read as f64 / self.input as f64
        }
    }

    pub fn as_text(&self) -> String {
        if self.replies == 0 {
            return "no reply on this branch has recorded a usage yet".into();
        }
        let mut out = format!(
            "{} replies sent {} input tokens; {} ({:.0}%) came from the provider's cache and {} were written to it.",
            self.replies,
            self.input,
            self.read,
            self.hit_share() * 100.0,
            self.written
        );
        if self.cold > 0 {
            out.push_str(&format!(
                " {} later request{} read nothing from it, {} after a pause longer than five minutes.",
                self.cold,
                if self.cold == 1 { "" } else { "s" },
                self.cold_after_pause
            ));
        }
        if self.read == 0 && self.written == 0 {
            out.push_str(" Nothing was cached: the provider does not report it, caching is off, or the prompt was under the model's minimum.");
        }
        out
    }
}

/// The cache's account of the current branch.
pub fn cache_report(store: &Store) -> CacheReport {
    cache_report_of(store.branch().into_iter().filter_map(|e| match &e.kind {
        Kind::Assistant { usage: Some(u), .. } => Some((e.ts, *u)),
        _ => None,
    }))
}

/// The account of replies given as (time in ms, usage), oldest first.
pub fn cache_report_of(replies: impl IntoIterator<Item = (u64, crate::entry::Usage)>) -> CacheReport {
    let mut r = CacheReport::default();
    let mut previous: Option<u64> = None;
    for (ts, u) in replies {
        if u.input_tokens == 0 {
            continue;
        }
        r.replies += 1;
        r.input += u.input_tokens;
        r.read += u.cached_input_tokens;
        r.written += u.cache_write_tokens;
        if let Some(before) = previous
            && u.cached_input_tokens == 0
        {
            r.cold += 1;
            if ts.saturating_sub(before) > CACHE_LIFETIME_MS {
                r.cold_after_pause += 1;
            }
        }
        previous = Some(ts);
    }
    r
}
