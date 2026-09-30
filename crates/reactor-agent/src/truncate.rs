//! The truncation policy for tool output.
//!
//! A tool may print a great deal; the model sees a bounded view and the whole of it is
//! kept (a blob in the session, readable with `history_read` and searchable with
//! `history_search`). The view is the **head and the tail**: the head shows what the
//! command was doing, the tail shows how it ended — the error, the prompt, the final
//! line — and the middle of a `strings` dump or a `logcat` is rarely what matters.
//! The tail gets the larger share for that reason.
//!
//! The marker names the entry the whole output lives in, so the model can tell that
//! more exists and where. That is the same address a fade stub carries
//! ([ADR-0037](../../../docs/adr/0037-context-reduction-is-one-budget-manager.md)):
//! truncation and reduction meet at one address format.

use crate::entry::EntryId;

/// The most model-visible bytes a single tool result may carry.
pub const MAX_INLINE_BYTES: usize = 32 * 1024;
pub const HEAD_BYTES: usize = 8 * 1024;
pub const TAIL_BYTES: usize = MAX_INLINE_BYTES - HEAD_BYTES;

/// Where a producer that already cut its own output marks the cut, so the loop can
/// put the real marker (which needs an entry id) in its place.
pub const CUT: &str = "\u{0}CUT\u{0}";

/// The first `budget` bytes, ending on a line boundary if one is near, and a char boundary always.
pub fn head(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut end = budget;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    match text[..end].rfind('\n') {
        Some(nl) if nl > budget / 2 => &text[..=nl],
        _ => &text[..end],
    }
}

/// The last `budget` bytes, starting on a line boundary if one is near.
pub fn tail(text: &str, budget: usize) -> &str {
    if text.len() <= budget {
        return text;
    }
    let mut start = text.len() - budget;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    match text[start..].find('\n') {
        Some(nl) if nl < budget / 2 => &text[start + nl + 1..],
        _ => &text[start..],
    }
}

/// Head and tail of `text`, joined at [`CUT`].
pub fn cut(text: &str) -> String {
    format!("{}{}{}", head(text, HEAD_BYTES), CUT, tail(text, TAIL_BYTES))
}

/// The marker that replaces the cut.
pub fn marker(elided: u64, total: u64, entry: EntryId) -> String {
    format!(
        "\n… [{elided} of {total} bytes elided -- the whole output is #{entry}; read it with history_read, search it with history_search] …\n"
    )
}

/// The model-visible view of an output of `total` bytes (`text` may already be a
/// head-and-tail composite with [`CUT`] in it). `entry` is where the whole lives.
pub fn view(text: &str, total: u64, entry: EntryId) -> String {
    let composite = if text.contains(CUT) { text.to_string() } else { cut(text) };
    let (h, t) = composite.split_once(CUT).unwrap_or((&composite, ""));
    let elided = total.saturating_sub((h.len() + t.len()) as u64);
    format!("{h}{}{t}", marker(elided, total, entry))
}

/// Whether `total` bytes need cutting at all.
pub fn needs_cut(total: u64) -> bool {
    total > MAX_INLINE_BYTES as u64
}
