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
//!
//! **How much is inline is the agent's to say, within a ceiling.** The default limit is
//! the person's (`toolOutput.maxInlineBytes` in `settings.json`); a call to a built-in
//! tool may ask for another with [`ARG`], up to [`MAX_REQUEST_BYTES`]. An MCP tool's
//! parameters belong to its server, so its output is cut at the default — and the
//! marker carries what `history_read` needs to page the rest at whatever size suits.

use serde_json::Value;

use crate::entry::EntryId;

/// The default for the most model-visible bytes a single tool result may carry.
pub const MAX_INLINE_BYTES: usize = 32 * 1024;
pub const HEAD_BYTES: usize = 8 * 1024;
pub const TAIL_BYTES: usize = MAX_INLINE_BYTES - HEAD_BYTES;
/// The smallest limit there is: below it the marker would be most of the view.
pub const MIN_INLINE_BYTES: usize = 1024;
/// The largest limit a call may ask for. Past it, paging with `history_read` is the way.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// The parameter every built-in tool takes: the most bytes of its output to show inline.
pub const ARG: &str = "max_output_bytes";

/// Where a producer that already cut its own output marks the cut, so the loop can
/// put the real marker (which needs an entry id) in its place.
pub const CUT: &str = "\u{0}CUT\u{0}";

/// How many bytes of one tool result the model sees, and how they split between head
/// and tail (a quarter and three quarters, as the defaults do).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit(usize);

impl Default for Limit {
    fn default() -> Self {
        Limit(MAX_INLINE_BYTES)
    }
}

impl Limit {
    /// A limit of `bytes`, brought into `MIN_INLINE_BYTES..=MAX_REQUEST_BYTES`.
    pub fn new(bytes: usize) -> Limit {
        Limit(bytes.clamp(MIN_INLINE_BYTES, MAX_REQUEST_BYTES))
    }

    /// The limit for one call: what its arguments ask for ([`ARG`]), else `default`.
    pub fn for_call(args: &Value, default: Limit) -> Limit {
        args.get(ARG)
            .and_then(Value::as_u64)
            .map(|n| Limit::new(n.min(usize::MAX as u64) as usize))
            .unwrap_or(default)
    }

    pub fn bytes(self) -> usize {
        self.0
    }
    pub fn head(self) -> usize {
        self.0 / 4
    }
    pub fn tail(self) -> usize {
        self.0 - self.head()
    }

    /// Whether `total` bytes need cutting at all.
    pub fn needs_cut(self, total: u64) -> bool {
        total > self.0 as u64
    }
}

/// The parameter's schema, as a built-in tool advertises it.
pub fn arg_schema(default: Limit) -> Value {
    serde_json::json!({
        "type": "integer",
        "description": format!(
            "Show at most this many bytes (default {}).",
            default.bytes()
        )
    })
}

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
pub fn cut(text: &str, limit: Limit) -> String {
    format!(
        "{}{}{}",
        head(text, limit.head()),
        CUT,
        tail(text, limit.tail())
    )
}

/// Where the whole of a cut output is, and how many lines it has — what a `history_read`
/// call for the rest needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Whole {
    pub entry: EntryId,
    pub lines: u64,
}

/// The marker that replaces the cut: that there was one, and the `history_read` call for
/// `first..=last`, the lines (1-based, as `history_read` counts them) not shown in full.
pub fn marker(entry: EntryId, first: u64, last: u64) -> String {
    let call = if last >= first {
        format!(
            "{{\"id\": {entry}, \"offset\": {first}, \"limit\": {}}}",
            last - first + 1
        )
    } else {
        format!("{{\"id\": {entry}}}")
    };
    format!("\n… [output truncated; the rest: history_read {call}] …\n")
}

/// The model-visible view of an output (`text` may already be a head-and-tail composite
/// with [`CUT`] in it). `whole` is where all of it lives.
pub fn view(text: &str, whole: Whole, limit: Limit) -> String {
    let composite = if text.contains(CUT) {
        text.to_string()
    } else {
        cut(text, limit)
    };
    let (h, t) = composite.split_once(CUT).unwrap_or((&composite, ""));
    // The head's last line may be partial and the tail's first one usually is not, but
    // a one-line overlap costs nothing and a gap would lose a line.
    let first = h.matches('\n').count() as u64 + 1;
    let last = whole.lines.saturating_sub(t.lines().count() as u64) + 1;
    let last = last.min(whole.lines);
    format!("{h}{}{t}", marker(whole.entry, first, last))
}

/// Lines in `bytes`, counted the way `history_read` splits them.
pub fn count_lines(bytes: &[u8]) -> u64 {
    let n = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    match bytes.last() {
        Some(b'\n') | None => n,
        Some(_) => n + 1,
    }
}

/// [`count_lines`] for a file, read in pieces.
pub fn count_lines_in(path: &std::path::Path) -> std::io::Result<u64> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let (mut buf, mut n, mut last) = (vec![0u8; 64 * 1024], 0u64, None);
    loop {
        let got = f.read(&mut buf)?;
        if got == 0 {
            break;
        }
        n += buf[..got].iter().filter(|b| **b == b'\n').count() as u64;
        last = Some(buf[got - 1]);
    }
    Ok(match last {
        Some(b'\n') | None => n,
        Some(_) => n + 1,
    })
}
