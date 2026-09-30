//! Reading the log back, whole — including everything a reduction hides.
//!
//! This is what makes reduction safe: a summary or a stub can point at `#42`, and
//! `history_read` gets there ([ADR-0036]: originals survive every reduction, or the
//! history tools go blind on precisely the range they exist to reach). The functions
//! here are also what the `history_*` tools and the GUI's tree call.
//!
//! [ADR-0036]: ../../../docs/adr/0036-reactor-owns-its-session-store-format.md

use regex::Regex;

use crate::context::hidden_by;
use crate::entry::{Block, Entry, EntryId, Kind};
use crate::error::{Error, Result};
use crate::store::Store;

/// The text of an entry as the model would read it (or, for a reduction, as it
/// would be told about it).
pub fn text_of(kind: &Kind) -> String {
    match kind {
        Kind::Session { session, cwd, .. } => format!("session {session} in {cwd}"),
        Kind::User { text } => text.clone(),
        Kind::Assistant { blocks, .. } => blocks
            .iter()
            .map(|b| match b {
                Block::Text { text } => text.clone(),
                Block::Thinking { text, .. } => format!("[thinking] {text}"),
                Block::ToolCall { name, arguments, .. } => format!("[call] {name} {arguments}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Kind::ToolResult { content, .. } => content.clone(),
        Kind::Custom { key, data } => format!("{key}: {data}"),
        Kind::Reduction(r) => crate::context::render_reduction(r),
        Kind::Restore { reduction } => format!("restored reduction #{reduction}"),
        Kind::Label { text } => text.clone(),
    }
}

/// The first line of an entry, cut to `n` characters.
pub fn preview(kind: &Kind, n: usize) -> String {
    let text = match kind {
        Kind::ToolResult { name, content, .. } => format!("{name}: {content}"),
        other => text_of(other),
    };
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= n { line.to_string() } else { format!("{}…", line.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

/// `history_index`: one line per entry in `from..=to` (the whole log by default),
/// marking the ones a reduction currently hides.
pub fn index_listing(store: &Store, from: Option<EntryId>, to: Option<EntryId>) -> String {
    let hidden = hidden_by(store);
    let (from, to) = (from.unwrap_or(0), to.unwrap_or(store.len().saturating_sub(1) as EntryId));
    let mut lines = Vec::new();
    for e in store.all().iter().filter(|e| e.id >= from && e.id <= to) {
        let mark = match hidden.get(&e.id) {
            Some(r) => format!(" (hidden by #{r})"),
            None => String::new(),
        };
        lines.push(format!(
            "#{} {} {}B{} {}",
            e.id,
            e.kind.label(),
            full_bytes(e),
            mark,
            preview(&e.kind, 80)
        ));
    }
    if lines.is_empty() { "no entries in that range".to_string() } else { lines.join("\n") }
}

/// The size of the whole of an entry: for a tool result, the blob if it was cut.
fn full_bytes(e: &Entry) -> u64 {
    match &e.kind {
        Kind::ToolResult { blob: Some(b), .. } => b.bytes,
        other => text_of(other).len() as u64,
    }
}

/// `history_read`: an entry in full — a truncated tool output read back from its
/// blob — as lines `offset..offset+limit` (1-based) so a huge one can be paged.
pub fn read(store: &Store, id: EntryId, offset: Option<usize>, limit: Option<usize>) -> Result<String> {
    let entry = store.get(id).ok_or_else(|| Error::Store(format!("no entry #{id}")))?;
    let full = match &entry.kind {
        Kind::ToolResult { blob: Some(b), .. } => String::from_utf8_lossy(&store.read_blob(b)?).into_owned(),
        other => text_of(other),
    };
    let lines: Vec<&str> = full.lines().collect();
    let start = offset.unwrap_or(1).max(1) - 1;
    if start >= lines.len().max(1) && !lines.is_empty() {
        return Err(Error::Store(format!("#{id} has {} lines; offset {} is past the end", lines.len(), start + 1)));
    }
    let end = limit.map(|l| (start + l).min(lines.len())).unwrap_or(lines.len());
    let mut out = lines[start.min(lines.len())..end].join("\n");
    if end < lines.len() {
        out.push_str(&format!("\n… [{} more line(s); continue with offset {}]", lines.len() - end, end + 1));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: EntryId,
    pub kind: &'static str,
    /// 1-based line within the entry's full text.
    pub line: usize,
    pub text: String,
}

/// `history_search`: every line, in every entry ever written (hidden ones and blobs
/// included), matching `pattern`.
pub fn search(store: &Store, pattern: &str, kinds: &[String], limit: usize) -> Result<Vec<Hit>> {
    let re = Regex::new(pattern).map_err(|e| Error::Store(format!("bad pattern: {e}")))?;
    let mut hits = Vec::new();
    'entries: for e in store.all() {
        if !kinds.is_empty() && !kinds.iter().any(|k| k == e.kind.label()) {
            continue;
        }
        if matches!(e.kind, Kind::Session { .. } | Kind::Restore { .. }) {
            continue;
        }
        let full = match &e.kind {
            Kind::ToolResult { blob: Some(b), .. } => String::from_utf8_lossy(&store.read_blob(b)?).into_owned(),
            other => text_of(other),
        };
        for (i, line) in full.lines().enumerate() {
            if re.is_match(line) {
                let text: String = line.chars().take(240).collect();
                hits.push(Hit { id: e.id, kind: e.kind.label(), line: i + 1, text });
                if hits.len() >= limit {
                    break 'entries;
                }
            }
        }
    }
    Ok(hits)
}

pub fn render_hits(hits: &[Hit], limit: usize) -> String {
    if hits.is_empty() {
        return "no matches".into();
    }
    let mut out: Vec<String> = hits.iter().map(|h| format!("#{}:{} ({}) {}", h.id, h.line, h.kind, h.text)).collect();
    if hits.len() >= limit {
        out.push(format!("… stopped at {limit} matches; narrow the pattern"));
    }
    out.join("\n")
}
