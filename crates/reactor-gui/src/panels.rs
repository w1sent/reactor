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
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::component::marker::{Marker, MarkerContent, MarkerIcon};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Window, div, px,
};
use reactor_context::settings::MemorySettings;

use crate::app::ReactorApp;
use crate::backend::TreeRow;
use crate::session::{AgentPhase, ChatItem, Session};
use crate::settings::Slot;

/// One activation row — a leading dot for on/off, a name, and an optional muted
/// detail note; clicking anywhere on the row toggles it — shared by the Tools and
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
    badge: Option<AnyElement>,
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
                        .text_size(theme.mono_font_size)
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
                })
                .children(badge),
        )
        .cursor_pointer()
        .on_click(move |_, window, cx| on_toggle(window, cx))
}

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
    let label = if up {
        format!("MCP · ~{tokens} tok")
    } else {
        "MCP · server down".to_string()
    };
    let tip = if up {
        format!(
            "{} the MCP server `{server}`. While {} active, {} definition — about {tokens} tokens{} — is part of every request. A catalogue tool costs a line in the system prompt; this costs its whole schema, so activate only what the work needs.",
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
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(if up {
                theme.info
            } else {
                theme.muted_foreground
            })
            .text_size(theme.font_size * 0.75)
            .text_color(if up {
                theme.info
            } else {
                theme.muted_foreground
            })
            .child(SharedString::from(label)),
        SharedString::from(tip),
    )
    .into_any_element()
}

fn mcp_badge(tool: &reactor_client::ToolRow, theme: &Theme) -> Option<AnyElement> {
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
fn mcp_toolset_badge(
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
    /// How many items the scroller was last told about, so a render can
    /// tell an append from an in-place mutation (a streaming delta changes
    /// the last item's content without changing the count) and remeasure
    /// instead of appending.
    last_item_count: usize,
    /// Which "thinking" blocks (by transcript index) the user has expanded.
    /// Presentation state, not session data — it lives here rather than on
    /// `ChatItem` (SPEC.md §4.4) — and indices are stable to key it by:
    /// the transcript only ever appends. Fixes "thinking is not
    /// collapsible": the trigger existed but had no click handler and always
    /// passed `open(false)`, so it never actually did anything.
    expanded_thinking: std::collections::HashSet<usize>,
    /// The message whose copy button was just pressed (it shows a tick for a moment).
    copied: Option<usize>,
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
            last_item_count: 0,
            expanded_thinking: std::collections::HashSet::new(),
            copied: None,
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

/// The background of a whole entry: what you said is a shade lighter than the window, what the
/// agent produced is the window's own. Errors bring their own colour.
fn entry_background(item: &ChatItem, theme: &gpui_kit::component::Theme) -> Option<gpui_kit::Hsla> {
    match item {
        // An absolute lift: `lighten` scales, and a near-black window has nothing to scale.
        ChatItem::User { .. } => Some(gpui_kit::Hsla {
            l: (theme.background.l + 0.04).min(1.0),
            ..theme.background
        }),
        ChatItem::Error { .. } => None,
        _ => Some(theme.background),
    }
}

/// What the transcript needs to make file references in the assistant's text clickable.
#[derive(Clone)]
struct FileLinks {
    cwd: std::path::PathBuf,
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

// One entry's look is one function of everything it draws; a context struct would only rename
// these arguments.
#[allow(clippy::too_many_arguments)]
fn render_item(
    index: usize,
    item: &ChatItem,
    theme: &gpui_kit::component::Theme,
    tools: &ToolStyle,
    thinking_expanded: bool,
    toggle_thinking: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    restore: impl Fn(u64, &mut Window, &mut App) + Clone + 'static,
    detach: impl Fn(u64, &mut Window, &mut App) + Clone + 'static,
    links: &FileLinks,
) -> gpui_kit::Div {
    match item {
        // Who said it is the entry's background (`entry_background`), not a label.
        ChatItem::User { text } => div().px_2().child(text.clone()),
        ChatItem::AssistantText { text, streaming } => {
            let streaming = *streaming;
            let linked = crate::files::refs::linkify(text, &|path| {
                crate::files::refs::exists(&links.cwd, path)
            });
            let open = links.open.clone();
            // The text as the model wrote it — Markdown and all — for the clipboard.
            let (copy, raw) = (links.copy.clone(), text.clone());
            let copied = links.copied == Some(index);
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
                            .on_click(move |_, window, cx| copy(index, raw.clone(), window, cx)),
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
                .when(streaming, |el| el.opacity(0.85))
        }
        ChatItem::Thinking { text, streaming } => {
            let chevron = if thinking_expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            v_flex().px_2().child(
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
                                    .child(crate::indicator::shimmer("thinking…", theme.foreground))
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
            )
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
            // A finished passage search is shown as a list, not as the tagged text the model got.
            let docs = (name == "docs_search" && *done && !*is_error)
                .then(|| reactor_agent::tools::docs::parse_result(output))
                .flatten();
            // On a character boundary: tool output is arbitrary text and `…` and friends are
            // not one byte. (The whole output is in the session, and `history_read` gets it.)
            let output = if output.len() > tools.max_chars {
                let mut end = tools.max_chars;
                while !output.is_char_boundary(end) {
                    end -= 1;
                }
                format!(
                    "{}\n… truncated (the whole output is in the session log)",
                    &output[..end]
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
                        .child(Label::new(name.clone()).text_color(if *is_error {
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
                        .font_family(tools.family.clone())
                        .text_size(tools.size)
                        // Selectable, and whole: what is copied is the command as it ran.
                        .child(
                            SelectableText::new(("tool-command", index), command_text(args))
                                .document_order(index as u64 * 10),
                        ),
                )
                .children(docs.as_ref().map(|parsed| {
                    let query = args
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    docs_result(index, parsed, query, theme, tools, links)
                }))
                .when(!output.is_empty() && docs.is_none(), |el| {
                    el.child(
                        div()
                            .px_2()
                            .text_color(if *is_error {
                                theme.danger
                            } else {
                                theme.foreground
                            })
                            .font_family(tools.family.clone())
                            .text_size(tools.size)
                            .child(
                                SelectableText::new(("tool-output", index), output)
                                    .document_order(index as u64 * 10 + 1)
                                    .text_style(TextStyleRefinement {
                                        color: Some(if *is_error {
                                            theme.danger
                                        } else {
                                            theme.foreground
                                        }),
                                        ..Default::default()
                                    }),
                            ),
                    )
                })
                .when(*is_error, |el| el.border_l_2().border_color(colour))
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
        } => {
            let entry = *entry;
            let active = *active;
            v_flex()
                .px_2()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.tokens.title_bar)
                        .child(Label::new(if active { "context reduced" } else { "reduction undone" }).text_color(theme.accent))
                        .child(
                            Label::new(format!("{mode} · {trigger} · {covers} entries · ~{before} → ~{after} tokens"))
                                .text_color(theme.muted_foreground)
                                .text_size(theme.font_size * 0.85),
                        )
                        .when(active, |el| {
                            el.child(
                                Button::new(("undo-reduction", index))
                                    .label("undo")
                                    .small()
                                    .tooltip("Bring the originals back into the model's context")
                                    .on_click(move |_, window, cx| restore(entry, window, cx)),
                            )
                        }),
                )
                .when_some(summary.clone().filter(|_| active), |el, summary| {
                    el.child(
                        div().px_2().text_color(theme.muted_foreground).text_size(theme.font_size * 0.85).child(summary),
                    )
                })
        }
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
            v_flex()
                .px_2()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .child(Label::new(title).text_color(theme.accent))
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
            div()
                .px_2()
                .text_color(theme.muted_foreground)
                .text_size(theme.font_size * 0.8)
                .child(crate::session::stats_label(
                    date,
                    time,
                    *duration_ms,
                    *tokens,
                    *stream_ms,
                    &today,
                ))
        }
        ChatItem::Error { message } => v_flex().px_2().child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.danger.opacity(0.2))
                .text_color(theme.danger)
                .child(message.clone()),
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

/// The command a tool call ran, as text: a shell command as typed, a string as it is, other
/// arguments as compact JSON.
fn command_text(args: &Value) -> String {
    match args {
        Value::String(s) => s.clone(),
        Value::Object(o) => match o.get("command").and_then(Value::as_str) {
            Some(command) => command.to_owned(),
            None => serde_json::to_string(args).unwrap_or_default(),
        },
        _ => String::new(),
    }
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
            family: tool_family,
            size: tool_size,
            max_chars: ui.tool_output_chars,
        };
        let (items, hidden, phase, scheduled, last_error, run) = {
            let session = self.session.read(cx);
            (
                session.items.clone(),
                session.hidden.clone(),
                session.phase,
                session.follow_up.clone(),
                session.last_error.clone(),
                session
                    .run
                    .as_ref()
                    .map(|r| (r.elapsed(std::time::Instant::now()), r.tokens())),
            )
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
            let (open_thinking, dim_reduced) = (ui.expand_thinking, ui.dim_reduced);
            let tool_style = tool_style.clone();
            let weak_self = self.weak_self.clone();
            let weak_app = self.app.clone();
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
            let links = FileLinks {
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
                        &render_theme,
                        &tool_style,
                        // The default flips on the choice: a click toggles either way.
                        expanded_thinking.contains(&index) != open_thinking,
                        toggle_thinking.clone(),
                        restore.clone(),
                        detach.clone(),
                        &links,
                    );
                    let row = match entry_background(&items[index], &render_theme) {
                        Some(background) => row.bg(background).px_2().py_2(),
                        None => row.px_2().py_1(),
                    };
                    // What a reduction has taken out of the model's context stays on screen,
                    // dimmed: the log keeps everything, and so does the transcript.
                    if dim_reduced && hidden.contains(&index) {
                        row.opacity(0.45)
                    } else {
                        row
                    }
                },
            )
            // Entries touch: each has its own background, and the gap between rows is a hair.
            .with_row_style(gpui_kit::StyleRefinement::default().px_0().pb_0p5())
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
            .map(|(i, text)| {
                let first = text.lines().next().unwrap_or("");
                let shown: String = if first.chars().count() > 90 { first.chars().take(90).chain(std::iter::once('…')).collect() } else { first.to_string() };
                crate::hints::tip(
                    div().id(("scheduled", i)).px_3().py_1().child(
                        Marker::new()
                            .id(("scheduled-marker", i))
                            .text_color(theme.muted_foreground)
                            .icon(MarkerIcon::new().child(Icon::new(IconName::Clock).small()))
                            .content(MarkerContent::new().text(format!("scheduled: {shown}"))),
                    ),
                    "Runs right after the current prompt. Press ↑ in the empty prompt to bring it back and change it.",
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

        // What this session may draw on from the rest of its case (ADR-0047).
        let (history_on, graph_on, docs_on) = self
            .app
            .upgrade()
            .map(|a| a.read(cx).memory())
            .unwrap_or((true, true, true));
        let switch = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      tip: &'static str,
                      change: fn(bool) -> MemorySettings| {
            let app = self.app.clone();
            Switch::new(id)
                .small()
                .checked(on)
                .label(label)
                .tooltip(tip)
                .on_click(move |next, _window, cx| {
                    let next = *next;
                    let _ = app.update(cx, |app, cx| app.set_memory(change(next), cx));
                })
        };
        let memory_switches = h_flex()
            .gap_4()
            .items_center()
            .text_size(cx.theme().font_size * 0.85)
            .text_color(cx.theme().muted_foreground)
            .child(switch(
                "memory-history",
                "Other sessions",
                history_on,
                "On: the agent may search and read this case's other sessions. Off: it sees only this session.",
                |on| MemorySettings {
                    session_history: Some(on),
                    ..Default::default()
                },
            ))
            .child(switch(
                "memory-graph",
                "Case graph",
                graph_on,
                "On: the agent reads and writes the case's graph. Off: this session is detached from it and leaves the case's results untouched.",
                |on| MemorySettings {
                    case_graph: Some(on),
                    ..Default::default()
                },
            ))
            .child(switch(
                "memory-documents",
                "Documents",
                docs_on,
                "On: the agent may search the case's imported documents (docs_search). Off: the tool is withdrawn from this session; you can still search them yourself in the Documents panel.",
                |on| MemorySettings {
                    documents: Some(on),
                    ..Default::default()
                },
            ));

        let composer = v_flex()
            .key_context(if slash_open { "PromptComposer SlashPopup" } else { "PromptComposer" })
            .border_t_1()
            .border_color(cx.theme().border)
            .p_2()
            .gap_2()
            .children(popup)
            .child(memory_switches)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().font_family(prompt_family).text_size(prompt_size).child(Textarea::new(&self.composer)))
                    .child(
                        Button::new("send")
                            .tooltip(if phase == AgentPhase::Idle {
                                "Enter sends · Shift+Enter starts a new line · / lists commands · Ctrl+P opens the palette"
                            } else {
                                "The agent is working: Enter schedules this to run right after, and ↑ in the empty prompt brings it back"
                            })
                            .primary()
                            .label(if phase == AgentPhase::Idle {
                                "Send"
                            } else {
                                "Schedule"
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
                    ),
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
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .font_family(body_family)
                    .text_size(body_size)
                    .child(div().flex_1().min_h_0().child(transcript))
                    .children(running)
                    .children(scheduled_markers),
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
                "Click to pick it; double-click to open its transcript"
            };
            session_list = session_list.child(crate::hints::tip(
                v_flex()
                    .id(SharedString::from(format!("session-{}", s.id)))
                    .relative()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
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
                        let mut tags: Vec<&str> = Vec::new();
                        if s.current {
                            tags.push("active");
                        } else if s.open_here {
                            tags.push("open");
                        }
                        if is_working {
                            tags.push("working");
                        }
                        if s.open_elsewhere {
                            tags.push("open elsewhere");
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
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
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
                    ),
            )
            .child(
                div()
                    .id("session-list")
                    .max_h(px(240.))
                    .overflow_y_scrollbar()
                    .child(session_list),
            )
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
                            if let (Some(id), Some(app)) = (this.selection, this.app.upgrade()) {
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
// ToolsPanel / ToolsetsPanel — right, top tab group: the catalogue and its
// toolsets, from the `reactor` CLI (SPEC.md §5). Two tabs rather than
// one merged scroll list — the redesign's fix for keeping tools and
// toolsets apart — using the dock's own tab-group primitive
// (`DockLayout::tabs().panel_view(a).panel_view(b)`, wired in `app.rs`)
// instead of a bespoke in-panel tab widget. Enable/disable runs the CLI and
// refreshes both panels, since one `reactor` round trip answers both
// (SPEC.md §5, ADR-0014).
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

/// The close button every panel ends its toolbar with: takes this panel out of the dock. It
/// is brought back from the Layout menu's Panels submenu or the palette.
pub(crate) fn close_button<P: gpui_kit::base::dock::Panel>(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    panel: gpui_kit::Entity<P>,
) -> Button {
    let app = app.clone();
    Button::new("close-panel")
        .icon(IconName::X)
        .ghost()
        .small()
        .tooltip("Close this panel — reopen it from Layout › Panels")
        .on_click(move |_, window, cx| {
            if let Some(app) = app.upgrade() {
                let panel = panel.clone();
                app.update(cx, |app, cx| app.close_panel(panel, window, cx));
            }
        })
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
    /// What is typed in the search box: the list is cut to what matches.
    query: Entity<InputState>,
}

/// A search box that redraws its panel as it is typed in.
fn search_box<T: 'static>(
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

impl ToolsPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
            show_uninstalled: false,
            query: search_box("Search tools", window, cx),
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
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(IconName::Wrench, "Tools", closer(&self.app, cx.entity()))
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
        let close_app = self.app.clone();
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
            close_button(&close_app, cx.entity()),
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
            .overflow_y_scrollbar();
        let query = self.query.read(cx).value().to_string();
        let searching = !query.trim().is_empty();
        if let Some(payload) = &tools {
            let mut hidden = 0usize;
            let mut found = 0usize;
            for tool in &payload.tools {
                let installed = tool.status == "present";
                // Searching looks through everything the catalogue knows: how else would one
                // find a tool to install. Without a search, what is not here is hidden.
                if searching {
                    let tags = tool.tags.join(" ");
                    if !crate::quick_filter::matches(
                        &query,
                        &[&tool.id, &tool.name, &tool.desc, &tags],
                    ) {
                        continue;
                    }
                } else if !installed && tool.mcp.is_none() && !self.show_uninstalled {
                    hidden += 1;
                    continue;
                }
                found += 1;
                let tool_id = tool.id.clone();
                let tool_active = tool.active;
                let weak_app = self.app.clone();
                // An MCP tool is not installed or not: its server answers or does not, and
                // it is listed either way, tagged with what it costs.
                if installed || tool.mcp.is_some() {
                    list = list.child(activation_row(
                        format!("tool-{tool_id}"),
                        tool_active,
                        tool.id.clone(),
                        None,
                        mcp_badge(tool, &theme),
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
            } else if searching && found == 0 {
                list = list.child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child(format!("no tool matches `{}`", query.trim())),
                );
            } else if hidden > 0 {
                list = list.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_color(theme.muted_foreground)
                        .text_size(theme.font_size * 0.8)
                        .child(format!("{hidden} not installed")),
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

        // What the active MCP tools cost every request, in one line above the list.
        let (mcp_active, mcp_tokens) = tools
            .as_ref()
            .map(|p| {
                p.tools
                    .iter()
                    .filter(|t| t.active && t.status == "present")
                    .filter_map(|t| t.mcp.as_ref())
                    .fold((0usize, 0u64), |(n, tok), m| (n + 1, tok + m.tokens))
            })
            .unwrap_or_default();
        let scope = self
            .app
            .upgrade()
            .and_then(|a| a.read(cx).activation_scope.clone());
        v_flex()
            .size_full()
            .child(scope_header(&self.app, scope.as_deref(), &theme))
            .child(div().px_2().pt_2().child(Input::new(&self.query).small()))
            .when(mcp_active > 0, |el| {
                el.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_color(theme.muted_foreground)
                        .text_size(theme.font_size * 0.8)
                        .child(format!(
                            "{mcp_active} MCP tool{} active — about {mcp_tokens} tokens in every request",
                            if mcp_active == 1 { "" } else { "s" }
                        )),
                )
            })
            .child(list)
    }
}

/// The activation scope line above the tools and toolsets: whether toggles apply to this
/// session only or to the machine, with the two ways to change that (ADR-0042).
fn scope_header(
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

pub struct ToolsetsPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
    /// What is typed in the search box: the list is cut to what matches.
    query: Entity<InputState>,
}

impl ToolsetsPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
            query: search_box("Search toolsets", window, cx),
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
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(IconName::Layers, "Toolsets", closer(&self.app, cx.entity()))
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
        Some(vec![
            refresh_button("refresh-toolsets", loading, move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| app.refresh_catalogue(cx));
                }
            }),
            close_button(&self.app, cx.entity()),
        ])
    }
}

impl Render for ToolsetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (toolsets, tools, loading) = match self.app.upgrade() {
            Some(app) => {
                let read = app.read(cx);
                (
                    read.toolsets.clone(),
                    read.catalogue.clone(),
                    read.catalogue_loading,
                )
            }
            None => (None, None, false),
        };

        let mut list = v_flex()
            .id("toolsets-list")
            .p_2()
            .gap_1()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();
        let query = self.query.read(cx).value().to_string();
        let searching = !query.trim().is_empty();
        if let Some(payload) = &toolsets {
            let mut found = 0usize;
            for row in &payload.toolsets {
                // By name, description, or any tool in it.
                let members = row.tools.join(" ");
                if !crate::quick_filter::matches(&query, &[&row.id, &row.desc, &members]) {
                    continue;
                }
                found += 1;
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
                    mcp_toolset_badge(row, tools.as_ref(), &theme),
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
            } else if searching && found == 0 {
                list = list.child(
                    div()
                        .px_2()
                        .text_color(theme.muted_foreground)
                        .child(format!("no toolset matches `{}`", query.trim())),
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

        let scope = self
            .app
            .upgrade()
            .and_then(|a| a.read(cx).activation_scope.clone());
        v_flex()
            .size_full()
            .child(scope_header(&self.app, scope.as_deref(), &theme))
            .child(div().px_2().pt_2().child(Input::new(&self.query).small()))
            .child(list)
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
// ContextPanel — the context budget: what it costs, where each setting comes
// from, what a reduction would do, and undo (ADR-0037, ADR-0038)
// ---------------------------------------------------------------------------

/// A small "this session" / "default" badge — the scope of a setting, so the cascade is
/// never the confusing kind of magic (ADR-0038).
fn origin_badge(origin: reactor_context::settings::Origin, theme: &Theme) -> impl IntoElement {
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
fn tokens_short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{}k", n / 1000),
    }
}

/// A section of either tab, drawn the same way everywhere: a bordered card whose header row
/// (a title, what it is about, its switch or button) is set off from its content by a line.
fn group_box(header: impl IntoElement, content: impl IntoElement, theme: &Theme) -> Div {
    v_flex()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .child(header),
        )
        .child(div().p_3().child(content))
}

/// The usage bar, and under it the totals per part of the request; opened, a legend of every
/// kind. Colours are the theme's chart colours first ([`crate::inspect_ui::kind_colour`]); a
/// row of the legend shows that kind in the context window.
fn usage_block(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    view: &crate::backend::ContextView,
    open: bool,
    on_details: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    theme: &Theme,
) -> impl IntoElement {
    use reactor_agent::inspect::{Group, Kind};
    let hard = view.hard.max(1) as f32;
    let counted: u64 = view.usage.iter().map(|(_, t)| *t).sum();
    // The kinds are estimates of the same request `used` measures: if they come to more than
    // the window, they are drawn to scale rather than past the end of the bar.
    let scale = if counted as f32 > hard {
        hard / counted as f32
    } else {
        1.0
    };
    let trigger_at = (view.trigger as f32 / hard).min(1.0);
    let mut bar = h_flex()
        .id("context-bar")
        .relative()
        .h(px(12.))
        .gap(px(1.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme.secondary);
    if view.usage.is_empty() {
        // Nothing to take apart (yet): the whole, as before.
        let fraction = (view.used as f32 / hard).min(1.0);
        bar = bar.child(
            div()
                .h_full()
                .w(relative(fraction))
                .bg(if view.used > view.trigger {
                    theme.warning
                } else {
                    theme.accent
                }),
        );
    }
    for (kind, tokens) in &view.usage {
        let share = (*tokens as f32 / hard * scale).max(0.0);
        bar = bar.child(crate::hints::tip(
            div()
                .id(SharedString::from(format!("usage-{kind:?}")))
                .h_full()
                .w(relative(share))
                .flex_none()
                .bg(crate::inspect_ui::kind_colour(*kind, theme)),
            format!("{}: ~{} tokens", kind.label(), tokens),
        ));
    }
    let bar = div().relative().child(bar).child(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(2.))
            .left(relative(trigger_at))
            .bg(theme.foreground.opacity(0.6)),
    );

    let group_total = |group: Group| -> u64 {
        view.usage
            .iter()
            .filter(|(k, _)| k.group() == group)
            .map(|(_, t)| *t)
            .sum()
    };
    let mut totals = h_flex()
        .justify_between()
        .items_center()
        .text_size(theme.font_size * 0.85);
    for group in [Group::System, Group::Tools, Group::Messages] {
        totals = totals.child(
            h_flex()
                .gap_1()
                .child(div().child(match group {
                    Group::System => "System",
                    Group::Tools => "Tools",
                    Group::Messages => "Messages",
                }))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(tokens_short(group_total(group))),
                ),
        );
    }
    totals = totals.child(
        Button::new("usage-details")
            .label("Details")
            .icon(if open {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            })
            .ghost()
            .small()
            .tooltip("What the request is made of, kind by kind")
            .on_click(on_details),
    );

    let mut block = v_flex()
        .gap_2()
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(Label::new("In the next request"))
                .child(
                    Label::new(format!(
                        "~{} of {} · {}%",
                        tokens_short(view.used),
                        tokens_short(view.hard),
                        view.percent()
                    ))
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.85),
                ),
        )
        .child(bar)
        .child(totals);
    if open {
        let mut legend = v_flex().gap_2();
        for group in [Group::System, Group::Tools, Group::Messages] {
            let mut rows = v_flex().gap_0p5().child(
                h_flex()
                    .justify_between()
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.75)
                    .child(group.label().to_uppercase())
                    .child(tokens_short(group_total(group))),
            );
            let mut any = false;
            for (kind, tokens) in view.usage.iter().filter(|(k, _)| k.group() == group) {
                any = true;
                let (kind, weak) = (*kind, app.clone());
                rows = rows.child(
                    h_flex()
                        .id(SharedString::from(format!("usage-row-{kind:?}")))
                        .gap_2()
                        .items_center()
                        .px_1()
                        .h(px(22.))
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |app, cx| app.show_kind(kind, window, cx));
                        })
                        .child(
                            div()
                                .size(px(10.))
                                .rounded_sm()
                                .flex_none()
                                .bg(crate::inspect_ui::kind_colour(kind, theme)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme.font_size * 0.9)
                                .child(kind.label()),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .text_size(theme.font_size * 0.85)
                                .child(tokens_short(*tokens)),
                        ),
                );
            }
            if any {
                legend = legend.child(rows);
            }
        }
        block = block.child(legend).child(
            div()
                .text_color(theme.muted_foreground)
                .text_size(theme.font_size * 0.75)
                .child("A row opens that kind in the context window."),
        );
    }
    let _ = Kind::ALL;
    block
}

/// The Goal section of the Plan tab: the goal, the guidelines, the steps the agent keeps, the
/// manifest switch, and the buttons for the editor and the Frame.
fn goal_section(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    plan: &crate::plan_ui::PlanView,
    theme: &Theme,
) -> impl IntoElement {
    let muted = theme.muted_foreground;
    let (a_switch, a_edit, a_frame) = (app.clone(), app.clone(), app.clone());
    let header = h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            h_flex().gap_1().items_center().child(Label::new("Goal")).child(
                crate::hints::info_button(
                    "goal-info",
                    "The goal and the guidelines belong to the case: every session of it is told them. The agent keeps its steps against the goal with update_steps, and they are this session's. The Manifest switch pauses all of it: nothing is sent and update_steps goes quiet, and what is set is kept. Frame shows what is sent. /goal, /guidelines, /manifest and /frame do the same.",
                ),
            ),
        )
        .child(
            Switch::new("manifest-switch")
                .small()
                .checked(plan.manifest_on)
                .label("Manifest")
                .tooltip("Send the goal, guidelines and steps to the model, and let it keep steps")
                .on_click(move |next, _w, cx| {
                    let next = *next;
                    let _ = a_switch.update(cx, |app, cx| app.manifest_set(next, cx));
                }),
        );
    let mut content = v_flex().gap_2();
    content = content.child(match &plan.goal {
        Some(g) => div()
            .child(SharedString::from(g.clone()))
            .into_any_element(),
        None => div()
            .text_color(muted)
            .child("No goal yet. The agent keeps no steps until there is one.")
            .into_any_element(),
    });
    if let Some(g) = &plan.guidelines {
        content = content.child(
            div()
                .id("guidelines-line")
                .max_h(px(34.))
                .overflow_hidden()
                .text_color(muted)
                .text_size(theme.font_size * 0.85)
                .child(SharedString::from(format!("Guidelines: {g}"))),
        );
    }
    content = content.child(
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("goal-edit")
                    .label("Edit…")
                    .small()
                    .tooltip("Write the goal and the guidelines")
                    .on_click(move |_, window, cx| {
                        let _ = a_edit.update(cx, |app, cx| app.open_goal_editor(window, cx));
                    }),
            )
            .child(
                Button::new("goal-frame")
                    .label("Frame…")
                    .small()
                    .tooltip("What the model is told, and what is kept")
                    .on_click(move |_, window, cx| {
                        let _ = a_frame.update(cx, |app, cx| app.open_frame(window, cx));
                    }),
            ),
    );
    if !plan.steps.is_empty() {
        let mut steps = v_flex().gap_0p5().child(
            div()
                .text_color(muted)
                .text_size(theme.font_size * 0.8)
                .child(format!("Steps {}/{}", plan.steps.len(), plan.soft_limit)),
        );
        for (status, summary) in &plan.steps {
            let lower = status.to_lowercase();
            let (glyph, colour) = if lower.contains("done") || lower.contains("complete") {
                ("●", theme.success)
            } else if lower.contains("progress")
                || lower.contains("working")
                || lower.contains("active")
            {
                ("◐", theme.warning)
            } else {
                ("○", muted)
            };
            steps = steps.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_size(theme.font_size * 0.9)
                    .child(div().flex_none().text_color(colour).child(glyph))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(summary.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(muted)
                            .text_size(theme.font_size * 0.8)
                            .child(SharedString::from(status.clone())),
                    ),
            );
        }
        content = content.child(steps);
    }
    group_box(header, content, theme)
}

/// The Reporting section of the Plan tab: the switch and the level. The folder, thresholds and
/// template are in Settings.
fn reporting_section(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    plan: &crate::plan_ui::PlanView,
    theme: &Theme,
) -> impl IntoElement {
    let muted = theme.muted_foreground;
    let (a_switch, a_level) = (app.clone(), app.clone());
    let on = plan.report_on;
    let header = h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            h_flex().gap_1().items_center().child(Label::new("Reporting")).child(
                crate::hints::info_button(
                    "reporting-info",
                    "Reporting makes the agent write its findings up as it goes, in a folder of the case. Level 0 adds only the instructions to the prompt; 1 also reminds the agent whenever the folder has not changed for a while; 2 reverts a turn that left it undocumented. Folder, thresholds and template are in Settings › Reporting. /report does the same.",
                ),
            ),
        )
        .child(
            Switch::new("reporting-switch")
                .small()
                .checked(on)
                .tooltip("Make the agent document its findings in the reporting folder")
                .on_click(move |next, _w, cx| {
                    let next = *next;
                    let _ = a_switch.update(cx, |app, cx| app.report_set(next, cx));
                }),
        );
    let level = TabBar::new("reporting-level")
        .segmented()
        .small()
        .selected_index(plan.report_level.min(2) as usize)
        .children([
            Tab::new().label("0 prompt"),
            Tab::new().label("1 nag"),
            Tab::new().label("2 strict"),
        ])
        .on_click(move |ix: &usize, _w, cx| {
            // A level is only meant while reporting is on: setting one would switch it on.
            if on {
                let ix = *ix as u8;
                let _ = a_level.update(cx, |app, cx| app.report_level(ix, cx));
            }
        });
    group_box(
        header,
        v_flex()
            .gap_2()
            .child(div().when(!on, |d| d.opacity(0.5)).child(level))
            .child(
                div()
                    .text_color(muted)
                    .text_size(theme.font_size * 0.8)
                    .child(if on {
                        format!(
                            "folder {} · level {}",
                            plan.report_folder, plan.report_level
                        )
                    } else {
                        "off".to_string()
                    }),
            ),
        theme,
    )
}

/// The Scenario section of the Context panel: the scenarios to choose from and a Start button,
/// or, while one runs, its phases as a vertical stepper -- the current one marked, a click on
/// any other moving to it -- and an Abort button. Moving by hand is for between turns, so the
/// stepper is disabled while the agent works; the agent moves on by itself with
/// `reactor_phase_complete`, and the stepper follows.
fn scenario_section(
    app: &gpui_kit::WeakEntity<ReactorApp>,
    view: &crate::scenario_ui::View,
    pick: Option<String>,
    toolsets: Option<&reactor_client::ToolsetsPayload>,
    tools: Option<&reactor_client::ToolsPayload>,
    idle: bool,
    theme: &Theme,
) -> (AnyElement, AnyElement) {
    use crate::scenario_ui::Outcome;
    use gpui_kit::component::stepper::{Stepper, StepperItem};

    let title = h_flex()
        .gap_1()
        .items_center()
        .child(Label::new("Scenario"))
        .child(crate::hints::info_button(
            "scenario-info",
            "A scenario is a series of phases, each with a briefing for the agent: it is added to the system prompt while the phase is current, and the agent moves to the next phase with a tool call that carries a summary. Click a phase to go there yourself, forwards or back — only between turns; the briefing is then in effect for the next prompt, which starts the phase. After moving on by hand, say in that prompt what the phase you left concluded: it is recorded as its summary. A phase asks for a toolset; tools of it that are not installed are not offered to the agent. /reactor-scenario list | start <id> | status | next | goto <n> | stop does the same.",
        ));
    let Some(run) = &view.run else {
        let chosen = pick
            .filter(|p| view.available.contains(p))
            .or_else(|| view.available.first().cloned());
        let mut choices = h_flex().gap_1().flex_wrap();
        for id in &view.available {
            let (a, pid) = (app.clone(), id.clone());
            choices = choices.child(
                Button::new(SharedString::from(format!("scenario-pick-{id}")))
                    .label(id.clone())
                    .small()
                    .when(chosen.as_deref() == Some(id.as_str()), |b| b.primary())
                    .on_click(move |_, _w, cx| {
                        if let Some(app) = a.upgrade() {
                            app.update(cx, |app, cx| {
                                app.scenario_pick = Some(pid.clone());
                                cx.notify();
                            });
                        }
                    }),
            );
        }
        let a = app.clone();
        let header = h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(title)
            .child(
                Button::new("scenario-start")
                    .label("Start")
                    .small()
                    .primary()
                    .disabled(!idle || view.available.is_empty())
                    .tooltip("Start the chosen scenario: its first phase is briefed and the agent begins")
                    .on_click(move |_, _w, cx| {
                        if let Some(app) = a.upgrade() {
                            app.update(cx, |app, cx| app.scenario_start(cx));
                        }
                    }),
            );
        let content = if view.available.is_empty() {
            div()
                .text_color(theme.muted_foreground)
                .child("no scenarios found in prompts/scenarios")
                .into_any_element()
        } else {
            choices.into_any_element()
        };
        return (header.into_any_element(), content);
    };

    let a = app.clone();
    let header = h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .child(title)
        .child(
            Label::new(format!(
                "{} · phase {}/{}",
                run.id,
                run.current + 1,
                run.phases.len()
            ))
            .text_color(theme.muted_foreground)
            .text_size(theme.font_size * 0.85),
        )
        .child(
            Button::new("scenario-abort")
                .label("Abort")
                .small()
                .ghost()
                .tooltip("Stop the scenario: the agent is no longer briefed and phase_complete goes away. What it did stays")
                .on_click(move |_, _w, cx| {
                    if let Some(app) = a.upgrade() {
                        app.update(cx, |app, cx| app.scenario_abort(cx));
                    }
                }),
        );

    let small = theme.font_size * 0.8;
    let items = run.phases.iter().enumerate().map(|(i, phase)| {
        let missing = phase
            .toolset
            .as_deref()
            .map(|t| crate::scenario_ui::missing_tools(t, toolsets, tools))
            .unwrap_or_default();
        // A line under the title: what a finished phase concluded, else the toolset it asks for.
        let (line, line_colour) = match &phase.outcome {
            Outcome::Summary(t) => (t.clone(), theme.muted_foreground),
            Outcome::Skipped => ("skipped".to_string(), theme.muted_foreground),
            Outcome::Awaiting => (
                "say what it concluded in your next prompt".to_string(),
                theme.warning,
            ),
            Outcome::None => (
                match (&phase.toolset, missing.len()) {
                    (Some(t), 0) => format!("toolset {t}"),
                    (Some(t), n) => format!("toolset {t} · {n} not available"),
                    (None, _) => String::new(),
                },
                if missing.is_empty() {
                    theme.muted_foreground
                } else {
                    theme.warning
                },
            ),
        };
        let tip = if missing.is_empty() {
            line.clone()
        } else {
            format!(
                "{line}\nThe phase starts anyway; the agent is not told about: {}",
                missing.join(", ")
            )
        };
        StepperItem::new().child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .text_color(if i == run.current {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        })
                        .child(SharedString::from(phase.title.clone())),
                )
                .when(!line.is_empty(), |c| {
                    c.child(crate::hints::tip(
                        div()
                            .id(SharedString::from(format!("scenario-phase-{i}")))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(small)
                            .text_color(line_colour)
                            .child(SharedString::from(line)),
                        SharedString::from(tip),
                    ))
                }),
        )
    });
    let a = app.clone();
    let content = Stepper::new("scenario-steps")
        .vertical()
        .small()
        .selected_index(run.current)
        .disabled(!idle)
        .items(items)
        .on_click(move |ix, _w, cx| {
            if let Some(app) = a.upgrade() {
                let n = *ix + 1;
                app.update(cx, |app, cx| app.scenario_goto(n, cx));
            }
        });
    (header.into_any_element(), content.into_any_element())
}

/// The two tabs of the Context panel: the plan (goal, scenario, reporting) and the budget
/// (settings and reduction). The usage bar above them belongs to both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ContextTab {
    #[default]
    Plan,
    Budget,
}

pub struct ContextPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
    tab: ContextTab,
    /// The usage legend is open.
    details: bool,
}

impl ContextPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
            tab: ContextTab::default(),
            details: false,
        }
    }
}

impl Focusable for ContextPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for ContextPanel {}

impl gpui_kit::base::dock::Panel for ContextPanel {
    fn panel_name(&self) -> &'static str {
        "context"
    }
}

impl Panel for ContextPanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self
            .app
            .upgrade()
            .map(|a| a.read(cx).active_session_id())
            .unwrap_or_default();
        let short = crate::session_list::short_id(&id);
        panel_title_closing(
            IconName::Layers,
            &format!("Context · {short}"),
            closer(&self.app, cx.entity()),
        )
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![close_button(&self.app, cx.entity())])
    }
}

impl Render for ContextPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use reactor_agent::entry::Mode;
        use reactor_context::settings::{ContextSettings, Origin};

        let theme = cx.theme().clone();
        let (view, preview, busy, idle, scenario) = match self.app.upgrade() {
            Some(app) => {
                let a = app.read(cx);
                (
                    a.context.clone(),
                    a.preview.clone(),
                    a.context_busy,
                    a.session.read(cx).phase == AgentPhase::Idle,
                    Some((
                        a.scenario.clone(),
                        a.scenario_pick.clone(),
                        a.toolsets.clone(),
                        a.catalogue.clone(),
                    )),
                )
            }
            None => (None, None, false, false, None),
        };
        let plan = self
            .app
            .upgrade()
            .map(|a| a.read(cx).plan.clone())
            .unwrap_or_default();
        // The Plan tab and the Budget tab: what is in each is built below, one is shown.
        let mut plan_body = v_flex().gap_3();
        let mut body = v_flex().gap_3();
        let Some(view) = view else {
            return v_flex().size_full().child(
                v_flex()
                    .p_2()
                    .child(loading_row("measuring the context…", &theme)),
            );
        };

        // -- usage: one bar by kind, above the tabs --
        let details = self.details;
        let usage = usage_block(
            &self.app,
            &view,
            details,
            cx.listener(|this, _, _w, cx| {
                this.details = !this.details;
                cx.notify();
            }),
            &theme,
        );

        // -- the plan: goal, scenario, reporting --
        plan_body = plan_body.child(goal_section(&self.app, &plan, &theme));
        if let Some((scenario, pick, toolsets, tools)) = scenario {
            let (header, content) = scenario_section(
                &self.app,
                &scenario,
                pick,
                toolsets.as_ref(),
                tools.as_ref(),
                idle,
                &theme,
            );
            plan_body = plan_body.child(group_box(header, content, &theme));
        }
        plan_body = plan_body.child(reporting_section(&self.app, &plan, &theme));

        // -- settings --
        let origin_of = |key: &str| {
            view.origins
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, o)| *o)
                .unwrap_or(Origin::Default)
        };
        let setting = |label: &'static str, value: String, key: &str| {
            h_flex()
                .justify_between()
                .items_center()
                .child(Label::new(label).text_color(theme.muted_foreground))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Label::new(value).font_family(theme.mono_font_family.clone()))
                        .child(origin_badge(origin_of(key), &theme)),
                )
        };
        let set_mode = {
            let app = self.app.clone();
            move |mode: &'static str, cx: &mut App| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.set_context(
                            ContextSettings {
                                mode: Some(mode.to_string()),
                                ..Default::default()
                            },
                            cx,
                        )
                    });
                }
            }
        };
        let modes = ["auto", "fade", "compact"];
        let mode_tabs = TabBar::new("context-mode")
            .segmented()
            .small()
            .selected_index(modes.iter().position(|m| *m == view.mode).unwrap_or(0))
            .children(modes.iter().map(|m| Tab::new().label(*m)))
            .on_click(move |ix: &usize, _w, cx| {
                if let Some(mode) = modes.get(*ix) {
                    set_mode(mode, cx);
                }
            });
        let settings_header = h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .child(Label::new("Settings"))
            .child(crate::hints::info_button(
                "context-settings-info",
                "Change one with /context <mode|window|reserve|pct|keep|summarizer> <value>, or use the mode buttons. \"this session\" values apply to this session only; \"default\" ones come from settings.json.",
            ));
        let (app_a, app_b) = (self.app.clone(), self.app.clone());
        let settings_content = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(mode_tabs))
                    .child(origin_badge(origin_of("mode"), &theme)),
            )
            .child(setting("window", view.window.to_string(), "window"))
            .child(setting("reserve", view.reserve.to_string(), "reserve"))
            .child(setting(
                "reduce at",
                format!("{:.0}%", view.pct * 100.0),
                "pct",
            ))
            .child(setting(
                "keep",
                format!("{:.0}%", view.keep * 100.0),
                "keep",
            ))
            .child(setting(
                "summarizer",
                view.summarizer
                    .clone()
                    .unwrap_or_else(|| "the session's model".into()),
                "summarizer",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("context-default")
                            .label("Make this the default")
                            .small()
                            .tooltip("Every new session starts with these settings")
                            .on_click(move |_, _window, cx| {
                                if let Some(app) = app_a.upgrade() {
                                    app.update(cx, |app, cx| app.make_context_default(cx));
                                }
                            }),
                    )
                    .child(
                        Button::new("context-inherit")
                            .label("Inherit the default")
                            .small()
                            .tooltip("Drop this session's own values")
                            .on_click(move |_, _window, cx| {
                                if let Some(app) = app_b.upgrade() {
                                    app.update(cx, |app, cx| app.inherit_context(cx));
                                }
                            }),
                    ),
            );
        body = body.child(group_box(settings_header, settings_content, &theme));

        // -- reduction: preview, do, undo, and what is in force --
        let action = |id: &'static str, label: &'static str, mode: Mode, reduce: bool| {
            let app = self.app.clone();
            Button::new(id)
                .label(label)
                .small()
                .disabled(busy || !idle)
                .on_click(move |_, _window, cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |app, cx| {
                            if reduce {
                                app.reduce_now(mode, cx)
                            } else {
                                app.preview_reduction(mode, cx)
                            }
                        });
                    }
                })
        };
        let reduce_header = h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(Label::new("Reduce"))
            .child({
                let app = self.app.clone();
                Button::new("context-inspect")
                    .label("Show the context window")
                    .small()
                    .tooltip(
                        "Everything the model would be sent next, and where each piece comes from",
                    )
                    .on_click(move |_, window, cx| {
                        if let Some(app) = app.upgrade() {
                            app.update(cx, |app, cx| app.toggle_inspect(window, cx));
                        }
                    })
            });
        let mut reduction = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(action(
                        "preview-compact",
                        "Preview: summarize",
                        Mode::Compact,
                        false,
                    ))
                    .child(action("preview-fade", "Preview: fade", Mode::Fade, false)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(action(
                        "reduce-compact",
                        "Summarize now",
                        Mode::Compact,
                        true,
                    ))
                    .child(action("reduce-fade", "Fade now", Mode::Fade, true)),
            );
        if busy {
            reduction = reduction.child(loading_row("working…", &theme));
        }
        if let Some(p) = &preview {
            reduction = reduction.child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(theme.secondary)
                    .child(Label::new(format!(
                        "{:?}: would reduce {} entries, keeping the newest",
                        p.mode, p.entries
                    )))
                    .child(
                        Label::new(format!(
                            "{} tool results (~{} tokens) and {} messages (~{} tokens)",
                            p.mechanical, p.mechanical_tokens, p.conceptual, p.conceptual_tokens
                        ))
                        .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new(format!(
                            "~{} → ~{} tokens{}",
                            p.before_tokens,
                            p.after_tokens,
                            if p.needs_model {
                                " · asks the model for a summary"
                            } else {
                                " · no model call"
                            }
                        ))
                        .text_color(theme.muted_foreground),
                    ),
            );
        }

        // What is in force, under the buttons that make and undo it.
        let mut in_force = v_flex()
            .gap_1()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("In force")
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.85),
            );
        if view.reductions.is_empty() {
            in_force = in_force.child(
                Label::new("none")
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.85),
            );
        }
        for r in &view.reductions {
            let (entry, app) = (r.entry, self.app.clone());
            in_force = in_force.child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new(format!(
                            "#{} {:?} · {} · {} entries · ~{} → ~{}",
                            r.entry,
                            r.mode,
                            crate::backend::trigger_label(r.trigger),
                            r.covers,
                            r.before_tokens,
                            r.after_tokens
                        ))
                        .text_size(theme.font_size * 0.85),
                    )
                    .child(
                        Button::new(("undo", r.entry as usize))
                            .label("undo")
                            .small()
                            .on_click(move |_, _window, cx| {
                                if let Some(app) = app.upgrade() {
                                    app.update(cx, |app, cx| app.restore_reduction(entry, cx));
                                }
                            }),
                    ),
            );
        }
        body = body.child(group_box(reduce_header, reduction.child(in_force), &theme));

        let tabs = TabBar::new("context-tabs")
            .selected_index(match self.tab {
                ContextTab::Plan => 0,
                ContextTab::Budget => 1,
            })
            .children([Tab::new().label("Plan"), Tab::new().label("Budget")])
            .on_click(cx.listener(|this, ix: &usize, _w, cx| {
                this.tab = if *ix == 0 {
                    ContextTab::Plan
                } else {
                    ContextTab::Budget
                };
                cx.notify();
            }));
        v_flex()
            .size_full()
            .child(div().flex_none().px_2().pt_2().child(usage))
            .child(div().flex_none().px_2().child(tabs))
            .child(
                div()
                    .id("context-body")
                    .p_2()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(match self.tab {
                        ContextTab::Plan => plan_body,
                        ContextTab::Budget => body,
                    }),
            )
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
