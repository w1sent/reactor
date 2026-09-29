//! The docked panels of gui/SPEC.md §6.
//!
//! Every panel is a *view* over [`ReactorApp`]'s state — stateless
//! presentation (gui/SPEC.md §4.4): it upgrades the weak handle, reads, and
//! renders. Nothing here owns facts; actions dispatch back through the app.
//! The weak handle reaches panels as `cx.weak_entity()` from
//! `ReactorApp::new`, where the dock is assembled.

use serde_json::Value;

use gpui_kit::assets::IconName;
use gpui_kit::base::{SelectableText, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Window, div, px,
};

use crate::app::ReactorApp;
use crate::contract::Placement as ViewPlacement;
use crate::session::{AgentPhase, ChatItem, content_text};
use crate::views;

/// One activation row — a leading dot for on/off, a name, an optional muted
/// detail note, and an enable/disable button — shared by the Tools and
/// Toolsets panels (and anything else that is simply "on or off") so the
/// same kind of status is never drawn two different ways. Before this, Tools
/// used a presence dot that had nothing to do with activation and Toolsets
/// used a plain "on"/"off" text label with no dot at all — two different
/// visual languages for the identical question ("is this active?").
///
/// `detail` is for a secondary fact that is *not* the on/off state itself —
/// Tools uses it for "not installed", Toolsets for the tool count.
fn activation_row(
    id: impl Into<SharedString>,
    active: bool,
    label: impl Into<SharedString>,
    detail: Option<SharedString>,
    theme: &Theme,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id = id.into();
    h_flex()
        .id(id.clone())
        .justify_between()
        .items_center()
        .px_2()
        .py_1()
        .rounded_md()
        .hover(|style| style.bg(theme.list_hover))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_color(if active {
                            theme.success
                        } else {
                            theme.muted_foreground
                        })
                        .child(if active { "●" } else { "○" }),
                )
                .child(
                    // A tool/toolset id (`bn`, `angr`, `triage`, …) is a
                    // literal command-line name, not prose — monospace fits
                    // the domain (a reverse-engineering/analysis toolbox)
                    // the same way it already does for inline code and tool
                    // output.
                    Label::new(label.into())
                        .font_family(theme.mono_font_family.clone())
                        .text_color(if active {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        }),
                )
                .when_some(detail, |el, detail| {
                    el.child(
                        Label::new(detail)
                            .text_color(theme.muted_foreground)
                            .text_size(theme.font_size * 0.8),
                    )
                }),
        )
        .child(
            Button::new(SharedString::from(format!("toggle-{id}")))
                .label(if active { "disable" } else { "enable" })
                .small()
                .on_click(move |_, window, cx| on_toggle(window, cx)),
        )
}

/// A catalogued tool that is not on this machine: same row anatomy as
/// [`activation_row`] — dot, name, note, button — so the list reads as one
/// list, with the dot dimmed to "absent" and the action being the one that
/// helps, installing it.
fn install_row(
    id: &str,
    theme: &Theme,
    on_install: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    h_flex()
        .id(SharedString::from(format!("install-row-{id}")))
        .justify_between()
        .items_center()
        .px_2()
        .py_1()
        .rounded_md()
        .hover(|style| style.bg(theme.list_hover))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_color(theme.muted_foreground).child("○"))
                .child(
                    Label::new(id.to_owned())
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground),
                )
                .child(
                    Label::new("not installed")
                        .text_color(theme.muted_foreground)
                        .text_size(theme.font_size * 0.8),
                ),
        )
        .child(
            Button::new(SharedString::from(format!("install-{id}")))
                .icon(IconName::Download)
                .label("install")
                .small()
                .tooltip("Run `reactor install` in the console")
                .on_click(move |_, window, cx| on_install(window, cx)),
        )
}

/// Resolve an ANSI colour index against the theme.
///
/// The palette is already there and already curated: `base.red`, `.green`,
/// `.yellow`, `.blue`, `.magenta`, `.cyan` are the six chromatic terminal
/// colours in Ayu's own hues, so a command's output lands in the window's
/// palette rather than importing xterm's. Black and white bend to the
/// theme's own foreground/muted so neither disappears into the background,
/// and the bright half reuses the same hues — this is a log, not a terminal
/// that owes anyone sixteen distinguishable colours.
fn ansi_color(index: Option<u8>, theme: &Theme) -> Option<gpui_kit::Hsla> {
    let index = index?;
    Some(match index % 8 {
        0 => theme.muted_foreground,
        1 => theme.red,
        2 => theme.green,
        3 => theme.yellow,
        4 => theme.blue,
        5 => theme.magenta,
        6 => theme.cyan,
        _ => theme.foreground,
    })
}

/// The docked chrome every panel's [`Panel::title`] draws: an icon plus a
/// name. The one place a panel gets its name, so a new panel can never ship
/// gpui-kit's default `t!("Dock.Unnamed")` tab by accident (the bug this
/// fixes — see the redesign notes at the top of this module).
fn panel_title_row(icon: IconName, label: &str) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::new(icon).small())
        .child(Label::new(label.to_owned()))
}

/// A panel body's loading placeholder — shown while a `reactor` CLI round
/// trip is in flight and no data has arrived yet (gui/SPEC.md §5). Every
/// side panel that fetches through [`reactor_client::ReactorClient`] uses this,
/// so "nothing happened yet" and "still loading" never look the same.
fn loading_row(label: &str, theme: &Theme) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .px_2()
        .py_1()
        .child(Spinner::new().small().color(theme.muted_foreground))
        .child(Label::new(label.to_owned()).text_color(theme.muted_foreground))
}

// ---------------------------------------------------------------------------
// TranscriptPanel — center: transcript + composer (gui/SPEC.md §6)
// ---------------------------------------------------------------------------

/// The center panel: the transcript (scrollable) with the composer docked to
/// its bottom, and the extension text widgets above the composer —
/// `aboveEditor` parity with pi's TUI (gui/SPEC.md §6).
pub struct TranscriptPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    pub composer: Entity<TextareaState>,
    focus: FocusHandle,
    /// The virtualized list's scroll/follow-tail state (fixes the "cannot
    /// scroll, no auto-scroll" bug: a bare `overflow_y_scroll` div has no
    /// notion of "stay pinned to the bottom as new content streams in",
    /// which is exactly what a transcript needs). See
    /// [`gpui_kit::component::message_scroller`].
    scroller: Entity<MessageScrollerState>,
    /// How many items the scroller was last told about, so a render can
    /// tell an append from an in-place mutation (a streaming delta changes
    /// the last item's content without changing the count) and remeasure
    /// instead of appending.
    last_item_count: usize,
    /// Which "thinking" blocks (by transcript index) the user has expanded.
    /// Presentation state, not session data — it lives here rather than on
    /// `ChatItem` (gui/SPEC.md §4.4) — and indices are stable to key it by:
    /// the transcript only ever appends. Fixes "thinking is not
    /// collapsible": the trigger existed but had no click handler and always
    /// passed `open(false)`, so it never actually did anything.
    expanded_thinking: std::collections::HashSet<usize>,
    weak_self: gpui_kit::WeakEntity<Self>,
}

impl TranscriptPanel {
    /// `composer` is passed in, not read back from `app`: the panels are
    /// built inside `ReactorApp::new`, and reading an entity while it is
    /// being constructed is gpui's re-entrancy panic. Anything else the
    /// panel needs is read at render time, after construction (§4.4).
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        composer: Entity<TextareaState>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            composer,
            focus: cx.focus_handle(),
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            last_item_count: 0,
            expanded_thinking: std::collections::HashSet::new(),
            weak_self: cx.weak_entity(),
        }
    }
}

/// Render one transcript item, at the width the panel was given.
fn render_item(
    index: usize,
    item: &ChatItem,
    theme: &gpui_kit::component::Theme,
    thinking_expanded: bool,
    toggle_thinking: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::Div {
    match item {
        ChatItem::User { text } => v_flex()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.secondary)
                    .text_color(theme.secondary_foreground)
                    .child(Label::new("you")),
            )
            .child(div().px_2().child(text.clone()))
            .into(),
        ChatItem::AssistantText { text, streaming } => {
            let streaming = *streaming;
            div()
                .px_2()
                .child(
                    TextView::markdown(format!("assistant-{index}"), text.clone())
                        .style(crate::theme::text_view_style(theme)),
                )
                .when(streaming, |el| el.opacity(0.85))
                .into()
        }
        ChatItem::Thinking { text, streaming } => {
            let chevron = if thinking_expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            v_flex()
                .px_2()
                .child(
                    gpui_kit::component::collapsible::Collapsible::new()
                        .open(thinking_expanded)
                        .child(
                            h_flex()
                                .id(("thinking-trigger", index))
                                .gap_1()
                                .items_center()
                                .cursor_pointer()
                                .on_click(move |_, window, cx| toggle_thinking(index, window, cx))
                                .child(
                                    Icon::new(chevron)
                                        .small()
                                        .text_color(theme.muted_foreground),
                                )
                                .child(
                                    Label::new(if *streaming {
                                        "thinking…"
                                    } else {
                                        "thinking"
                                    })
                                    .text_color(theme.muted_foreground),
                                ),
                        )
                        .content(
                            div().child(
                                TextView::markdown(format!("thinking-{index}"), text.clone())
                                    .style(crate::theme::text_view_style(theme)),
                            ),
                        ),
                )
                .into()
        }
        ChatItem::ToolCall {
            name,
            args,
            output,
            is_error,
            done,
            ..
        } => {
            let colour = if *is_error {
                theme.danger
            } else {
                *theme.tokens.tab_bar
            };
            let output = if output.len() > crate::app::TOOL_OUTPUT_MAX_CHARS {
                format!(
                    "{}\n… truncated",
                    &output[..crate::app::TOOL_OUTPUT_MAX_CHARS]
                )
            } else {
                output.clone()
            };
            v_flex()
                .px_2()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(Label::new(format!("{name}")).text_color(if *is_error {
                            theme.danger
                        } else {
                            theme.accent
                        }))
                        .child(
                            Label::new(if *done { "done" } else { "running…" })
                                .text_color(theme.muted_foreground),
                        ),
                )
                .child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .text_size(theme.mono_font_size * 0.8)
                        .child(short_args(args)),
                )
                .when(!output.is_empty(), |el| {
                    el.child(
                        div()
                            .px_2()
                            .text_color(if *is_error {
                                theme.danger
                            } else {
                                theme.foreground
                            })
                            .text_size(theme.mono_font_size * 0.8)
                            .child(output),
                    )
                })
                .when(*is_error, |el| el.border_l_2().border_color(colour))
                .into()
        }
        ChatItem::CustomEntry { custom_type, raw } => {
            let title = raw
                .get("details")
                .and_then(|d| d.get("title"))
                .and_then(Value::as_str)
                .unwrap_or(custom_type);
            let body = raw
                .get("details")
                .and_then(|d| d.get("body"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            v_flex()
                .px_2()
                .gap_1()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.tokens.title_bar)
                        .child(Label::new(title).text_color(theme.accent)),
                )
                .when_some(body, |el, body| {
                    el.child(
                        div().px_2().child(
                            TextView::markdown(format!("custom-{index}"), body)
                                .style(crate::theme::text_view_style(theme)),
                        ),
                    )
                })
                .into()
        }
        ChatItem::Error { message } => v_flex()
            .px_2()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.danger.opacity(0.2))
                    .text_color(theme.danger)
                    .child(message.clone()),
            )
            .into(),
        ChatItem::Unknown { raw } => div()
            .px_2()
            .text_size(theme.mono_font_size * 0.8)
            .text_color(theme.muted_foreground)
            .child(format!("debug: {}", short_json(raw)))
            .into(),
    }
}

fn short_args(args: &Value) -> String {
    let rendered = match args {
        Value::String(s) => s.clone(),
        Value::Object(o) => {
            if let Some(command) = o.get("command").and_then(Value::as_str) {
                command.to_owned()
            } else {
                serde_json::to_string(args).unwrap_or_default()
            }
        }
        _ => String::new(),
    };
    let mut rendered = rendered.replace('\n', " ");
    if rendered.len() > 120 {
        rendered.truncate(120);
        rendered.push('…');
    }
    rendered
}

fn short_json(raw: &Value) -> String {
    let rendered = serde_json::to_string_pretty(raw).unwrap_or_default();
    if rendered.len() > 2000 {
        format!("{}\n…", &rendered[..2000])
    } else {
        rendered
    }
}

/// The transcript, newest last, in a scrollable column. The composer sits
/// under it, and the extension text widgets (`setWidget` without the
/// `reactor:` prefix) render above it — `aboveEditor` parity with the TUI
/// (gui/SPEC.md §6).
impl Focusable for TranscriptPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for TranscriptPanel {}

impl gpui_kit::base::dock::Panel for TranscriptPanel {
    fn panel_name(&self) -> &'static str {
        "transcript"
    }
}

impl Panel for TranscriptPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::MessageSquare, "Transcript")
    }
}

impl Render for TranscriptPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (items, widgets, notifications, phase, queue_state, last_error) =
            match self.app.upgrade() {
                Some(app) => {
                    let session = app.read(cx).session.read(cx);
                    (
                        session.items.clone(),
                        session.widgets.clone(),
                        app.read(cx).notifications.clone(),
                        session.phase,
                        (!session.steering.is_empty(), !session.follow_up.is_empty()),
                        session.last_error.clone(),
                    )
                }
                None => Default::default(),
            };

        // Tell the scroller about new content before rendering it: an append
        // keeps tail-following, a shrink (a branch switch, a fresh session)
        // resets the anchor, and an unchanged count still remeasures — a
        // streaming delta grows the last row's height without changing how
        // many rows there are, and a stale cached height is exactly the
        // "scrolling feels broken" bug this fixes.
        let new_len = items.len();
        if new_len > self.last_item_count {
            let grew_by = new_len - self.last_item_count;
            self.scroller.update(cx, |state, cx| {
                state.append(grew_by, cx);
            });
        } else if new_len < self.last_item_count {
            self.scroller
                .update(cx, |state, cx| state.reset(new_len, cx));
        } else {
            self.scroller.update(cx, |state, cx| state.remeasure(cx));
        }
        self.last_item_count = new_len;

        let transcript = if items.is_empty() {
            v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .child(
                    Label::new("send a prompt to start the conversation")
                        .text_color(theme.muted_foreground),
                )
                .into_any_element()
        } else {
            let render_theme = theme.clone();
            let expanded_thinking = self.expanded_thinking.clone();
            let weak_self = self.weak_self.clone();
            let toggle_thinking = move |index: usize, _window: &mut Window, cx: &mut App| {
                if let Some(panel) = weak_self.upgrade() {
                    panel.update(cx, |panel, cx| {
                        if !panel.expanded_thinking.remove(&index) {
                            panel.expanded_thinking.insert(index);
                        }
                        cx.notify();
                    });
                }
            };
            MessageScroller::new(
                "transcript",
                self.scroller.clone(),
                move |index, _window, _cx| {
                    render_item(
                        index,
                        &items[index],
                        &render_theme,
                        expanded_thinking.contains(&index),
                        toggle_thinking.clone(),
                    )
                },
            )
            .with_bottom_fade(theme.background)
            .into_any_element()
        };

        // The composer: Enter submits (queued as a follow-up while the agent
        // works) — the subscription lives on ReactorApp, the view here.
        let phase_label = match phase {
            AgentPhase::Idle => "idle",
            AgentPhase::Working => "working — Enter queues a follow-up",
            AgentPhase::Retrying => "retrying — Enter queues a follow-up",
            AgentPhase::Compacting => "compacting…",
        };
        let queue_label = match queue_state {
            (false, false) => None,
            (steering, follow_up) => Some(format!(
                "queued:{} steering:{} follow-up:{}",
                if queue_state.0 || queue_state.1 {
                    "•"
                } else {
                    ""
                },
                if steering { "●" } else { "" },
                if follow_up { "●" } else { "" }
            )),
        };

        let composer = v_flex()
            .border_t_1()
            .border_color(cx.theme().border)
            .p_2()
            .gap_2()
            .children(widgets.iter().map(|(key, lines)| {
                div()
                    .px_2()
                    .text_color(cx.theme().muted_foreground)
                    .text_size(cx.theme().font_size * 0.85)
                    .child(Label::new(key.clone()).text_size(cx.theme().font_size * 0.75))
                    .child(lines.join("\n"))
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Textarea::new(&self.composer).flex_1())
                    .child(
                        Button::new("send")
                            .primary()
                            .label(if phase == AgentPhase::Idle {
                                "Send"
                            } else {
                                "Follow up"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(app) = this.app.upgrade() {
                                    app.update(cx, |app, cx| app.send_composer(window, cx));
                                }
                            })),
                    )
                    .child(
                        Button::new("interrupt")
                            .danger()
                            .disabled(phase == AgentPhase::Idle)
                            .label("Interrupt")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(app) = this.app.upgrade() {
                                    app.update(cx, |app, cx| app.interrupt(window, cx));
                                }
                            })),
                    )
                    .when(phase != AgentPhase::Idle, |el| {
                        el.child(Spinner::new().small().color(cx.theme().accent))
                    })
                    .child(Label::new(phase_label).text_color(cx.theme().muted_foreground))
                    .when_some(queue_label, |el, label| {
                        el.child(Label::new(label).text_color(cx.theme().accent))
                    }),
            );

        let mut panel = v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().child(transcript))
            .child(composer);

        // Transient extension toasts above the status bar (§4.5's `notify`).
        if !notifications.is_empty() {
            let mut toasts = v_flex().absolute().bottom_12().right_4().gap_1();
            for (kind, message) in notifications {
                let colour = match kind.as_str() {
                    "error" => cx.theme().danger,
                    "warning" => cx.theme().warning,
                    _ => cx.theme().accent,
                };
                toasts = toasts.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(cx.theme().popover)
                        .text_color(colour)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(message),
                );
            }
            panel = panel.child(div().absolute().inset_0().overflow_hidden().child(toasts));
        }

        if let Some(error) = last_error {
            panel = panel.child(
                div()
                    .px_2()
                    .text_color(cx.theme().danger)
                    .text_size(cx.theme().font_size * 0.85)
                    .child(error),
            );
        }

        panel.relative()
    }
}

// ---------------------------------------------------------------------------
// TreePanel — left: the session tree of pi's `/tree` (gui/SPEC.md §4.6)
// ---------------------------------------------------------------------------

/// The left panel: the session tree of pi's `/tree` (gui/SPEC.md §4.6) —
/// flatten `get_tree` depth-first, click selects, the button switches the
/// branch through the tree bridge (disabled while streaming).
pub struct TreePanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    /// The node the user clicked — the switch-branch affordance acts on it.
    selection: Option<SharedString>,
    focus: FocusHandle,
}

/// One flattened row of the session tree: depth, id, and a readable label.
#[derive(Debug, Clone, PartialEq)]
struct TreeNode {
    depth: usize,
    id: SharedString,
    label: String,
}

impl TreePanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
            selection: None,
        }
    }
}

/// Flatten `get_tree`'s nodes depth-first, labelling each entry the way the
/// transcript would label it (§4.6): user text, assistant text, custom types.
fn flatten_tree(roots: &[Value], out: &mut Vec<TreeNode>, depth: usize) {
    for node in roots {
        let Some(entry) = node.get("entry") else {
            continue;
        };
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let message = entry.get("message");
        let role = message.and_then(|m| m.get("role")).and_then(Value::as_str);
        let label = match role {
            Some("user") => {
                "user: ".to_owned()
                    + &content_text(message.unwrap().get("content").unwrap_or(&Value::Null))
                        .chars()
                        .take(60)
                        .collect::<String>()
            }
            Some("assistant") => "assistant".to_owned(),
            Some("toolResult") => "tool result".to_owned(),
            Some("custom") => format!(
                "custom: {}",
                message
                    .and_then(|m| m.get("customType"))
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            ),
            // A non-message entry (model/thinking-level changes, compaction
            // records, …): its `type` field, humanized — snake_case verbatim
            // (with the quote marks `{:?}` adds) read as raw debug output,
            // not a tree label.
            _ => entry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("entry")
                .replace('_', " "),
        };
        out.push(TreeNode {
            depth,
            id: SharedString::from(id.to_owned()),
            label,
        });
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            flatten_tree(children, out, depth + 1);
        }
    }
}

impl Render for TreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tree = self.app.upgrade().and_then(|app| app.read(cx).tree.clone());
        let leaf = self
            .app
            .upgrade()
            .and_then(|app| app.read(cx).leaf_id.clone());
        let streaming = self
            .app
            .upgrade()
            .map(|app| app.read(cx).session.read(cx).phase != AgentPhase::Idle)
            .unwrap_or(true);

        let mut nodes = Vec::new();
        if let Some(data) = &tree {
            if let Some(roots) = data.get("tree").and_then(Value::as_array) {
                flatten_tree(roots, &mut nodes, 0);
            }
        }

        let mut list = v_flex().p_2().gap_1();
        for node in &nodes {
            let is_leaf = leaf.as_deref() == Some(node.id.as_ref());
            list = list.child(
                div()
                    .id(node.id.clone())
                    .pl(px(8. * node.depth as f32))
                    .flex()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .when(is_leaf, |el| el.bg(theme.tokens.list_active))
                    .when(!is_leaf, |el| el.hover(|style| style.bg(theme.list_hover)))
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .text_color(if is_leaf {
                                theme.accent
                            } else {
                                theme.foreground
                            })
                            .text_size(theme.font_size * 0.85)
                            .child(node.label.clone()),
                    ),
            );
        }
        let loading = self
            .app
            .upgrade()
            .map(|app| app.read(cx).tree_loading)
            .unwrap_or(false);
        if nodes.is_empty() {
            list = list.child(if loading {
                loading_row("loading session tree…", &theme).into_any_element()
            } else {
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child("no session tree yet — send a prompt")
                    .into_any_element()
            });
        }

        // The branch-switch affordance: disabled while streaming (§4.6's
        // verified rejection), the tree bridge does the navigation.
        let selected = self.selection.clone();
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex().justify_end().px_2().child(
                    Button::new("switch-branch")
                        .label("Switch branch here")
                        .small()
                        .disabled(selected.is_none() || streaming)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(id) = this.selection.clone() {
                                if let Some(app) = this.app.upgrade() {
                                    app.update(cx, |app, cx| app.switch_branch(&id, cx));
                                }
                            }
                            let _ = window;
                        })),
                ),
            )
            .child(
                div()
                    .id("scroll-panel")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
    }
}
impl Focusable for TreePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for TreePanel {}

impl gpui_kit::base::dock::Panel for TreePanel {
    fn panel_name(&self) -> &'static str {
        "tree"
    }

    /// The tree is a glance panel: it has no close affordance in v0.1 — the
    /// layout is collapsible (gui/SPEC.md §6).
    fn closable(&self, _cx: &App) -> bool {
        false
    }
}

impl Panel for TreePanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::GitBranch, "Session Tree")
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let loading = self
            .app
            .upgrade()
            .map(|a| a.read(cx).tree_loading)
            .unwrap_or(false);
        let app = self.app.clone();
        Some(vec![refresh_button(
            "refresh-tree",
            loading,
            move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| app.refresh_tree(cx));
                }
            },
        )])
    }
}

// ---------------------------------------------------------------------------
// ToolsPanel / ToolsetsPanel — right, top tab group: the catalogue and its
// toolsets, from the `reactor` CLI (gui/SPEC.md §5). Two tabs rather than
// one merged scroll list — the redesign's fix for keeping tools and
// toolsets apart — using the dock's own tab-group primitive
// (`DockLayout::tabs().panel_view(a).panel_view(b)`, wired in `app.rs`)
// instead of a bespoke in-panel tab widget. Enable/disable runs the CLI and
// refreshes both panels, since one `reactor` round trip answers both
// (gui/SPEC.md §5, ADR-0014).
// ---------------------------------------------------------------------------

/// The toolbar's refresh affordance, shared by every side panel that fetches
/// through the CLI — one look for "fetch this again", disabled while a fetch
/// is already in flight so a second click cannot race the first.
fn refresh_button(
    id: &'static str,
    loading: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .icon(IconName::RotateCw)
        .ghost()
        .small()
        .disabled(loading)
        .tooltip("Refresh from the reactor CLI")
        .on_click(move |_, window, cx| on_click(window, cx))
}

pub struct ToolsPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
    /// Whether tools that are not installed on this machine are listed.
    ///
    /// Off by default: the catalogue is a menu of what REactor *knows*,
    /// and on a fresh machine most of it is not installed (44 of 55 here),
    /// which buried the handful of tools the user actually has behind rows
    /// they cannot act on. Turning it on turns the panel into the install
    /// menu it also needs to be.
    show_uninstalled: bool,
}

impl ToolsPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
            show_uninstalled: false,
        }
    }
}

impl Focusable for ToolsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for ToolsPanel {}

impl gpui_kit::base::dock::Panel for ToolsPanel {
    fn panel_name(&self) -> &'static str {
        "tools"
    }
}

impl Panel for ToolsPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::Wrench, "Tools")
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let loading = self
            .app
            .upgrade()
            .map(|a| a.read(cx).catalogue_loading)
            .unwrap_or(false);
        let app = self.app.clone();
        let showing = self.show_uninstalled;
        Some(vec![
            Button::new("toggle-uninstalled")
                .icon(if showing {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                })
                .ghost()
                .small()
                .tooltip(if showing {
                    "Hide tools that are not installed"
                } else {
                    "Show tools that are not installed, to install them"
                })
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.show_uninstalled = !this.show_uninstalled;
                    cx.notify();
                })),
            refresh_button("refresh-tools", loading, move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| app.refresh_catalogue(cx));
                }
            }),
        ])
    }
}

impl Render for ToolsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (tools, loading) = match self.app.upgrade() {
            Some(app) => {
                let read = app.read(cx);
                (read.catalogue.clone(), read.catalogue_loading)
            }
            None => (None, false),
        };

        let mut list = v_flex()
            .id("tools-list")
            .p_2()
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(payload) = &tools {
            let mut hidden = 0usize;
            for tool in &payload.tools {
                let installed = tool.status == "present";
                if !installed && !self.show_uninstalled {
                    hidden += 1;
                    continue;
                }
                let tool_id = tool.id.clone();
                let tool_active = tool.active;
                let weak_app = self.app.clone();
                if installed {
                    list = list.child(activation_row(
                        format!("tool-{tool_id}"),
                        tool_active,
                        tool.id.clone(),
                        None,
                        &theme,
                        move |_window, cx| {
                            if let Some(app) = weak_app.upgrade() {
                                // The target state is the opposite of the
                                // current one — passing `tool_active` itself
                                // re-applies the state the tool is already
                                // in, a no-op that looked like "toggling
                                // does nothing" (the bug this fixed).
                                app.update(cx, |app, cx| {
                                    app.toggle_tool(&tool_id, !tool_active, cx)
                                });
                            }
                        },
                    ));
                } else {
                    // Not installed: activating it would be a hint about a
                    // tool that is not there, so the row offers the thing
                    // that would actually help — opening a fresh terminal
                    // already running the install, where its questions can
                    // be answered.
                    let install_id = tool_id.clone();
                    list = list.child(install_row(&tool_id, &theme, move |window, cx| {
                        if let Some(app) = weak_app.upgrade() {
                            let id = install_id.clone();
                            app.update(cx, |app, cx| {
                                app.open_console(
                                    Some(("reactor".to_owned(), vec!["install".to_owned(), id])),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }));
                }
            }
            if payload.tools.is_empty() {
                list = list.child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child("no tools catalogued"),
                );
            } else if hidden > 0 {
                list = list.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_color(theme.muted_foreground)
                        .text_size(theme.font_size * 0.8)
                        .child(format!("{hidden} not installed — show them to install",)),
                );
            }
        } else if loading {
            list = list.child(loading_row("loading tools…", &theme));
        } else {
            list = list.child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child("no catalogue data — try `reactor doctor`"),
            );
        }

        v_flex().size_full().child(list)
    }
}

pub struct ToolsetsPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
}

impl ToolsetsPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for ToolsetsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for ToolsetsPanel {}

impl gpui_kit::base::dock::Panel for ToolsetsPanel {
    fn panel_name(&self) -> &'static str {
        "toolsets"
    }
}

impl Panel for ToolsetsPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::Layers, "Toolsets")
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let loading = self
            .app
            .upgrade()
            .map(|a| a.read(cx).catalogue_loading)
            .unwrap_or(false);
        let app = self.app.clone();
        Some(vec![refresh_button(
            "refresh-toolsets",
            loading,
            move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| app.refresh_catalogue(cx));
                }
            },
        )])
    }
}

impl Render for ToolsetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (toolsets, loading) = match self.app.upgrade() {
            Some(app) => {
                let read = app.read(cx);
                (read.toolsets.clone(), read.catalogue_loading)
            }
            None => (None, false),
        };

        let mut list = v_flex()
            .id("toolsets-list")
            .p_2()
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(payload) = &toolsets {
            for row in &payload.toolsets {
                let toolset_id = row.id.clone();
                let toolset_active = row.active;
                let weak_app = self.app.clone();
                let detail = Some(SharedString::from(format!(
                    "{} tool{}",
                    row.tools.len(),
                    if row.tools.len() == 1 { "" } else { "s" }
                )));
                list = list.child(activation_row(
                    format!("toolset-{toolset_id}"),
                    toolset_active,
                    row.id.clone(),
                    detail,
                    &theme,
                    move |_window, cx| {
                        if let Some(app) = weak_app.upgrade() {
                            app.update(cx, |app, cx| {
                                app.toggle_toolset(&toolset_id, !toolset_active, cx);
                            });
                        }
                    },
                ));
            }
            if payload.toolsets.is_empty() {
                list = list.child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child("no toolsets catalogued"),
                );
            }
        } else if loading {
            list = list.child(loading_row("loading toolsets…", &theme));
        } else {
            list = list.child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child("no catalogue data — try `reactor doctor`"),
            );
        }

        v_flex().size_full().child(list)
    }
}

// ---------------------------------------------------------------------------
// ServicesPanel — right, bottom: live service state, from the CLI (§5).
// ---------------------------------------------------------------------------

pub struct ServicesPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
}

impl ServicesPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for ServicesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for ServicesPanel {}

impl gpui_kit::base::dock::Panel for ServicesPanel {
    fn panel_name(&self) -> &'static str {
        "services"
    }
}

impl Panel for ServicesPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::Server, "Services")
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let loading = self
            .app
            .upgrade()
            .map(|a| a.read(cx).services_loading)
            .unwrap_or(false);
        let app = self.app.clone();
        // No `--refresh`: the CLI's own TTLs decide freshness (gui/SPEC.md
        // §5, ADR-0014) — the button re-asks, it does not force a probe.
        Some(vec![refresh_button(
            "refresh-services",
            loading,
            move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| app.refresh_services(false, cx));
                }
            },
        )])
    }
}

impl Render for ServicesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (services, loading) = match self.app.upgrade() {
            Some(app) => {
                let read = app.read(cx);
                (read.services.clone(), read.services_loading)
            }
            None => (None, false),
        };

        let mut list = v_flex()
            .id("services-list")
            .p_2()
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(payload) = &services {
            // Down first, then unknown, then up — the footer's ladder (the
            // same rank `extensions/status/` renders in the TUI).
            let mut ordered: Vec<_> = payload.services.iter().collect();
            ordered.sort_by_key(|s| match s.state.as_str() {
                "down" => 0,
                "unknown" => 1,
                _ => 2,
            });
            for service in ordered {
                if service.status != "present" {
                    // A tool that is not installed has no service to show —
                    // reporting it would be a status for a thing that does
                    // not exist (gui/SPEC.md §6, the TUI panel's same rule).
                    continue;
                }
                let (glyph, colour) = match service.state.as_str() {
                    "up" => ("●", theme.success),
                    "down" => ("✗", theme.danger),
                    _ => ("?", theme.muted_foreground),
                };
                list = list.child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_color(colour)
                                .text_size(theme.font_size * 0.9)
                                .child(glyph),
                        )
                        .child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.font_size * 0.9)
                                .child(service.id.clone()),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .text_size(theme.font_size * 0.8)
                                .child(
                                    service
                                        .detail
                                        .clone()
                                        .unwrap_or_else(|| service.state.clone()),
                                ),
                        ),
                );
            }
            if payload.services.is_empty() {
                list = list.child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child("no catalogued tool declares a service probe"),
                );
            }
        } else if loading {
            list = list.child(loading_row("loading services…", &theme));
        } else {
            list = list.child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child("no service data — try `reactor doctor`"),
            );
        }

        v_flex().size_full().child(list)
    }
}

// ---------------------------------------------------------------------------
// ExtensionViewsPanel — right, top tab group: side-docked extension views
// (the `placement: "side"` half of the envelope contract, gui/SPEC.md §4.2,
// ADR-0032). Overlay-placed views (today's selector, guide) float over the
// transcript instead — see `ReactorApp::render_overlay`. Both call the same
// `views::render_content`, so a future view primitive (a timeline, a graph,
// a code view — see `crate::views`'s module doc) renders identically in
// either spot; this panel is the fix for the "side views render nothing"
// bug (`app.views` was parsed but never drawn) and the extensibility point
// the redesign plans around.
// ---------------------------------------------------------------------------

pub struct ExtensionViewsPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
}

impl ExtensionViewsPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for ExtensionViewsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for ExtensionViewsPanel {}

impl gpui_kit::base::dock::Panel for ExtensionViewsPanel {
    fn panel_name(&self) -> &'static str {
        "extension-views"
    }
}

impl Panel for ExtensionViewsPanel {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(IconName::LayoutDashboard, "Views")
    }
}

impl Render for ExtensionViewsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let side_views: Vec<crate::contract::View> = self
            .app
            .upgrade()
            .map(|app| {
                app.read(cx)
                    .views
                    .iter()
                    .filter(|v| v.placement == ViewPlacement::Side)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let mut body = v_flex()
            .id("extension-views-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap_3()
            .p_2();
        if side_views.is_empty() {
            body = body.child(div().px_2().text_color(theme.muted_foreground).child(
                "no side view active — an extension docks one here with a \
                     `side`-placement envelope (gui/SPEC.md §4.2)",
            ));
        }
        for view in side_views {
            let weak_app = self.app.clone();
            let dismiss_view_id = view.view_id.clone();
            let dispatch_view = view.clone();
            body = body.child(
                v_flex()
                    .gap_1()
                    .pb_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(views::view_chrome(&view, {
                        let weak_app = weak_app.clone();
                        move |_window, cx| {
                            if let Some(app) = weak_app.upgrade() {
                                app.update(cx, |app, cx| app.dismiss_view(&dismiss_view_id, cx));
                            }
                        }
                    }))
                    .child(views::render_content(
                        &view.content,
                        &theme,
                        move |action, row, _window, cx| {
                            if let Some(app) = weak_app.upgrade() {
                                let (command, payload) = dispatch_view.dispatch(action, row);
                                app.update(cx, |app, cx| app.run_ui_event(command, payload, cx));
                            }
                        },
                    ))
                    .when_some(view.footer.clone(), |el, footer| {
                        el.child(views::view_footer(&footer, &theme))
                    }),
            );
        }

        v_flex().size_full().child(body)
    }
}

// ---------------------------------------------------------------------------
// ConsolePanel — bottom: run commands, watch them, answer them.
//
// A general command runner, not an install log: anything typed here runs
// through the user's shell, and `reactor install <id>` from the Tools panel
// opens its own console already running it. Each instance owns its own pty
// session, output buffer and polling loop — nothing here reaches back into
// `ReactorApp` except to open another console (`ReactorApp::open_console`)
// and to refresh the catalogue once a command finishes — which is what lets
// more than one of these exist side by side as tabs, each independent.
// ---------------------------------------------------------------------------

/// How often an idle-or-not console checks its session for new output.
/// Matches `PUMP_TICK` in spirit — a console has nothing to synchronize with
/// pi's own pump, so it keeps its own timer rather than borrowing one.
const CONSOLE_POLL_TICK: std::time::Duration = std::time::Duration::from_millis(50);

pub struct ConsolePanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    /// Given at construction rather than read from `app` on demand: a new
    /// console is often built from inside an `app.update` (opening one from
    /// the Tools panel, or the very first console at startup, built while
    /// `ReactorApp` itself is still under construction) — reading the same
    /// entity mid-update panics ("already being updated"), and the cwd never
    /// changes for the life of a window anyway.
    cwd: Option<std::path::PathBuf>,
    input: Entity<InputState>,
    scroller: Entity<MessageScrollerState>,
    last_line_count: usize,
    buffer: crate::console::ConsoleBuffer,
    session: Option<crate::console::ConsoleSession>,
    /// The command currently running, shown as this console's tab title so
    /// several of them are told apart at a glance instead of all reading
    /// "Console".
    command: Option<String>,
}

impl ConsolePanel {
    /// `initial`, when given, is run immediately rather than leaving the
    /// console at an idle prompt — how installing a catalogued tool opens a
    /// terminal already running `reactor install <id>`.
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        cwd: Option<std::path::PathBuf>,
        initial: Option<(String, Vec<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("$"));
        cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            },
        )
        .detach();

        let mut this = Self {
            app,
            cwd: cwd.clone(),
            input,
            // One row of "item count" more than lines held, always: the
            // prompt is the scroller's own last row (render's doc), and even
            // an empty console still shows one — the idle prompt.
            scroller: cx.new(|cx| MessageScrollerState::new(1, cx)),
            last_line_count: 0,
            buffer: crate::console::ConsoleBuffer::default(),
            session: None,
            command: None,
        };

        if let Some((program, args)) = initial {
            let shown = std::iter::once(program.clone())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            this.start(
                shown,
                crate::console::ConsoleSession::program(&program, &args, cwd),
                cx,
            );
        }

        // Self-polling: every console owns its own timer, draining its own
        // session on the same cadence `ReactorApp`'s RPC pump uses. Nothing
        // routes through the app entity for this — the reason a second
        // console's command cannot interleave into this one's buffer is
        // that there is no shared state left for it to interleave into.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CONSOLE_POLL_TICK).await;
                let alive = this.update(cx, |this, cx| this.poll(cx)).is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        this
    }

    /// Enter: run the line, or answer the command that is already running.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.input.read(cx).value().to_string();
        if line.trim().is_empty() {
            return;
        }
        // Cleared before dispatching, so a second Enter for the same
        // keystroke finds nothing to run (see `ReactorApp::send_composer`).
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        if let Some(session) = self.session.as_mut() {
            // Not echoed here: the pty's line discipline echoes what is
            // written to it, so the answer already appears where the
            // program asked for it. Echoing locally too would show it twice.
            if let Err(e) = session.write_line(&line) {
                self.buffer.push_line(format!("could not send: {e}"));
            }
        } else {
            let cwd = self.cwd.clone();
            self.start(
                line.clone(),
                crate::console::ConsoleSession::shell(&line, cwd),
                cx,
            );
        }
        cx.notify();
    }

    fn start(
        &mut self,
        shown: String,
        spawned: anyhow::Result<crate::console::ConsoleSession>,
        cx: &mut Context<Self>,
    ) {
        self.buffer.push_line(format!("$ {shown}"));
        match spawned {
            Ok(session) => {
                self.session = Some(session);
                self.command = Some(shown);
            }
            Err(e) => self.buffer.push_line(format!("could not run: {e}")),
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session.as_mut() {
            session.kill();
            self.buffer.push_line("^C");
        }
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer.clear();
        cx.notify();
    }

    /// Move whatever the running command has produced into the buffer, and
    /// notice when it finishes.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let before = self.buffer.lines().len();
        let finished = session.drain(&mut self.buffer);
        let grew = self.buffer.lines().len() != before;
        let Some(code) = finished else {
            if grew {
                cx.notify();
            }
            return;
        };
        self.session = None;
        self.command = None;
        match code {
            // Success stays silent, the way a shell does not announce that
            // a command worked — only that it did not (the "[done]" noise
            // this removed).
            Some(0) => {}
            Some(code) => self.buffer.push_line(format!("[exit {code}]")),
            None => self.buffer.push_line("[killed]".to_owned()),
        }
        // A command that just finished may well have changed what is
        // installed — an install certainly did, and a hand-typed `brew
        // install` counts too. Re-reading the catalogue is cheap and keeps
        // the Tools panel honest without the user knowing to refresh it.
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.refresh_catalogue(cx);
                app.refresh_services(false, cx);
            });
        }
        cx.notify();
    }
}

impl Focusable for ConsolePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<PanelEvent> for ConsolePanel {}

impl gpui_kit::base::dock::Panel for ConsolePanel {
    fn panel_name(&self) -> &'static str {
        "console"
    }
}

impl Panel for ConsolePanel {
    /// Shows the running command rather than a fixed "Console" — the tab
    /// label that tells several terminals apart from one another.
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_row(
            IconName::SquareTerminal,
            self.command.as_deref().unwrap_or("Console"),
        )
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let busy = self.session.is_some();
        let new_app = self.app.clone();
        Some(vec![
            Button::new("console-new")
                .icon(IconName::Plus)
                .ghost()
                .small()
                .tooltip("Open another terminal")
                .on_click(move |_, window, cx| {
                    if let Some(app) = new_app.upgrade() {
                        app.update(cx, |app, cx| app.open_console(None, window, cx));
                    }
                }),
            Button::new("console-stop")
                .icon(IconName::CircleStop)
                .ghost()
                .small()
                .disabled(!busy)
                .tooltip("Stop the running command")
                .on_click(cx.listener(|this, _, _window, cx| this.stop(cx))),
            Button::new("console-clear")
                .icon(IconName::Eraser)
                .ghost()
                .small()
                .tooltip("Clear this terminal")
                .on_click(cx.listener(|this, _, _window, cx| this.clear(cx))),
        ])
    }
}

impl Render for ConsolePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let lines = self.buffer.lines().to_vec();
        let busy = self.session.is_some();

        // Same tail-following bookkeeping the transcript uses: output should
        // stay pinned to the newest line unless the user has scrolled away.
        let new_len = lines.len();
        if new_len > self.last_line_count {
            let grew_by = new_len - self.last_line_count;
            self.scroller.update(cx, |state, cx| {
                state.append(grew_by, cx);
            });
        } else if new_len < self.last_line_count {
            self.scroller
                .update(cx, |state, cx| state.reset(new_len + 1, cx));
        } else {
            self.scroller.update(cx, |state, cx| state.remeasure(cx));
        }
        self.last_line_count = new_len;

        let row_theme = theme.clone();
        let input = self.input.clone();
        let row_count = lines.len();

        // The prompt is this scroller's own last row rather than a control
        // bolted on below it — it reads as the log's own next line and moves
        // with the scrollback exactly the way a real terminal's cursor does,
        // instead of sitting in a separate area the user has to look away
        // from the output to reach. It is still a real `InputState`
        // underneath (full cursor/editing/IME support), not a hand-rolled
        // text renderer: only where it sits changed.
        let output = MessageScroller::new(
            "console-output",
            self.scroller.clone(),
            move |at, _window, _cx| {
                if at < row_count {
                    // Every span is its own selectable run, ordered by line
                    // then position in it — the plain `div().child(string)`
                    // this replaced could not be selected or copied at all
                    // (`TextSelectionLayer`, which `Root` already renders,
                    // only picks up runs made of this). Each span gets its
                    // own participant (`SelectableText::new`, not a document
                    // shared via `with_handle`): a handle is one hitbox and
                    // one run slot, so several spans sharing one clobbered
                    // every span but whichever painted last each frame — the
                    // bug behind "selection works most of the time".
                    let mut row = h_flex()
                        .id(("console-line", at))
                        .font_family(row_theme.mono_font_family.clone())
                        .text_size(row_theme.mono_font_size * 0.85);
                    for (index, span) in lines[at].iter().enumerate() {
                        let order = (at as u64) * 1000 + index as u64;
                        let style = TextStyleRefinement {
                            color: Some(
                                ansi_color(span.color, &row_theme).unwrap_or(row_theme.foreground),
                            ),
                            font_weight: span.bold.then_some(gpui_kit::FontWeight::BOLD),
                            ..Default::default()
                        };
                        row = row.child(
                            SelectableText::new(("console-span", order), span.text.clone())
                                .document_order(order)
                                .text_style(style),
                        );
                    }
                    row.into_any_element()
                } else {
                    h_flex()
                        .id("console-prompt")
                        .gap_1()
                        .items_center()
                        .font_family(row_theme.mono_font_family.clone())
                        .text_size(row_theme.mono_font_size * 0.85)
                        .child(
                            Label::new("$")
                                .font_family(row_theme.mono_font_family.clone())
                                .text_size(row_theme.mono_font_size * 0.85)
                                .text_color(if busy {
                                    row_theme.accent
                                } else {
                                    row_theme.muted_foreground
                                }),
                        )
                        .child(
                            // `appearance(false)` is load-bearing, not
                            // cosmetic: `Input`'s default chrome is a
                            // bordered, backgrounded box with its own focus
                            // ring, which painted a visible search-box
                            // outline around the prompt — this row still
                            // looked like a control sitting in the
                            // scrollback rather than the scrollback's own
                            // text, even after moving it into the same list
                            // as the output. Stripped, plus the font, size
                            // and padding matched to a plain output row, it
                            // reads as one more line of terminal text with a
                            // cursor in it.
                            Input::new(&input)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .font_family(row_theme.mono_font_family.clone())
                                .text_size(row_theme.mono_font_size * 0.85)
                                .px_0()
                                .py_0()
                                .flex_1(),
                        )
                        .when(busy, |el| {
                            el.child(Spinner::new().small().color(row_theme.accent))
                        })
                        .into_any_element()
                }
            },
        )
        .with_row_style({
            // The scroller spaces rows like chat messages; console lines
            // are lines, and must sit directly under one another.
            let mut style = gpui_kit::StyleRefinement::default();
            style.padding.bottom = Some(px(0.).into());
            style
        })
        .flex_1()
        .min_h_0();

        v_flex().size_full().child(output)
    }
}
