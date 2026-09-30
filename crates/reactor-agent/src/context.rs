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
use reactor_context::settings::ContextSettings;
use reactor_context::{identity, manifest, reporting, scenario};

/// A message as the loop and the model seam handle it — ours, not rig's.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    User { text: String },
    Assistant { blocks: Vec<Block> },
    ToolResult { call_id: String, name: String, content: String, is_error: bool },
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
                let covered: Vec<EntryId> = r.covers.iter().copied().filter(|c| active.contains(c)).collect();
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

/// The current branch as the model sees it.
pub fn project(store: &Store) -> Vec<Item> {
    let branch = store.branch();
    let active = active_reductions(store);
    let hidden = hidden_by(store);

    // Where each reduction's stand-in goes: at its first covered entry on the branch.
    let mut first: HashMap<EntryId, EntryId> = HashMap::new();
    for e in &branch {
        if let Some(rid) = hidden.get(&e.id) {
            first.entry(*rid).or_insert(e.id);
        }
    }
    let placed_at: HashMap<EntryId, EntryId> = first.iter().map(|(r, e)| (*e, *r)).collect();

    let mut out = Vec::new();
    for e in &branch {
        if let Some(rid) = placed_at.get(&e.id)
            && active.contains(rid)
            && let Some(Kind::Reduction(r)) = store.get(*rid).map(|x| &x.kind)
        {
            out.push(Item { source: Source::Reduction(*rid), msg: Msg::User { text: render_reduction(r) } });
        }
        if hidden.contains_key(&e.id) {
            continue;
        }
        let msg = match &e.kind {
            Kind::User { text } => Msg::User { text: text.clone() },
            Kind::Assistant { blocks, .. } => Msg::Assistant { blocks: blocks.clone() },
            Kind::ToolResult { call_id, name, content, is_error, .. } => {
                Msg::ToolResult { call_id: call_id.clone(), name: name.clone(), content: content.clone(), is_error: *is_error }
            }
            // State, markers and the reductions themselves are not messages.
            Kind::Session { .. } | Kind::Custom { .. } | Kind::Reduction(_) | Kind::Restore { .. } | Kind::Label { .. } => continue,
        };
        out.push(Item { source: Source::Entry(e.id), msg });
    }
    out
}

/// What stands in for a reduced range. Deterministic: same reduction, same bytes.
pub fn render_reduction(r: &Reduction) -> String {
    let mut out = String::from("## Earlier in this session (context reduced)\n");
    if let Some(s) = &r.summary {
        out.push('\n');
        out.push_str(s.trim());
        out.push('\n');
    }
    if !r.stubs.is_empty() {
        out.push_str("\nDropped from view (the originals are kept; read one back with `history_read` and its #address):\n");
        for s in &r.stubs {
            out.push_str(&format!("- #{} {}: {} ({} bytes)\n", s.entry, s.what, s.detail, s.bytes));
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
                Block::ToolCall { name, arguments, .. } => estimate_tokens(name) + estimate_tokens(&arguments.to_string()),
            })
            .sum(),
        Msg::ToolResult { content, name, .. } => estimate_tokens(content) + estimate_tokens(name),
    }
}

// -- session state ------------------------------------------------------------------

/// The state modules of `reactor-context`, as this session's log holds them: the
/// latest `custom` entry per key on the branch wins.
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub manifest: manifest::State,
    pub identity: identity::State,
    pub reporting: reporting::SessionState,
    pub scenario: Option<scenario::State>,
    /// This session's own layer of the context settings (ADR-0038).
    pub context: ContextSettings,
}

pub const KEY_MANIFEST: &str = "manifest";
pub const KEY_IDENTITY: &str = "identity";
pub const KEY_REPORTING: &str = "reporting";
pub const KEY_SCENARIO: &str = "scenario";
pub const KEY_CONTEXT: &str = "context";

impl SessionState {
    pub fn load(store: &Store) -> SessionState {
        let mut s = SessionState::default();
        for e in store.branch() {
            let Kind::Custom { key, data } = &e.kind else { continue };
            match key.as_str() {
                KEY_MANIFEST => s.manifest = manifest::State::normalize(data),
                KEY_IDENTITY => s.identity = identity::State::normalize(data),
                KEY_REPORTING => s.reporting = reporting::SessionState::normalize(data),
                KEY_CONTEXT => s.context = ContextSettings::normalize(data),
                KEY_SCENARIO => s.scenario = serde_json::from_value(data.clone()).ok().filter(|_: &scenario::State| !data.is_null()),
                _ => {}
            }
        }
        s
    }
}

/// Store a state module's value as this session's latest.
pub fn save_state(store: &mut Store, key: &str, data: Value) -> crate::error::Result<EntryId> {
    store.append(Kind::Custom { key: key.to_string(), data })
}

/// Entry ids covered by reductions in force — handy for the budget manager.
pub fn hidden_ids(store: &Store) -> HashSet<EntryId> {
    hidden_by(store).into_keys().collect()
}
