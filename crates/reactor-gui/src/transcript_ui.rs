//! The transcript's look: the lane down its left edge, and the rows that are more than text
//! (a tool call, a context reduction).
//!
//! A *lane* is a thin line with one marker per kind of row, so a long transcript can be read
//! by its shapes before its words: a square for you, a filled circle for the agent, a ring for
//! thinking, a small blue dot for a tool, a red one for a failure. A tool call is two lines
//! until it is opened: the command, then one line of what came back. Opened, it shows the
//! whole command and as much of the output as the Tools setting allows.
//!
//! Which rows changed since the last frame is decided here too ([`row_sign`],
//! [`changed_rows`]): only those are measured again, so a streaming reply costs one row, not
//! the whole transcript.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use serde_json::Value;

use gpui_kit::assets::IconName;
use gpui_kit::base::{SelectableText, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;

use reactor_agent::truncate;

use crate::panels::ToolStyle;
use crate::session::ChatItem;

// ---------------------------------------------------------------------------
// Which rows changed
// ---------------------------------------------------------------------------

/// A fingerprint of everything about a row that can change what it looks like: its text, and
/// the flags that switch a spinner for a tick. Two equal fingerprints are the same row.
pub fn row_sign(item: &ChatItem) -> u64 {
    let mut h = DefaultHasher::new();
    match item {
        ChatItem::User { text } => (0u8, text).hash(&mut h),
        ChatItem::AssistantText { text, streaming } => (1u8, text, streaming).hash(&mut h),
        ChatItem::Thinking { text, streaming } => (2u8, text, streaming).hash(&mut h),
        ChatItem::ToolCall {
            name,
            output,
            is_error,
            done,
            ..
        } => (3u8, name, output, is_error, done).hash(&mut h),
        ChatItem::Reduction {
            entry,
            active,
            after,
            summary,
            ..
        } => (4u8, entry, active, after, summary).hash(&mut h),
        ChatItem::Attachment {
            entry,
            active,
            truncated,
            tokens,
            ..
        } => (5u8, entry, active, truncated, tokens).hash(&mut h),
        ChatItem::Error { message } => (6u8, message).hash(&mut h),
        ChatItem::Stats {
            duration_ms,
            tokens,
            ..
        } => (7u8, duration_ms, tokens).hash(&mut h),
    }
    h.finish()
}

/// The runs of rows, among those both lists have, whose fingerprint is not the same.
pub fn changed_rows(old: &[u64], new: &[u64]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (i, (a, b)) in old.iter().zip(new).enumerate() {
        if a != b {
            match out.last_mut() {
                Some(run) if run.end == i => run.end = i + 1,
                _ => out.push(i..i + 1),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The lane
// ---------------------------------------------------------------------------

/// What kind of row a marker stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Node {
    /// No marker: the line runs past (a finished prompt's statistics).
    None,
    User,
    Agent,
    Thinking {
        live: bool,
    },
    Tool(ToolState),
    Document,
    Failure,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// A tool call's state from the two flags the session keeps.
pub fn tool_state(done: bool, is_error: bool) -> ToolState {
    match (done, is_error) {
        (false, _) => ToolState::Running,
        (true, false) => ToolState::Done,
        (true, true) => ToolState::Failed,
    }
}

/// Width of the lane, and the x of its line.
const LANE: f32 = 22.;
const SPINE_X: f32 = 10.;

/// A row with the lane at its left: the line runs the whole height of the row, so rows follow
/// one another without a break, and the marker sits beside the row's first line of text.
pub fn lane(node: Node, content: gpui_kit::Div, theme: &Theme) -> gpui_kit::Div {
    let (top, shape): (f32, Option<gpui_kit::AnyElement>) = match node {
        Node::None => (0., None),
        Node::User => (
            11.,
            Some(
                div()
                    .w(px(10.))
                    .h(px(10.))
                    .rounded_sm()
                    .bg(theme.accent)
                    .into_any_element(),
            ),
        ),
        Node::Agent => (
            9.,
            Some(
                div()
                    .w(px(10.))
                    .h(px(10.))
                    .rounded_full()
                    .bg(theme.foreground)
                    .into_any_element(),
            ),
        ),
        Node::Thinking { live } => (
            9.,
            Some(
                div()
                    .w(px(10.))
                    .h(px(10.))
                    .rounded_full()
                    .border_1()
                    .border_color(if live {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .into_any_element(),
            ),
        ),
        Node::Tool(state) => {
            let colour = if state == ToolState::Failed {
                theme.danger
            } else {
                theme.info
            };
            let dot = div().w(px(8.)).h(px(8.)).rounded_full().bg(colour);
            (
                11.,
                Some(if state == ToolState::Running {
                    // A halo round the dot: it has not finished.
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded_full()
                        .border_1()
                        .border_color(colour.opacity(0.4))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(dot)
                        .into_any_element()
                } else {
                    dot.into_any_element()
                }),
            )
        }
        Node::Document => (
            11.,
            Some(
                div()
                    .w(px(9.))
                    .h(px(9.))
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.muted_foreground)
                    .into_any_element(),
            ),
        ),
        Node::Failure => (
            9.,
            Some(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_full()
                    .bg(theme.danger)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .text_color(theme.background)
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("!")
                    .into_any_element(),
            ),
        ),
    };
    let gutter = div()
        .relative()
        .flex_none()
        .w(px(LANE))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(SPINE_X))
                .w(px(1.))
                .bg(theme.border),
        )
        .children(shape.map(|shape| {
            div()
                .absolute()
                .top(px(top))
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                // The marker hides the line behind it.
                .child(div().py(px(3.)).bg(theme.background).child(shape))
        }));
    h_flex()
        .items_stretch()
        .w_full()
        .pl_1()
        .pr_2()
        .child(gutter)
        .child(content.flex_1().min_w_0())
}

/// The small caps above a run of messages: who is speaking.
pub fn speaker(label: &'static str, colour: gpui_kit::Hsla, theme: &Theme) -> gpui_kit::Div {
    div()
        .text_size(theme.font_size * 0.8)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(colour)
        .child(label)
}

// ---------------------------------------------------------------------------
// A tool call
// ---------------------------------------------------------------------------

/// The command a tool call ran, as text: a shell command as typed, a string as it is, other
/// arguments as compact JSON.
pub fn command_text(args: &Value) -> String {
    match args {
        Value::String(s) => s.clone(),
        Value::Object(o) => match o.get("command").and_then(Value::as_str) {
            Some(command) => command.to_owned(),
            None => serde_json::to_string(args).unwrap_or_default(),
        },
        _ => String::new(),
    }
}

/// Whether `command_text` is a typed command (so it is shown after a `$`).
fn is_shell(args: &Value) -> bool {
    matches!(args, Value::String(_))
        || args
            .as_object()
            .is_some_and(|o| o.get("command").and_then(Value::as_str).is_some())
}

/// A string cut to at most `max` characters, with `…` where it was cut.
fn clip(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// What a collapsed tool call says about its output on its second line.
#[derive(Debug, PartialEq, Eq)]
pub struct Preview {
    /// One line of the output, or a word for none.
    pub line: String,
    /// Lines of output in all.
    pub total: usize,
    /// Lines of output the preview does not show.
    pub more: usize,
}

/// The preview of an output: the first line that says something, or, while the call runs,
/// the latest one — the output as it grows.
pub fn preview(output: &str, running: bool) -> Preview {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let pick = if running {
        lines.iter().rposition(|l| !l.trim().is_empty())
    } else {
        lines.iter().position(|l| !l.trim().is_empty())
    };
    match pick {
        Some(i) => Preview {
            line: clip(lines[i].trim_end(), 240),
            total,
            more: if running { 0 } else { total - i - 1 },
        },
        None => Preview {
            line: String::new(),
            total,
            more: 0,
        },
    }
}

/// `1 line`, `2,004 lines`.
pub fn lines_label(n: usize) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if n == 1 {
        "1 line".into()
    } else {
        format!("{grouped} lines")
    }
}

/// Output cut to `max` bytes on a character boundary, with a note saying where the rest is.
///
/// A result the loop already cut carries its own marker, the `history_read` call for the rest.
/// That is kept, as a head and a tail of the same shares around it, and its lines are made to
/// match what is left, so the call still reads everything that is not on screen.
pub fn cut_output(output: &str, max: usize) -> String {
    if output.len() <= max {
        return output.to_owned();
    }
    if let Some((head, entry, first, last, tail)) = truncate::split_marker(output) {
        let whole = whole_lines(last, tail);
        let room = max.saturating_sub(truncate::marker(entry, first, last).len());
        let head = truncate::head(head, room / 4);
        let tail = truncate::tail(tail, room - head.len());
        let first = head.matches('\n').count() as u64 + 1;
        let last = (whole + 1)
            .saturating_sub(tail.lines().count() as u64)
            .min(whole);
        return format!("{head}{}{tail}", truncate::marker(entry, first, last));
    }
    let mut end = max;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n… truncated (the whole output is in the session log)",
        &output[..end]
    )
}

/// Lines in the whole output a marker stands for: its last hidden line, and the tail after it
/// that starts on that line.
fn whole_lines(last: u64, tail: &str) -> u64 {
    last + (tail.lines().count() as u64).saturating_sub(1)
}

/// The footer's counts for a tool row: the lines on screen and the lines there are. For a
/// result the loop cut, `shown` leaves out the marker and `total` is the stored output's, which
/// the marker's range gives; otherwise both are counted in the text.
pub fn line_counts(output: &str, drawn: &str) -> (usize, usize) {
    match (
        truncate::split_marker(output),
        truncate::split_marker(drawn),
    ) {
        (Some((_, _, _, last, tail)), Some((head, _, _, _, drawn_tail))) => (
            head.lines().count() + drawn_tail.lines().count(),
            whole_lines(last, tail) as usize,
        ),
        _ => (drawn.lines().count(), output.lines().count()),
    }
}

/// What a tool call row draws.
pub struct ToolCallView<'a> {
    pub index: usize,
    pub name: &'a str,
    pub args: &'a Value,
    pub output: &'a str,
    pub is_error: bool,
    pub done: bool,
    /// A context reduction has taken it out of what the model sees.
    pub out_of_context: bool,
    pub expanded: bool,
    /// The copy button was just pressed.
    pub copied: bool,
}

/// A tool call: two lines, and, when opened, the whole command and its output. `docs`
/// replaces the output with a list of passages (a `docs_search` result), `summary` is the line
/// that stands for it while it is closed.
pub fn tool_row(
    v: ToolCallView<'_>,
    docs: Option<(String, gpui_kit::Div)>,
    theme: &Theme,
    tools: &ToolStyle,
    toggle: impl Fn(&mut Window, &mut App) + 'static,
    copy: impl Fn(String, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    let index = v.index;
    let state = tool_state(v.done, v.is_error);
    let colour = if state == ToolState::Failed {
        theme.danger
    } else {
        theme.info
    };
    let small = tools.size * 0.92;

    let full_command = command_text(v.args);
    let first_line = full_command.lines().next().unwrap_or("");
    let command = if full_command.lines().count() > 1 {
        format!("{} …", clip(first_line, 240))
    } else {
        clip(first_line, 240)
    };

    let (line, meta, line_colour) = match &docs {
        Some((summary, _)) => (summary.clone(), String::new(), theme.muted_foreground),
        None => {
            let p = preview(v.output, state == ToolState::Running);
            let meta = if state == ToolState::Running {
                String::new()
            } else if p.total == 0 {
                "no output".to_string()
            } else {
                lines_label(p.total)
            };
            let line = if p.line.is_empty() {
                String::new()
            } else if p.more > 0 {
                format!("{}   +{} more", p.line, p.more)
            } else {
                p.line
            };
            let colour = match state {
                ToolState::Failed => theme.danger,
                ToolState::Running => theme.foreground,
                ToolState::Done => theme.muted_foreground,
            };
            (line, meta, colour)
        }
    };
    let meta = if v.out_of_context {
        if meta.is_empty() {
            "out of context".to_string()
        } else {
            format!("out of context · {meta}")
        }
    } else {
        meta
    };

    let status = match state {
        ToolState::Running => div()
            .flex_none()
            .text_size(small)
            .text_color(theme.muted_foreground)
            .child(crate::indicator::shimmer("running", theme.foreground))
            .into_any_element(),
        ToolState::Done => Icon::new(IconName::Check)
            .small()
            .text_color(theme.success)
            .into_any_element(),
        ToolState::Failed => Icon::new(IconName::Close)
            .small()
            .text_color(theme.danger)
            .into_any_element(),
    };

    let header = v_flex()
        .id(("tool-toggle", index))
        .cursor_pointer()
        .rounded_md()
        .px_2()
        .py_1()
        .hover(|s| s.bg(theme.list_hover))
        .when(state == ToolState::Failed, |el| {
            el.bg(theme.danger.opacity(0.08))
        })
        .on_click(move |_, window, cx| toggle(window, cx))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(if v.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .small()
                    .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(tools.family.clone())
                        .text_size(tools.size)
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(colour)
                        .child(v.name.to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(tools.family.clone())
                        .text_size(tools.size)
                        .text_color(theme.foreground)
                        .child(SharedString::from(command)),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(tools.family.clone())
                        .text_size(small)
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(meta)),
                )
                .child(status),
        )
        .when(!line.is_empty(), |el| {
            el.child(
                div()
                    .pl(px(22.))
                    .truncate()
                    .font_family(tools.family.clone())
                    .text_size(small)
                    .text_color(line_colour)
                    .child(SharedString::from(line)),
            )
        })
        // Seen by the window tests, which click it; nothing outside them.
        .test_support();

    let body = v.expanded.then(|| {
        let well = gpui_kit::Hsla {
            l: (theme.background.l - 0.02).max(0.0),
            ..theme.background
        };
        let text_colour = if v.is_error {
            theme.danger
        } else {
            theme.foreground
        };
        let shell = is_shell(v.args);
        let cut = cut_output(v.output, tools.max_chars);
        let (shown, total) = line_counts(v.output, &cut);
        let footer = format!(
            "{}{}",
            if shown < total {
                format!("{shown} of ")
            } else {
                String::new()
            },
            lines_label(total)
        );
        let raw = v.output.to_owned();
        let output_block = match docs {
            Some((_, list)) => div().px_3().py_2().child(list),
            None if v.output.is_empty() => div()
                .px_3()
                .py_2()
                .text_size(small)
                .text_color(theme.muted_foreground)
                .child(if state == ToolState::Running {
                    "no output yet"
                } else {
                    "no output"
                }),
            None => div()
                .px_3()
                .py_2()
                .text_color(text_colour)
                .font_family(tools.family.clone())
                .text_size(tools.size)
                // Selectable, and whole: what is copied is the text as it came.
                .child(
                    SelectableText::new(("tool-output", index), cut)
                        .document_order(index as u64 * 10 + 1)
                        .text_style(TextStyleRefinement {
                            color: Some(text_colour),
                            ..Default::default()
                        }),
                ),
        };
        v_flex()
            .ml(px(26.))
            .mt_0p5()
            .mb_1()
            .rounded_md()
            .border_1()
            .border_color(if v.is_error {
                theme.danger.opacity(0.4)
            } else {
                theme.border
            })
            .bg(well)
            .overflow_hidden()
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_start()
                    .border_b_1()
                    .border_color(theme.border)
                    .font_family(tools.family.clone())
                    .text_size(tools.size)
                    .when(shell, |el| {
                        el.child(div().text_color(theme.muted_foreground).child("$"))
                    })
                    .child(
                        div().flex_1().min_w_0().child(
                            // Selectable, and whole: what is copied is the command as it ran.
                            SelectableText::new(("tool-command", index), full_command)
                                .document_order(index as u64 * 10),
                        ),
                    ),
            )
            .child(output_block)
            .when(!raw.is_empty(), |el| {
                el.child(
                    h_flex()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_size(small)
                        .text_color(theme.muted_foreground)
                        .child(div().font_family(tools.family.clone()).child(footer))
                        .child(div().flex_1())
                        .child(
                            Button::new(("tool-copy", index))
                                .ghost()
                                .small()
                                .icon(if v.copied {
                                    IconName::Check
                                } else {
                                    IconName::Copy
                                })
                                .label(if v.copied { "Copied" } else { "Copy" })
                                .tooltip("Copy the whole output")
                                .on_click(move |_, window, cx| copy(raw.clone(), window, cx)),
                        ),
                )
            })
    });

    v_flex().child(header).children(body)
}

// ---------------------------------------------------------------------------
// A context reduction
// ---------------------------------------------------------------------------

/// What a reduction row says.
pub struct ReductionView<'a> {
    pub entry: u64,
    pub index: usize,
    pub mode: &'a str,
    pub trigger: &'a str,
    pub covers: usize,
    pub before: u64,
    pub after: u64,
    pub active: bool,
    pub summary: Option<&'a str>,
}

/// A reduction is a seam across the transcript, with no lane: everything above it may be out
/// of the model's context, and the seam says by how much.
pub fn reduction_seam(
    v: ReductionView<'_>,
    theme: &Theme,
    restore: impl Fn(u64, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    let line = theme.info.opacity(0.35);
    let entry = v.entry;
    let rule = || div().flex_1().h(px(1.)).bg(line);
    v_flex()
        .px_2()
        .py_1()
        .gap_1()
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .child(rule())
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pl_3()
                        .pr_1()
                        .py_0p5()
                        .rounded_full()
                        .border_1()
                        .border_color(line)
                        .child(
                            div()
                                .text_color(theme.info)
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(if v.active {
                                    "context reduced"
                                } else {
                                    "reduction undone"
                                }),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .text_size(theme.font_size * 0.85)
                                .child(format!(
                                    "{} · {} · {} entries · ~{} → ~{} tokens",
                                    v.mode, v.trigger, v.covers, v.before, v.after
                                )),
                        )
                        .when(v.active, |el| {
                            el.child(
                                Button::new(("undo-reduction", v.index))
                                    .label("undo")
                                    .small()
                                    .tooltip("Bring the originals back into the model's context")
                                    .on_click(move |_, window, cx| restore(entry, window, cx)),
                            )
                        }),
                )
                .child(rule()),
        )
        .when_some(v.summary.filter(|_| v.active), |el, summary| {
            el.child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.85)
                    .child(summary.to_owned()),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use serde_json::Value;

    use super::{
        ToolState, changed_rows, command_text, cut_output, is_shell, lines_label, preview,
        row_sign, tool_state,
    };
    use crate::session::ChatItem;

    #[test]
    fn a_closed_call_previews_the_first_line_that_says_something() {
        let p = preview("\n\nlicence.dat\nlicence expired\n", false);
        assert_eq!(p.line, "licence.dat");
        assert_eq!(p.total, 4);
        assert_eq!(p.more, 1);
    }

    #[test]
    fn a_running_call_previews_its_latest_line() {
        let p = preview("writing node 1 of 2…\nwriting node 2 of 2…\n\n", true);
        assert_eq!(p.line, "writing node 2 of 2…");
        assert_eq!(p.more, 0);
    }

    #[test]
    fn no_output_is_no_line() {
        let p = preview("", false);
        assert_eq!((p.line.as_str(), p.total, p.more), ("", 0, 0));
        assert_eq!(preview("  \n\n", false).line, "");
    }

    #[test]
    fn a_long_line_is_clipped_on_a_character_boundary() {
        let p = preview(&"é".repeat(500), false);
        assert_eq!(p.line.chars().count(), 241);
        assert!(p.line.ends_with('…'));
    }

    #[test]
    fn lines_are_counted_with_separators() {
        assert_eq!(lines_label(1), "1 line");
        assert_eq!(lines_label(48), "48 lines");
        assert_eq!(lines_label(2004), "2,004 lines");
        assert_eq!(lines_label(1_234_567), "1,234,567 lines");
    }

    #[test]
    fn cut_output_keeps_a_character_whole() {
        let s = "aé".repeat(10);
        let cut = cut_output(&s, 4);
        assert!(cut.starts_with("aéa"));
        assert!(cut.contains("truncated"));
        assert_eq!(cut_output("short", 100), "short");
    }

    #[test]
    fn a_cut_the_loop_made_keeps_its_marker_and_its_lines_true() {
        use reactor_agent::truncate::{Whole, split_marker, view};
        let text: String = (1..=400).map(|n| format!("line {n:03}\n")).collect();
        let whole = Whole {
            entry: 7,
            lines: 400,
        };
        let seen = view(&text, whole, reactor_agent::truncate::Limit::new(2048));
        let cut = cut_output(&seen, 600);
        assert!(!cut.contains("session log"));
        assert!(cut.len() <= 600 + 16);
        let (head, entry, first, last, tail) = split_marker(&cut).expect("the marker is kept");
        assert_eq!(entry, 7);
        // What is not on screen is exactly what the call reads: no gap at either end.
        assert_eq!(first, head.lines().count() as u64 + 1);
        assert_eq!(last, 400 - tail.lines().count() as u64 + 1);
        assert!(tail.ends_with("line 400\n"));
        // The footer counts the stored output, not the view of it.
        let (shown, total) = super::line_counts(&seen, &cut);
        assert_eq!(total, 400);
        assert_eq!(shown, head.lines().count() + tail.lines().count());
        assert_eq!(super::line_counts("a\nb\n", "a\nb\n"), (2, 2));
    }

    #[test]
    fn a_command_is_what_was_typed() {
        assert_eq!(command_text(&Value::String("ls".into())), "ls");
        assert_eq!(
            command_text(&serde_json::json!({"command": "ls -l"})),
            "ls -l"
        );
        assert_eq!(
            command_text(&serde_json::json!({"path": "a"})),
            r#"{"path":"a"}"#
        );
        assert!(is_shell(&serde_json::json!({"command": "ls"})));
        assert!(!is_shell(&serde_json::json!({"path": "a"})));
    }

    #[test]
    fn only_the_rows_that_changed_are_named() {
        assert!(changed_rows(&[1, 2, 3], &[1, 2, 3]).is_empty());
        assert_eq!(changed_rows(&[1, 2, 3], &[1, 9, 3]), vec![1..2]);
        assert_eq!(changed_rows(&[1, 2, 3, 4], &[9, 9, 3, 8]), vec![0..2, 3..4]);
        // Rows only one list has are not compared: they are appended or removed.
        assert_eq!(
            changed_rows(&[1, 2], &[1, 2, 3, 4]),
            Vec::<Range<usize>>::new()
        );
    }

    #[test]
    fn a_row_changes_when_its_text_or_its_state_does() {
        let call = |output: &str, done| ChatItem::ToolCall {
            call_id: "c".into(),
            name: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
            output: output.into(),
            is_error: false,
            done,
        };
        assert_eq!(row_sign(&call("a", false)), row_sign(&call("a", false)));
        assert_ne!(row_sign(&call("a", false)), row_sign(&call("ab", false)));
        assert_ne!(row_sign(&call("a", false)), row_sign(&call("a", true)));
        let said = |text: &str, streaming| ChatItem::AssistantText {
            text: text.into(),
            streaming,
        };
        assert_ne!(row_sign(&said("x", true)), row_sign(&said("x", false)));
        assert_ne!(
            row_sign(&said("one two", false)),
            row_sign(&said("two one", false))
        );
    }

    #[test]
    fn a_tool_is_running_until_it_is_done() {
        assert_eq!(tool_state(false, false), ToolState::Running);
        assert_eq!(tool_state(true, false), ToolState::Done);
        assert_eq!(tool_state(true, true), ToolState::Failed);
    }
}
