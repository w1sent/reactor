//! The docked panels of SPEC.md §6.
//!
//! Every panel is a *view* over [`ReactorApp`]'s state — stateless
//! presentation (SPEC.md §4.4): it upgrades the weak handle, reads, and
//! renders. Nothing here owns facts; actions dispatch back through the app.
//! The weak handle reaches panels as `cx.weak_entity()` from
//! `ReactorApp::new`, where the dock is assembled.

use serde_json::Value;

use gpui_kit::assets::IconName;
use gpui_kit::base::{SelectableText, h_flex, v_flex};
use gpui_kit::component::resizable::{resizable_panel, v_resizable};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::component::marker::{Marker, MarkerContent, MarkerIcon};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Window, div, px,
};

use crate::app::ReactorApp;
use crate::backend::TreeRow;
use crate::session::{AgentPhase, ChatItem, Session};
use crate::settings::Slot;

/// The tag on an MCP tool or on a server's toolset: what it is and what it costs. A
/// catalogue tool costs one line of the system prompt; an MCP tool puts its whole definition
/// into every request while it is active, so the cost is on the row, where the choice to
/// activate it is made (ADR-0048). `every` says the tag is of a toolset: all its tools.
fn mcp_tag(
    id: &str,
    server: &str,
    tokens: u64,
    up: bool,
    every: bool,
    theme: &Theme,
) -> AnyElement {
    // The state is an icon, not a word: a plug with a spark when the server answers, an
    // unplugged one when it does not. The cost stays beside the first; the tip says the rest.
    let tint = if up {
        theme.info
    } else {
        theme.muted_foreground
    };
    let tip = if up {
        format!(
            "{} the MCP server `{server}`. While {} active, {} definition — about {tokens} tokens{} — is part of every request once it is loaded — at once, unless the server's tools together cost more than the limit in Settings › Tools, when the agent loads each on demand. A catalogue tool costs a line in the system prompt; this costs its whole schema, so activate only what the work needs.",
            if every { "Every tool of" } else { "A tool of" },
            if every { "they are" } else { "it is" },
            if every { "their" } else { "its" },
            if every { " together" } else { "" },
        )
    } else {
        format!(
            "{} the MCP server `{server}`, which could not be reached the last time it was tried. {} not advertised until the server is back; see Settings › MCP servers.",
            if every { "Every tool of" } else { "A tool of" },
            if every { "They are" } else { "It is" },
        )
    };
    crate::hints::tip(
        div()
            .id(SharedString::from(format!("mcp-badge-{id}")))
            .flex()
            .items_center()
            .gap_1()
            .text_size(theme.font_size * 0.75)
            .text_color(tint)
            .child(
                Icon::new(if up {
                    IconName::PlugZap
                } else {
                    IconName::Unplug
                })
                .small()
                .text_color(tint),
            )
            .when(up, |el| el.child(SharedString::from(format!("~{tokens} tok")))),
        SharedString::from(tip),
    )
    .into_any_element()
}

pub(crate) fn mcp_badge(tool: &reactor_client::ToolRow, theme: &Theme) -> Option<AnyElement> {
    let mcp = tool.mcp.as_ref()?;
    Some(mcp_tag(
        &tool.id,
        &mcp.server,
        mcp.tokens,
        tool.status == "present",
        false,
        theme,
    ))
}

/// The same tag on a toolset whose members are all MCP tools of one server (the one made for
/// each server, or a user's own made of nothing else): the cost is of all of them.
pub(crate) fn mcp_toolset_badge(
    toolset: &reactor_client::ToolsetRow,
    tools: Option<&reactor_client::ToolsPayload>,
    theme: &Theme,
) -> Option<AnyElement> {
    let members: Vec<&reactor_client::ToolRow> = toolset
        .tools
        .iter()
        .filter_map(|id| tools?.tools.iter().find(|t| &t.id == id))
        .collect();
    if members.is_empty() || members.len() != toolset.tools.len() {
        return None;
    }
    let servers: std::collections::BTreeSet<&str> = members
        .iter()
        .filter_map(|t| t.mcp.as_ref().map(|m| m.server.as_str()))
        .collect();
    if servers.len() != 1 || members.iter().any(|t| t.mcp.is_none()) {
        return None;
    }
    let tokens = members
        .iter()
        .filter_map(|t| t.mcp.as_ref())
        .map(|m| m.tokens)
        .sum();
    let up = members.iter().all(|t| t.status == "present");
    Some(mcp_tag(
        &toolset.id,
        servers.first().copied().unwrap_or_default(),
        tokens,
        up,
        true,
        theme,
    ))
}

/// A catalogued tool that is not on this machine: same row anatomy as
/// [`activation_row`] — dot, name, note, button — so the list reads as one
/// list, with the dot dimmed to "absent" and the action being the one that
/// helps, installing it.
pub(crate) fn install_row(
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
                        .text_size(theme.mono_font_size)
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
pub(crate) fn panel_title_row(icon: IconName, label: &str) -> gpui_kit::Div {
    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::new(icon).small())
        .child(Label::new(label.to_owned()))
}

/// A panel's title, which also closes the panel when the tab is middle-clicked. The dock draws
/// the tab and offers no hook for that, but it draws this element inside it.
pub(crate) fn panel_title_closing<F: Fn(&mut Window, &mut App) + 'static>(
    icon: IconName,
    label: &str,
    on_close: F,
) -> impl IntoElement + use<F> {
    panel_title_row(icon, label).on_mouse_up(MouseButton::Middle, move |_, window, cx| {
        on_close(window, cx)
    })
}

/// What closing `panel` does: take it out of the dock through the app.
pub(crate) fn closer<P: gpui_kit::base::dock::Panel>(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    panel: gpui_kit::Entity<P>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let app = app.clone();
    move |window, cx| {
        if let Some(app) = app.upgrade() {
            let panel = panel.clone();
            app.update(cx, |app, cx| app.close_panel(panel, window, cx));
        }
    }
}

/// A panel body's loading placeholder — shown while a `reactor` CLI round
/// trip is in flight and no data has arrived yet (SPEC.md §5). Every
/// side panel that fetches through [`reactor_client::ReactorClient`] uses this,
/// so "nothing happened yet" and "still loading" never look the same.
pub(crate) fn loading_row(label: &str, theme: &Theme) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .px_2()
        .py_1()
        .child(Spinner::new().small().color(theme.muted_foreground))
        .child(Label::new(label.to_owned()).text_color(theme.muted_foreground))
}

// ---------------------------------------------------------------------------
// TranscriptPanel — center: transcript + composer (SPEC.md §6)
// ---------------------------------------------------------------------------

/// The center panel: the transcript (scrollable) with the composer docked to
/// its bottom (SPEC.md §6).
pub struct TranscriptPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    /// The session this transcript shows, and the model of its rows. A window can have
    /// several of these: each tab is one session's.
    pub session_id: String,
    session: Entity<Session>,
    pub composer: Entity<TextareaState>,
    focus: FocusHandle,
    /// The virtualized list's scroll/follow-tail state (fixes the "cannot
    /// scroll, no auto-scroll" bug: a bare `overflow_y_scroll` div has no
    /// notion of "stay pinned to the bottom as new content streams in",
    /// which is exactly what a transcript needs). See
    /// [`gpui_kit::component::message_scroller`].
    scroller: Entity<MessageScrollerState>,
    /// A fingerprint of each row as the scroller last measured it
    /// ([`crate::transcript_ui::row_sign`]), so a render can tell an append from an
    /// in-place mutation (a streaming delta changes one row's content without changing the
    /// count) and measure just that row again, instead of every row.
    row_signs: Vec<u64>,
    /// What the rows' measured heights depend on besides their text (fonts, the output
    /// limit, whether thinking opens by itself): when it changes every row is measured again.
    layout_key: String,
    /// Which tool calls (by transcript index) are open. Closed is the default: a call is two
    /// lines until the person opens it.
    expanded_tools: std::collections::HashSet<usize>,
    /// Which "thinking" blocks (by transcript index) the user has expanded.
    /// Presentation state, not session data — it lives here rather than on
    /// `ChatItem` (SPEC.md §4.4) — and indices are stable to key it by:
    /// the transcript only ever appends. Fixes "thinking is not
    /// collapsible": the trigger existed but had no click handler and always
    /// passed `open(false)`, so it never actually did anything.
    expanded_thinking: std::collections::HashSet<usize>,
    /// The message whose copy button was just pressed (it shows a tick for a moment).
    copied: Option<usize>,
    /// The usage bar's details and the goal's are open.
    details_open: bool,
    goal_open: bool,
    /// The scenario step whose card is open.
    step_selected: Option<usize>,
    /// *Scenario* was chosen from Activate: its chip shows the scenarios to start.
    choosing_scenario: bool,
    weak_self: gpui_kit::WeakEntity<Self>,
}

impl TranscriptPanel {
    /// `composer` is passed in, not read back from `app`: the panels are
    /// built inside `ReactorApp::new`, and reading an entity while it is
    /// being constructed is gpui's re-entrancy panic. Anything else the
    /// panel needs is read at render time, after construction (§4.4).
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        session_id: String,
        session: Entity<Session>,
        composer: Entity<TextareaState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        // Focus anywhere in the tab (its prompt among it) makes its session the active one.
        cx.on_focus_in(&focus, window, |this, window, cx| {
            this.make_active(window, cx)
        })
        .detach();
        Self {
            app,
            session_id,
            session,
            composer,
            focus,
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            row_signs: Vec::new(),
            layout_key: String::new(),
            expanded_tools: std::collections::HashSet::new(),
            expanded_thinking: std::collections::HashSet::new(),
            copied: None,
            details_open: false,
            goal_open: false,
            step_selected: None,
            choosing_scenario: false,
            weak_self: cx.weak_entity(),
        }
    }
}

impl TranscriptPanel {
    /// Make this tab's session the window's active one. Deferred: this runs from focus and
    /// pointer handling, while the app may be in the middle of an update.
    fn make_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (app, id) = (self.app.clone(), self.session_id.clone());
        window.defer(cx, move |window, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| app.activate_session(&id, window, cx));
            }
        });
    }
}

/// Render one transcript item, at the width the panel was given.
/// How tool cards are drawn: the font, and how much output they show.
#[derive(Clone)]
pub struct ToolStyle {
    pub family: SharedString,
    pub size: gpui_kit::Pixels,
    pub max_chars: usize,
}

/// What the transcript needs to make file references in the assistant's text clickable.
#[derive(Clone)]
struct FileLinks {
    cwd: std::path::PathBuf,
    /// The window's app and this transcript's session: what a saved scenario's card starts it
    /// in or opens it with.
    app: gpui_kit::WeakEntity<ReactorApp>,
    session: String,
    /// Path as written, its line, the modifier keys held for the click.
    open: std::sync::Arc<OpenReference>,
    /// Open a `docs_search` result in the Documents panel: the query, its passages and the
    /// one to select.
    show_docs: std::sync::Arc<ShowDocs>,
    /// Put a message's text on the clipboard: its index and its text as written.
    copy: std::sync::Arc<CopyText>,
    /// The message just copied.
    copied: Option<usize>,
}

/// A message's index and its text, to put on the clipboard.
type CopyText = dyn Fn(usize, String, &mut Window, &mut App) + Send + Sync;

type ShowDocs = dyn Fn(String, Vec<reactor_agent::tools::docs::ResultPassage>, usize, &mut Window, &mut App)
    + Send
    + Sync;

type OpenReference = dyn Fn(String, Option<u32>, Modifiers, &mut Window, &mut App) + Send + Sync;

/// Under a scenario the agent saved: *Start here*, *Start in a new session*, *Edit*.
fn scenario_card(
    index: usize,
    id: String,
    links: &FileLinks,
    theme: &gpui_kit::component::Theme,
) -> gpui_kit::Div {
    let action = |new_session: Option<bool>| {
        let (app, session, id) = (links.app.clone(), links.session.clone(), id.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            let _ = app.update(cx, |app, cx| match new_session {
                Some(new) => {
                    app.activate_session(&session, window, cx);
                    app.open_scenario_start(&id, new, window, cx);
                }
                None => app.open_scenario_editor(
                    crate::scenario_editor::Start::Edit(id.clone()),
                    cx,
                ),
            });
        }
    };
    h_flex()
        .gap_2()
        .items_center()
        .px_2()
        .py_1()
        .mt_1()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .child(Label::new("Scenario").text_color(theme.muted_foreground))
        .child(
            div()
                .font_family(theme.mono_font_family.clone())
                .child(SharedString::from(id.clone())),
        )
        .child(Label::new("saved -- not started").text_color(theme.muted_foreground))
        .child(div().flex_1())
        .child(
            Button::new(("scenario-card-edit", index))
                .label("Edit")
                .small()
                .ghost()
                .on_click(action(None)),
        )
        .child(
            Button::new(("scenario-card-new", index))
                .label("Start in a new session")
                .small()
                .tooltip("A new session of this case begins with an empty context")
                .on_click(action(Some(true))),
        )
        .child(
            Button::new(("scenario-card-here", index))
                .label("Start here")
                .small()
                .primary()
                .tooltip("Start it in this session, which keeps what was discussed")
                .on_click(action(Some(false))),
        )
}

// One entry's look is one function of everything it draws; a context struct would only rename
// these arguments.
#[allow(clippy::too_many_arguments)]
fn render_item(
    index: usize,
    item: &ChatItem,
    prev: Option<&ChatItem>,
    out_of_context: bool,
    theme: &gpui_kit::component::Theme,
    tools: &ToolStyle,
    thinking_expanded: bool,
    tool_expanded: bool,
    toggle_thinking: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    toggle_tool: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    restore: impl Fn(u64, &mut Window, &mut App) + Clone + 'static,
    detach: impl Fn(u64, &mut Window, &mut App) + Clone + 'static,
    links: &FileLinks,
) -> gpui_kit::Div {
    use crate::transcript_ui::{self as ui, Node};
    // The agent's name goes above its first row after something else was said.
    let continues_agent = matches!(
        prev,
        Some(
            ChatItem::AssistantText { .. }
                | ChatItem::Thinking { .. }
                | ChatItem::ToolCall { .. }
                | ChatItem::Reduction { .. }
        )
    );
    match item {
        // What you said is a shade lighter than the window.
        ChatItem::User { text } => {
            // An absolute lift: `lighten` scales, and a near-black window has nothing to scale.
            let lifted = gpui_kit::Hsla {
                l: (theme.background.l + 0.04).min(1.0),
                ..theme.background
            };
            ui::lane(
                Node::User,
                v_flex().py_1().child(
                    v_flex()
                        .gap_0p5()
                        .rounded_lg()
                        .px_3()
                        .py_2()
                        .bg(lifted)
                        .child(ui::speaker("YOU", theme.accent, theme))
                        .child(div().child(text.clone())),
                ),
                theme,
            )
        }
        ChatItem::AssistantText { text, streaming } => {
            let streaming = *streaming;
            let linked = crate::files::refs::linkify(text, &|path| {
                crate::files::refs::exists(&links.cwd, path)
            });
            let open = links.open.clone();
            // The text as the model wrote it — Markdown and all — for the clipboard.
            let (copy, raw) = (links.copy.clone(), text.clone());
            let copied = links.copied == Some(index);
            ui::lane(
                Node::Agent,
                v_flex()
                    .py_1()
                    .when(!continues_agent, |el| {
                        el.child(
                            div()
                                .px_2()
                                .child(ui::speaker("REACTOR", theme.muted_foreground, theme)),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .px_2()
                            .pr_12()
                            .child(
                                // Clear of the scrollbar, which runs down the right edge.
                                div().absolute().top_0().right(px(16.)).child(
                                    Button::new(("copy-message", index))
                                        .icon(if copied {
                                            IconName::Check
                                        } else {
                                            IconName::Copy
                                        })
                                        .ghost()
                                        .small()
                                        .tooltip(if copied {
                                            "Copied"
                                        } else {
                                            "Copy the message as written (Markdown)"
                                        })
                                        .on_click(move |_, window, cx| {
                                            copy(index, raw.clone(), window, cx)
                                        }),
                                ),
                            )
                            .child(
                                TextView::markdown(format!("assistant-{index}"), linked)
                                    .style(crate::theme::text_view_style(theme))
                                    .on_link_click(move |url, event, window, cx| {
                                        match crate::files::refs::parse_url(url) {
                                            Some((path, line)) => {
                                                open(path, line, event.modifiers(), window, cx)
                                            }
                                            None => cx.open_url(url),
                                        }
                                    }),
                            )
                            .when(streaming, |el| el.opacity(0.85)),
                    ),
                theme,
            )
        }
        ChatItem::Thinking { text, streaming } => {
            let chevron = if thinking_expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            ui::lane(
                Node::Thinking { live: *streaming },
                v_flex().px_2().py_0p5().child(
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
                                .child(if *streaming {
                                    // Still thinking: a shimmer sweeps over the word.
                                    div()
                                        .text_color(theme.muted_foreground)
                                        .child(crate::indicator::shimmer(
                                            "thinking…",
                                            theme.foreground,
                                        ))
                                        .into_any_element()
                                } else {
                                    Label::new("thinking")
                                        .text_color(theme.muted_foreground)
                                        .into_any_element()
                                }),
                        )
                        .content(
                            div().child(
                                TextView::markdown(format!("thinking-{index}"), text.clone())
                                    .style(crate::theme::text_view_style(theme)),
                            ),
                        ),
                ),
                theme,
            )
        }
        ChatItem::ToolCall {
            name,
            args,
            output,
            is_error,
            done,
            entry,
            ..
        } => {
            // A finished passage search is shown as a list, not as the tagged text the model got.
            let docs = (name == "docs_search" && *done && !*is_error)
                .then(|| reactor_agent::tools::docs::parse_result(output))
                .flatten();
            let docs = docs.as_ref().map(|parsed| {
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                (
                    parsed.summary.clone(),
                    docs_result(index, parsed, query, theme, tools, links),
                )
            });
            let copy = links.copy.clone();
            // A scenario the agent saved: the person starts it, here or in a new session, or
            // opens it in the editor first (ADR-0065).
            let saved_scenario = (name == "scenario_save" && *done && !*is_error)
                .then(|| args.get("id").and_then(Value::as_str).map(str::to_string))
                .flatten()
                .map(|id| scenario_card(index, id, links, theme));
            ui::lane(
                Node::Tool(ui::tool_state(*done, *is_error)),
                v_flex().py_0p5().child(ui::tool_row(
                    ui::ToolCallView {
                        index,
                        name,
                        args,
                        output,
                        entry: *entry,
                        is_error: *is_error,
                        done: *done,
                        out_of_context,
                        expanded: tool_expanded,
                        copied: links.copied == Some(index),
                    },
                    docs,
                    theme,
                    tools,
                    move |window, cx| toggle_tool(index, window, cx),
                    move |text, window, cx| copy(index, text, window, cx),
                ))
                .children(saved_scenario),
                theme,
            )
        }
        ChatItem::Reduction {
            entry,
            mode,
            trigger,
            covers,
            before,
            after,
            active,
            summary,
        } => ui::reduction_seam(
            ui::ReductionView {
                entry: *entry,
                index,
                mode,
                trigger,
                covers: *covers,
                before: *before,
                after: *after,
                active: *active,
                summary: summary.as_deref(),
            },
            theme,
            restore,
        ),
        ChatItem::Attachment {
            entry,
            label,
            part,
            source,
            tokens,
            truncated,
            via,
            active,
        } => {
            let entry = *entry;
            let active = *active;
            let from_panel = via == "panel";
            let title = if from_panel {
                "You gave the agent a document"
            } else {
                "Sent with this message"
            };
            let place = [part.clone(), format!("≈ {tokens} tokens")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            ui::lane(
                Node::Document,
                v_flex().px_2().py_0p5().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .child(Label::new(title).text_color(theme.muted_foreground))
                        .child(crate::hints::tip(
                            div()
                                .id(("attachment-label", index))
                                .child(Label::new(label.clone())),
                            source.clone(),
                        ))
                        .child(
                            Label::new(place)
                                .text_color(theme.muted_foreground)
                                .text_size(theme.font_size * 0.85),
                        )
                        .when(*truncated, |el| {
                            el.child(crate::hints::tip(
                                div()
                                    .id(("attachment-cut", index))
                                    .text_color(theme.warning)
                                    .text_size(theme.font_size * 0.85)
                                    .child("cut"),
                                "Longer than one attachment may be: only the start was given.",
                            ))
                        })
                        .child(div().flex_1())
                        .when(active, |el| {
                            el.child(
                                Button::new(("detach", index))
                                    .label(if from_panel { "undo" } else { "remove" })
                                    .small()
                                    .tooltip("Take it out of the model's context again. It stays in the log")
                                    .on_click(move |_, window, cx| detach(entry, window, cx)),
                            )
                        })
                        .when(!active, |el| {
                            el.child(
                                Label::new("removed from the context")
                                    .text_color(theme.muted_foreground)
                                    .text_size(theme.font_size * 0.85),
                            )
                        }),
                ),
                theme,
            )
        }
        ChatItem::Stats {
            date,
            time,
            duration_ms,
            tokens,
            stream_ms,
        } => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            ui::lane(
                Node::None,
                div().px_2().pt_1().child(
                    div()
                        .pt_1()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_color(theme.muted_foreground)
                        .text_size(theme.font_size * 0.8)
                        .child(crate::session::stats_label(
                            date,
                            time,
                            *duration_ms,
                            *tokens,
                            *stream_ms,
                            &today,
                        )),
                ),
                theme,
            )
        }
        ChatItem::Error { message } => ui::lane(
            Node::Failure,
            v_flex().py_1().child(
                div()
                    .px_3()
                    .py_1p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.danger.opacity(0.4))
                    .bg(theme.danger.opacity(0.15))
                    .text_color(theme.danger)
                    .child(message.clone()),
            ),
            theme,
        ),
    }
}

/// How many matches of a search the transcript lists; the rest are in the panel.
const DOCS_LINES: usize = 5;

/// A `docs_search` result for the transcript: what the search was, and one line for each of
/// the best matches (title, section, the start of the passage). A line opens the whole
/// result in the Documents panel at that passage; its tooltip has more of the text.
fn docs_result(
    index: usize,
    parsed: &reactor_agent::tools::docs::ParsedResult,
    query: String,
    theme: &gpui_kit::component::Theme,
    tools: &ToolStyle,
    links: &FileLinks,
) -> gpui_kit::Div {
    let small = tools.size;
    let words = crate::docs_panel::query_words(&query);
    let open = links.show_docs.clone();
    let passages = parsed.passages.clone();
    let lines = parsed
        .passages
        .iter()
        .take(DOCS_LINES)
        .enumerate()
        .map(|(i, p)| {
            let (open, passages, query) = (open.clone(), passages.clone(), query.clone());
            let heading = if p.title.is_empty() {
                p.source.clone()
            } else {
                p.title.clone()
            };
            let more: String = p.text.chars().take(600).collect();
            let row = h_flex()
                .id(SharedString::from(format!("docs-line-{index}-{i}")))
                .gap_2()
                .px_1()
                .rounded_sm()
                .overflow_hidden()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .text_size(small)
                .on_click(move |_, window, cx| open(query.clone(), passages.clone(), i, window, cx))
                .child(
                    div()
                        .flex_none()
                        .w(px(16.))
                        .text_color(theme.muted_foreground)
                        .child(format!("{}", i + 1)),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w(px(220.))
                        .truncate()
                        .text_color(theme.foreground)
                        .child(SharedString::from(heading)),
                )
                .when(!p.section.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .max_w(px(200.))
                            .truncate()
                            .text_color(theme.info)
                            .child(SharedString::from(p.section.clone())),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(crate::docs_panel::snippet(
                            &p.text, &words, 140,
                        ))),
                );
            crate::hints::tip(row, SharedString::from(format!("{}\n\n{more}", p.source)))
        })
        .collect::<Vec<_>>();
    let rest = parsed.passages.len().saturating_sub(DOCS_LINES);
    let all = {
        let (open, passages, query) = (open.clone(), passages.clone(), query.clone());
        Button::new(("docs-open-all", index))
            .small()
            .ghost()
            .icon(IconName::BookOpen)
            .label(if rest > 0 { format!("All {} in Documents", parsed.passages.len()) } else { "Open in Documents".to_string() })
            .tooltip("Open this result in the Documents panel: the whole passage, its neighbours, its source")
            .on_click(move |_, window, cx| open(query.clone(), passages.clone(), 0, window, cx))
    };
    v_flex()
        .px_2()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(small)
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(parsed.summary.clone())),
                )
                .child(all),
        )
        .children(lines)
}

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
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Which session this tab is, and whether it is the one the rest of the window
        // (tree, context, status) follows.
        let active = self
            .app
            .upgrade()
            .is_some_and(|a| a.read(cx).active_session_id() == self.session_id);
        let short = crate::session_list::short_id(&self.session_id);
        panel_title_closing(
            IconName::MessageSquare,
            &format!("Transcript · {short}{}", if active { " ●" } else { "" }),
            closer(&self.app, cx.entity()),
        )
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let (app, id) = (self.app.clone(), self.session_id.clone());
        Some(vec![
            // The tab's own close only hides it: the session stays open and keeps working.
            // This lets go of the session itself.
            Button::new("close-session-tab")
                .icon(IconName::PowerOff)
                .ghost()
                .small()
                .tooltip("Close this session: its tab goes and the window lets go of it. It stays in the case and can be opened again. (Closing just the tab hides it, and the session keeps running.)")
                .on_click(move |_, window, cx| {
                    if let Some(app) = app.upgrade() {
                        let id = id.clone();
                        app.update(cx, |app, cx| app.close_session(&id, window, cx));
                    }
                }),
            close_button(&self.app, cx.entity()),
        ])
    }
}

impl Render for TranscriptPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ui = self
            .app
            .upgrade()
            .map(|a| a.read(cx).ui.clone())
            .unwrap_or_default();
        let (body_family, body_size) = ui.text(Slot::Transcript, &theme);
        let (tool_family, tool_size) = ui.text(Slot::Tools, &theme);
        let (prompt_family, prompt_size) = ui.text(Slot::Prompt, &theme);
        let tool_style = ToolStyle {
            family: tool_family.clone(),
            size: tool_size,
            max_chars: ui.tool_output_chars,
        };
        let (items, hidden, phase, scheduled, last_error, run) = {
            let session = self.session.read(cx);
            (
                session.items.clone(),
                session.hidden.clone(),
                session.phase,
                session.waiting.clone(),
                session.last_error.clone(),
                session
                    .run
                    .as_ref()
                    .map(|r| (r.elapsed(std::time::Instant::now()), r.tokens())),
            )
        };

        // Tell the scroller about new content before rendering it. An append keeps
        // tail-following; a shrink (a branch switch, a fresh session) resets the anchor; and a
        // row whose content changed is measured again on its own — a streaming delta grows
        // one row's height without changing how many rows there are, and measuring every row
        // for it (with a proportional anchor) is both slow and what moves a transcript the
        // person has scrolled up in.
        let signs: Vec<u64> = items.iter().map(crate::transcript_ui::row_sign).collect();
        let (old_len, new_len) = (self.row_signs.len(), signs.len());
        let layout_key = format!(
            "{body_family:?}{body_size:?}{tool_family:?}{tool_size:?}{}{}",
            ui.tool_output_chars, ui.expand_thinking
        );
        if new_len < old_len {
            self.expanded_tools.clear();
            self.expanded_thinking.clear();
            self.scroller
                .update(cx, |state, cx| state.reset(new_len, cx));
        } else {
            let changed = crate::transcript_ui::changed_rows(&self.row_signs, &signs);
            let style_changed = layout_key != self.layout_key;
            self.scroller.update(cx, |state, cx| {
                for range in changed {
                    state.remeasure_items(range, cx);
                }
                if new_len > old_len {
                    state.append(new_len - old_len, cx);
                }
                if style_changed {
                    state.remeasure(cx);
                }
            });
        }
        self.layout_key = layout_key;
        self.row_signs = signs;

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
            let expanded_tools = self.expanded_tools.clone();
            let (open_thinking, dim_reduced) = (ui.expand_thinking, ui.dim_reduced);
            let tool_style = tool_style.clone();
            let weak_self = self.weak_self.clone();
            let weak_app = self.app.clone();
            let toggle_thinking = {
                let weak_self = weak_self.clone();
                move |index: usize, _window: &mut Window, cx: &mut App| {
                    if let Some(panel) = weak_self.upgrade() {
                        panel.update(cx, |panel, cx| {
                            if !panel.expanded_thinking.remove(&index) {
                                panel.expanded_thinking.insert(index);
                            }
                            // Its height changed: this row, not the whole transcript.
                            panel.scroller.update(cx, |state, cx| {
                                state.remeasure_items(index..index + 1, cx);
                            });
                            cx.notify();
                        });
                    }
                }
            };
            let toggle_tool = move |index: usize, _window: &mut Window, cx: &mut App| {
                if let Some(panel) = weak_self.upgrade() {
                    panel.update(cx, |panel, cx| {
                        if !panel.expanded_tools.remove(&index) {
                            panel.expanded_tools.insert(index);
                        }
                        panel.scroller.update(cx, |state, cx| {
                            state.remeasure_items(index..index + 1, cx);
                        });
                        cx.notify();
                    });
                }
            };
            let links = FileLinks {
                app: self.app.clone(),
                session: self.session_id.clone(),
                cwd: self
                    .app
                    .upgrade()
                    .map(|a| a.read(cx).cwd.clone())
                    .unwrap_or_default(),
                open: {
                    let app = self.app.clone();
                    std::sync::Arc::new(move |path, line, modifiers, window, cx| {
                        if let Some(app) = app.upgrade() {
                            app.update(cx, |app, cx| {
                                app.open_reference(&path, line, modifiers, window, cx)
                            });
                        }
                    })
                },
                copy: {
                    let weak_self = self.weak_self.clone();
                    std::sync::Arc::new(move |index, text, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        if let Some(panel) = weak_self.upgrade() {
                            panel.update(cx, |panel, cx| {
                                panel.copied = Some(index);
                                cx.notify();
                                // The tick goes after a moment, if nothing else was copied.
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(1500))
                                        .await;
                                    let _ = this.update(cx, |panel, cx| {
                                        if panel.copied == Some(index) {
                                            panel.copied = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                            });
                        }
                    })
                },
                copied: self.copied,
                show_docs: {
                    let app = self.app.clone();
                    std::sync::Arc::new(move |query, passages, index, window, cx| {
                        if let Some(app) = app.upgrade() {
                            app.update(cx, |app, cx| {
                                app.show_docs(query, passages, index, window, cx)
                            });
                        }
                    })
                },
            };
            let detach_app = weak_app.clone();
            let restore = move |entry: u64, _window: &mut Window, cx: &mut App| {
                if let Some(app) = weak_app.upgrade() {
                    app.update(cx, |app, cx| app.restore_reduction(entry, cx));
                }
            };
            let detach = move |entry: u64, _window: &mut Window, cx: &mut App| {
                if let Some(app) = detach_app.upgrade() {
                    app.update(cx, |app, cx| app.detach_attachment(entry, cx));
                }
            };
            MessageScroller::new(
                "transcript",
                self.scroller.clone(),
                move |index, _window, _cx| {
                    let row = render_item(
                        index,
                        &items[index],
                        index.checked_sub(1).map(|i| &items[i]),
                        hidden.contains(&index),
                        &render_theme,
                        &tool_style,
                        // The default flips on the choice: a click toggles either way.
                        expanded_thinking.contains(&index) != open_thinking,
                        expanded_tools.contains(&index),
                        toggle_thinking.clone(),
                        toggle_tool.clone(),
                        restore.clone(),
                        detach.clone(),
                        &links,
                    );
                    // What a reduction has taken out of the model's context stays on screen,
                    // dimmed: the log keeps everything, and so does the transcript.
                    if dim_reduced && hidden.contains(&index) {
                        row.opacity(0.45)
                    } else {
                        row
                    }
                },
            )
            // Rows touch: the lane's line runs through them, and each row pads itself.
            .with_row_style(gpui_kit::StyleRefinement::default().px_0().pb_0())
            .with_bottom_fade(theme.background)
            .into_any_element()
        };

        // The composer: Enter submits (queued as a follow-up while the agent
        // works) — the subscription lives on ReactorApp, the view here.
        // No marker at all while nothing is being processed.
        let phase_label = match phase {
            AgentPhase::Idle => None,
            AgentPhase::Working => Some("working"),
            AgentPhase::Compacting => Some("reducing the context"),
        };
        // A shimmering line at the foot of the transcript: the agent is working on a prompt.
        // Nothing at all when it is not.
        let running = phase_label.map(|label| {
            div()
                .id("agent-running")
                .px_3()
                .py_1()
                .text_color(theme.muted_foreground)
                .child(crate::indicator::shimmer(
                    match run {
                        // How long this prompt has run and what it has streamed so far.
                        Some((elapsed, tokens)) if tokens > 0 => format!(
                            "{label} · {} · {tokens} tokens",
                            crate::session::format_duration(elapsed.as_millis() as u64)
                        ),
                        Some((elapsed, _)) => format!(
                            "{label} · {}",
                            crate::session::format_duration(elapsed.as_millis() as u64)
                        ),
                        None => label.to_string(),
                    },
                    theme.foreground,
                ))
        });
        // Messages scheduled to run right after this prompt, each a marker: what they say, and
        // (on hover) how to bring them back into the input.
        let scheduled_markers: Vec<_> = scheduled
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let first = p.text.lines().next().unwrap_or("");
                let shown: String = if first.chars().count() > 90 { first.chars().take(90).chain(std::iter::once('…')).collect() } else { first.to_string() };
                let (word, tip) = match p.class {
                    reactor_agent::inbox::Class::Steer => (
                        "steering",
                        "Reaches the model at the next step of the turn, after the tool call that is running. Press ↑ in the empty prompt to take it back and change it.",
                    ),
                    reactor_agent::inbox::Class::Queue => (
                        "queued",
                        "Runs as a prompt of its own once this one is over. Press ↑ in the empty prompt to take it back and change it.",
                    ),
                };
                crate::hints::tip(
                    div().id(("scheduled", i)).px_3().py_1().child(
                        Marker::new()
                            .id(("scheduled-marker", i))
                            .text_color(theme.muted_foreground)
                            .icon(MarkerIcon::new().child(Icon::new(IconName::Clock).small()))
                            .content(MarkerContent::new().text(format!("{word}: {shown}"))),
                    ),
                    tip,
                )
            })
            .collect();

        // The `/` completion popup, above the input: the ranked commands for what has been
        // typed. Keys (up/down/Enter/Tab/Esc) reach it through the `SlashPopup` key context.
        let slash = self.app.upgrade().map(|a| {
            let slash = &a.read(cx).slash;
            (
                slash.is_open(),
                slash.items.clone(),
                slash.selected,
                slash.reference.is_some(),
            )
        });
        let (slash_open, slash_items, slash_selected, reference) = slash.unwrap_or_default();
        let popup = slash_open.then(|| {
            let mut list = v_flex()
                .id("slash-popup")
                .gap_0p5()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover);
            for (i, entry) in slash_items.iter().enumerate() {
                let selected = i == slash_selected;
                list = list.child(
                    h_flex()
                        .id(("slash-row", i))
                        .gap_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(selected, |row| row.bg(theme.list_active))
                        .hover(|row| row.bg(theme.list_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(app) = this.app.upgrade() {
                                app.update(cx, |app, cx| app.slash_pick(i, window, cx));
                            }
                        }))
                        .child(
                            div()
                                .min_w(px(140.))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size)
                                .text_color(theme.accent)
                                .child(format!(
                                    "{}{}",
                                    if reference { "#" } else { "/" },
                                    entry.run
                                )),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .text_size(theme.font_size * 0.85)
                                .child(entry.detail.clone()),
                        ),
                );
            }
            list
        });

        // What the pane shows of its own session around the conversation (`crate::pane`).
        let (pane, toolsets, tools, models, (thinking, thinking_levels)) = self
            .app
            .upgrade()
            .map(|a| {
                let a = a.read(cx);
                (
                    a.pane(&self.session_id, cx),
                    a.toolsets.clone(),
                    a.catalogue.clone(),
                    a.models.clone(),
                    a.thinking_state(),
                )
            })
            .unwrap_or_default();
        let idle = phase == AgentPhase::Idle;
        let local = |change: fn(&mut Self)| -> crate::pane::Local {
            let weak = self.weak_self.clone();
            std::rc::Rc::new(move |_window, cx| {
                let _ = weak.update(cx, |panel, cx| {
                    change(panel);
                    cx.notify();
                });
            })
        };
        let select: crate::pane::SelectStep = {
            let weak = self.weak_self.clone();
            std::rc::Rc::new(move |step, _window, cx| {
                let _ = weak.update(cx, |panel, cx| {
                    panel.step_selected = step;
                    cx.notify();
                });
            })
        };
        let choose: crate::pane::Flip = {
            let weak = self.weak_self.clone();
            std::rc::Rc::new(move |on, _window, cx| {
                let _ = weak.update(cx, |panel, cx| {
                    panel.choosing_scenario = on;
                    cx.notify();
                });
            })
        };
        let header = v_flex()
            .flex_none()
            .child(crate::pane::context_bar(
                &self.app,
                &self.session_id,
                pane.context.as_ref(),
                self.details_open,
                local(|p| p.details_open = !p.details_open),
                &theme,
            ))
            .children(crate::pane::goal_line(
                &self.app,
                &self.session_id,
                &pane.plan,
                self.goal_open,
                local(|p| p.goal_open = !p.goal_open),
                &theme,
            ));
        let run = pane.scenario.run.as_ref();
        let steps = run.map(|run| {
            crate::pane::stepper(
                &self.app,
                &self.session_id,
                run,
                self.step_selected,
                idle,
                select.clone(),
                &theme,
            )
        });
        let step_card = run.zip(self.step_selected).and_then(|(run, ix)| {
            crate::pane::step_card(
                &self.app,
                &self.session_id,
                run,
                ix,
                toolsets.as_ref(),
                tools.as_ref(),
                idle,
                select.clone(),
                &theme,
            )
        });
        let chips = crate::pane::chips(
            &self.app,
            &self.session_id,
            &pane,
            idle,
            self.choosing_scenario,
            choose,
            &theme,
        );
        let model_picker = Button::new("model-picker")
            .ghost()
            .small()
            .label(format!(
                "{} · {}",
                pane.model.clone().unwrap_or_else(|| "no model".to_owned()),
                crate::app::thinking_label(thinking.as_deref()),
            ))
            .disabled(models.is_empty())
            .dropdown_menu(move |mut menu, _window, _cx| {
                for spec in &models {
                    menu = menu.menu(
                        spec.clone(),
                        Box::new(crate::app::SelectModelAction { spec: spec.clone() }),
                    );
                }
                // The thinking level of the model in use, in the same menu: only the levels
                // its provider takes are listed, and `Default` leaves it to the model.
                if !thinking_levels.is_empty() {
                    menu = menu.separator().label("Thinking");
                    for level in std::iter::once(None)
                        .chain(thinking_levels.iter().map(|l| Some(l.to_string())))
                    {
                        let on = level == thinking;
                        menu = menu.menu_with_check(
                            crate::app::thinking_label(level.as_deref()),
                            on,
                            Box::new(crate::app::SelectThinkingAction {
                                level: level.unwrap_or_default(),
                            }),
                        );
                    }
                }
                menu
            });

        let composer = v_flex()
            .key_context(if slash_open { "PromptComposer SlashPopup" } else { "PromptComposer" })
            .border_t_1()
            .border_color(cx.theme().border)
            .p_2()
            .gap_2()
            .children(popup)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().font_family(prompt_family).text_size(prompt_size).child(Textarea::new(&self.composer)))
                    // One button: it sends, and while the agent works it interrupts. Enter in
                    // the prompt still steers it, and Ctrl+Enter queues a message for after.
                    .child(if idle {
                        Button::new("send")
                            .tooltip("Enter sends · Shift+Enter starts a new line · / lists commands · Ctrl+P opens the palette · while the agent works, Enter steers it and Ctrl+Enter queues")
                            .primary()
                            .label("Send")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(app) = this.app.upgrade() {
                                    app.update(cx, |app, cx| app.send_composer(false, window, cx));
                                }
                            }))
                    } else {
                        Button::new("interrupt")
                            .tooltip("Stop the agent. Enter in the prompt steers it at the next step instead, Ctrl+Enter queues a message for after, and ↑ in the empty prompt brings the last one back")
                            .danger()
                            .label("Interrupt")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(app) = this.app.upgrade() {
                                    app.update(cx, |app, cx| {
                                        app.activate_session(&this.session_id, window, cx);
                                        app.interrupt(window, cx)
                                    });
                                }
                            }))
                    }),
            )
            // The chips, with the model and its thinking level at the right of the same line.
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(chips))
                    .child(div().flex_none().child(model_picker)),
            );

        let mut panel = v_flex()
            .id(SharedString::from(format!(
                "transcript-{}",
                self.session_id
            )))
            .track_focus(&self.focus)
            .size_full()
            // A click anywhere in the tab makes its session the active one.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.make_active(window, cx)),
            )
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .font_family(body_family)
                            .text_size(body_size)
                            .child(div().flex_1().min_h_0().child(transcript))
                            .children(running)
                            .children(scheduled_markers),
                    )
                    .children(steps)
                    .children(step_card),
            )
            .child(composer);

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
// TreePanel — left: the session's branches (SPEC.md §4.6)
// ---------------------------------------------------------------------------

/// The left panel: the session tree, read straight from the session store. Only user
/// turns, final replies, reductions and labels are rows — tool rounds are elided — and
/// depth grows only where the tree actually forks. A branch is continued from a final
/// reply ("Continue from here"), disabled while the agent works.
pub struct TreePanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    /// The row the user clicked — the switch affordance acts on it.
    selection: Option<u64>,
    /// The session picked in the list of the case's sessions.
    session_selection: Option<String>,
    /// The session `selection` is an entry of: entry numbers are each session's own.
    for_session: String,
    focus: FocusHandle,
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
            session_selection: None,
            for_session: String::new(),
        }
    }
}

impl Render for TreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // The tree is the active session's: a row picked in another one's means nothing.
        let active = self
            .app
            .upgrade()
            .map(|a| a.read(cx).active_session_id())
            .unwrap_or_default();
        if self.for_session != active {
            self.for_session = active;
            self.selection = None;
        }
        let working: Vec<String> = self
            .app
            .upgrade()
            .map(|a| a.read(cx).working_ids())
            .unwrap_or_default();
        let (rows, streaming, sessions): (
            Vec<TreeRow>,
            bool,
            Vec<crate::session_list::SessionRow>,
        ) = match self.app.upgrade() {
            Some(app) => {
                let read = app.read(cx);
                (
                    read.tree.clone(),
                    read.session.read(cx).phase != AgentPhase::Idle,
                    read.sessions.clone(),
                )
            }
            None => (Vec::new(), true, Vec::new()),
        };
        let selectable = |id: u64| rows.iter().any(|r| r.id == id && r.can_switch);

        let mut list = v_flex().p_2().gap_1();
        for row in &rows {
            let id = row.id;
            let selected = self.selection == Some(id);
            let colour = if row.is_head {
                theme.accent
            } else if row.on_branch {
                theme.foreground
            } else {
                theme.muted_foreground
            };
            list = list.child(
                div()
                    .id(("tree-row", id as usize))
                    .pl(px(8. * row.depth as f32))
                    .flex()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    // The tip (where the next prompt goes) is the list's active colour; the row
                    // picked for "Continue from here" is Ayu's blue, with an edge, so the two
                    // never look alike.
                    .when(row.is_head, |el| el.bg(theme.tokens.list_active))
                    .when(selected, |el| {
                        el.bg(theme.info.opacity(0.28))
                            .border_l_2()
                            .border_color(theme.info)
                    })
                    .when(!(row.is_head || selected), |el| {
                        el.hover(|style| style.bg(theme.list_hover))
                    })
                    .px_2()
                    .py_1()
                    .when(row.can_switch, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.selection = Some(id);
                                cx.notify();
                            }))
                    })
                    .child(
                        div()
                            .text_color(colour)
                            .text_size(theme.font_size * 0.85)
                            .child(row.label.clone()),
                    ),
            );
        }
        if rows.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child("no session tree yet — send a prompt"),
            );
        }

        let can_switch = self.selection.is_some_and(selectable) && !streaming;

        // The case's sessions. A click picks one; a double click, or Open, shows its
        // transcript in a tab of its own, next to the ones already open.
        let picked = self
            .session_selection
            .as_ref()
            .and_then(|id| sessions.iter().find(|s| s.id == *id))
            .cloned();
        let can_open = picked.as_ref().is_some_and(|s| !s.open_elsewhere);
        let can_close = picked.as_ref().is_some_and(|s| s.open_here);
        // A child agent that is running can be stopped from here.
        let can_stop = picked
            .as_ref()
            .is_some_and(|s| s.parent.is_some() && working.contains(&s.id));
        let active_id = sessions
            .iter()
            .find(|s| s.current)
            .map(|s| crate::session_list::short_id(&s.id))
            .unwrap_or_default();
        let mut session_list = v_flex().p_2().gap_1();
        let case_id = self
            .app
            .upgrade()
            .map(|a| a.read(cx).case_id())
            .unwrap_or_default();
        for s in sessions.iter() {
            let id = s.id.clone();
            let is_working = working.contains(&s.id);
            let selected = self.session_selection.as_deref() == Some(s.id.as_str());
            let tip = if s.current {
                "The active session: the tree, the context and the status follow it"
            } else if s.open_elsewhere {
                "Open in another window: it can be opened here once that one is closed"
            } else if s.open_here {
                "Open in a tab. Double-click to bring it forward and make it the active one"
            } else {
                "Click to pick it; double-click to open its transcript (a child agent that is running is followed live)"
            };
            session_list = session_list.child(crate::hints::tip(
                v_flex()
                    .id(SharedString::from(format!("session-{}", s.id)))
                    .relative()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    // A child agent's session sits one step in under the one that started it,
                    // with a line down its left edge.
                    .when(s.depth > 0, |el| {
                        el.ml(px(14. * s.depth as f32))
                            .border_l_1()
                            .border_color(theme.border)
                    })
                    .when(s.current, |el| el.bg(theme.tokens.list_active))
                    .when(selected, |el| {
                        el.bg(theme.info.opacity(0.28))
                            .border_l_2()
                            .border_color(theme.info)
                    })
                    // The whole row breathes while the session works: a wash of the accent
                    // colour, behind the text, that swells and fades.
                    .when(is_working, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .rounded_md()
                                .bg(theme.accent.opacity(0.05 + 0.2 * crate::indicator::pulse())),
                        )
                    })
                    .when(!(s.current || selected), |el| {
                        el.hover(|style| style.bg(theme.list_hover))
                    })
                    .when(!s.open_elsewhere, |el| {
                        let open = s.clone();
                        el.cursor_pointer().on_click(cx.listener(
                            move |this, event: &gpui_kit::ClickEvent, window, cx| {
                                this.session_selection = Some(id.clone());
                                if event.click_count() >= 2
                                    && let Some(app) = this.app.upgrade()
                                {
                                    let id = open.id.clone();
                                    app.update(cx, |app, cx| {
                                        app.open_session(Some(id), window, cx)
                                    });
                                }
                                cx.notify();
                            },
                        ))
                    })
                    .child({
                        let text = if s.prompt.is_empty() {
                            "(nothing said yet)".to_string()
                        } else {
                            s.prompt.clone()
                        };
                        // One line, cut by the row: a plain clipping box around the text, as
                        // the shimmer is drawn like the transcript's own "thinking" one — a
                        // text element in an ordinary div, not under a truncating style.
                        let colour = if s.open_elsewhere {
                            theme.muted_foreground
                        } else {
                            theme.foreground
                        };
                        if is_working {
                            div()
                                .w_full()
                                .overflow_hidden()
                                .text_size(theme.font_size * 0.9)
                                .text_color(theme.muted_foreground)
                                .child(crate::indicator::shimmer(text, theme.accent))
                        } else {
                            div()
                                .truncate()
                                .text_size(theme.font_size * 0.9)
                                .text_color(colour)
                                .child(SharedString::from(text))
                        }
                    })
                    .child({
                        let mut tags: Vec<String> = Vec::new();
                        if let Some(parent) = &s.parent {
                            tags.push(format!("agent of {}", crate::session_list::short_id(parent)));
                        }
                        if s.current {
                            tags.push("active".into());
                        } else if s.open_here {
                            tags.push("open".into());
                        }
                        if is_working {
                            tags.push("working".into());
                        }
                        if s.open_elsewhere {
                            tags.push("open elsewhere".into());
                        }
                        let line = format!(
                            "{} · {} entr{} · {}{}",
                            s.started,
                            s.entries,
                            if s.entries == 1 { "y" } else { "ies" },
                            crate::session_list::short_id(&s.id),
                            tags.iter().map(|t| format!(" · {t}")).collect::<String>()
                        );
                        let base = div()
                            .w_full()
                            .overflow_hidden()
                            .text_size(theme.font_size * 0.8)
                            .text_color(if s.current {
                                theme.accent
                            } else {
                                theme.muted_foreground
                            });
                        // The line under it shimmers too, in the same sweep.
                        if is_working {
                            base.child(crate::indicator::shimmer(line, theme.foreground))
                        } else {
                            base.child(SharedString::from(line))
                        }
                    }),
                // The session named in full, the way it is named across cases.
                format!("{}\n{tip}", crate::session_list::reference(&case_id, &s.id)),
            ));
        }
        let header = h_flex()
                    .justify_between()
                    .items_center()
                    .px_2()
                    .child(
                        Label::new(format!("Sessions ({})", sessions.len()))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("new-session")
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .small()
                                    .tooltip("Start a new session in this case, in a tab of its own")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(app) = this.app.upgrade() {
                                            app.update(cx, |app, cx| {
                                                app.open_session(None, window, cx)
                                            });
                                        }
                                    })),
                            )
                            .child(
                                Button::new("stop-session")
                                    .label("Stop")
                                    .small()
                                    .ghost()
                                    .tooltip("Stop the picked child agent. It ends with what it has, and its session stays")
                                    .disabled(!can_stop)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        if let (Some(id), Some(app)) =
                                            (this.session_selection.clone(), this.app.upgrade())
                                        {
                                            app.update(cx, |app, cx| app.stop_child(&id, cx));
                                        }
                                    })),
                            )
                            .child(
                                Button::new("close-session")
                                    .label("Close")
                                    .small()
                                    .ghost()
                                    .tooltip("Close the picked session's tab and let go of the session. It stays in the case")
                                    .disabled(!can_close)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let (Some(id), Some(app)) =
                                            (this.session_selection.clone(), this.app.upgrade())
                                        {
                                            app.update(cx, |app, cx| {
                                                app.close_session(&id, window, cx)
                                            });
                                        }
                                    })),
                            )
                            .child(
                                Button::new("open-session")
                                    .label("Open")
                                    .small()
                                    .tooltip("Show the picked session's transcript in a tab (double-click does the same)")
                                    .disabled(!can_open)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let (Some(id), Some(app)) =
                                            (this.session_selection.clone(), this.app.upgrade())
                                        {
                                            app.update(cx, |app, cx| {
                                                app.open_session(Some(id), window, cx)
                                            });
                                        }
                                    })),
                            ),
                    );
        // Two panes with a divider between them that is dragged: the case's sessions above, the
        // active session's tree below. Each scrolls in the room it has.
        div().size_full().child(
            v_resizable("tree-split")
                .child(
                    resizable_panel()
                        .size(px(260.))
                        .size_range(px(90.)..px(4000.))
                        .child(
                            v_flex()
                                .size_full()
                                .gap_2()
                                .child(header)
                                .child(
                                    div()
                                        .id("session-list")
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scrollbar()
                                        .child(session_list),
                                ),
                        ),
                )
                .child(
                    resizable_panel()
                        .size_range(px(90.)..px(4000.))
                        .child(
                            v_flex()
                                .size_full()
                                .gap_2()
                                .pt_2()
                                .child(
                                    h_flex().px_2().child(
                                        Label::new(format!("Tree of session {active_id}"))
                                            .text_color(theme.muted_foreground),
                                    ),
                                )
                                .child(
                                    h_flex().justify_end().px_2().child(
                                        Button::new("switch-branch")
                                            .label("Continue from here")
                                            .small()
                                            .tooltip(
                                                "Make the selected reply the tip: the next prompt branches from it",
                                            )
                                            .disabled(!can_switch)
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                if let (Some(id), Some(app)) =
                                                    (this.selection, this.app.upgrade())
                                                {
                                                    app.update(cx, |app, cx| app.switch_branch(id, cx));
                                                }
                                            })),
                                    ),
                                )
                                .child(
                                    div()
                                        .id("scroll-panel")
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scrollbar()
                                        .child(list),
                                ),
                        ),
                ),
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
}

impl Panel for TreePanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(
            IconName::GitBranch,
            "Session Tree",
            closer(&self.app, cx.entity()),
        )
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let app = self.app.clone();
        Some(vec![
            Button::new("sessions-refresh")
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("Read the case's sessions again")
                .on_click(move |_, _window, cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |app, cx| app.refresh_sessions(cx));
                    }
                }),
            close_button(&self.app, cx.entity()),
        ])
    }
}

// ---------------------------------------------------------------------------
// What the side panels share (the Toolbox is `crate::toolbox`)
// ---------------------------------------------------------------------------

/// The toolbar's refresh affordance, shared by every side panel that fetches
/// through the CLI — one look for "fetch this again", disabled while a fetch
/// is already in flight so a second click cannot race the first.
pub(crate) fn refresh_button(
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

/// The close button every panel ends its toolbar with: takes this panel out of the dock. It
/// is brought back from the Panels menu or the palette.
pub(crate) fn close_button<P: gpui_kit::base::dock::Panel>(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    panel: gpui_kit::Entity<P>,
) -> Button {
    let app = app.clone();
    Button::new("close-panel")
        .icon(IconName::X)
        .ghost()
        .small()
        .tooltip("Close this panel — reopen it from the Panels menu")
        .on_click(move |_, window, cx| {
            if let Some(app) = app.upgrade() {
                let panel = panel.clone();
                app.update(cx, |app, cx| app.close_panel(panel, window, cx));
            }
        })
}

/// A search box that redraws its panel as it is typed in.
pub(crate) fn search_box<T: 'static>(
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
        if let InputEvent::Change = event {
            cx.notify();
        }
    })
    .detach();
    input
}

/// The activation scope line above the tools and toolsets: whether toggles apply to this
/// session only or to the machine, with the two ways to change that (ADR-0042).
pub(crate) fn scope_header(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    scope: Option<&str>,
    theme: &Theme,
) -> impl IntoElement {
    let session = scope == Some("session");
    let (a, b) = (app.clone(), app.clone());
    h_flex()
        .px_2()
        .pt_2()
        .gap_2()
        .items_center()
        .child(crate::hints::tip(
            div().id("activation-scope").text_color(theme.muted_foreground).text_size(theme.font_size * 0.8).child(if session { "this session" } else { "machine default" }),
            if session {
                "This session has its own tool activation, which overrides the machine default. Make it the default for new sessions, or inherit the default again."
            } else {
                "Toggling a tool here starts a session-only override; the machine default is left alone."
            },
        ))
        .when(session, |row| {
            row.child(Button::new("act-default").label("make default").small().on_click(move |_, _w, cx| {
                if let Some(app) = a.upgrade() {
                    app.update(cx, |app, cx| app.make_activation_default(cx));
                }
            }))
            .child(Button::new("act-inherit").label("inherit").small().on_click(move |_, _w, cx| {
                if let Some(app) = b.upgrade() {
                    app.update(cx, |app, cx| app.inherit_activation(cx));
                }
            }))
        })
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
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(IconName::Server, "Services", closer(&self.app, cx.entity()))
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
        // No `--refresh`: the CLI's own TTLs decide freshness (SPEC.md
        // §5, ADR-0014) — the button re-asks, it does not force a probe.
        Some(vec![
            refresh_button("refresh-services", loading, move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| app.refresh_services(false, cx));
                }
            }),
            close_button(&self.app, cx.entity()),
        ])
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
            .overflow_y_scrollbar();
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
                    // not exist (SPEC.md §6, the TUI panel's same rule).
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
// What the panes and the settings share of the context budget (ADR-0037, ADR-0038)
// ---------------------------------------------------------------------------

/// A small "this session" / "default" badge — the scope of a setting, so the cascade is
/// never the confusing kind of magic (ADR-0038).
pub(crate) fn origin_badge(
    origin: reactor_context::settings::Origin,
    theme: &Theme,
) -> impl IntoElement {
    use reactor_context::settings::Origin;
    let (text, colour) = match origin {
        Origin::Session => ("this session", theme.accent),
        Origin::Default => ("default", theme.muted_foreground),
    };
    div()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(colour)
        .text_color(colour)
        .text_size(theme.font_size * 0.7)
        .child(text)
}

/// 12 400 as `12k`, 1 240 as `1.2k`: sizes in the usage bar are estimates.
pub(crate) fn tokens_short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{}k", n / 1000),
    }
}

/// How often an idle-or-not console checks its session for new output.
/// Matches `PUMP_TICK` in spirit — a console has nothing to synchronize with
/// the agent-event pump, so it keeps its own timer rather than borrowing one.
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(""));
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
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(
            IconName::SquareTerminal,
            self.command.as_deref().unwrap_or("Console"),
            closer(&self.app, cx.entity()),
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
            close_button(&self.app, cx.entity()),
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
        let ui = self
            .app
            .upgrade()
            .map(|a| a.read(cx).ui.clone())
            .unwrap_or_default();
        let (console_family, console_size) = ui.text(Slot::Console, &theme);
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
                        .font_family(console_family.clone())
                        .text_size(console_size);
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
                        .font_family(console_family.clone())
                        .text_size(console_size)
                        .child(
                            Label::new("$")
                                .font_family(console_family.clone())
                                .text_size(console_size)
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
                                .font_family(console_family.clone())
                                .text_size(console_size)
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
