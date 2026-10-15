//! The Toolbox: the catalogue's tools and its toolsets in one panel.
//!
//! A tool can be in several toolsets (a toolset is a tag list that intersects, plus explicit
//! tools: ADR-0013), so the toolsets are not groups of the list: each tool is listed once, and
//! the toolsets are tags above it. A click on a tag toggles the toolset, as a click on a tool's
//! row toggles the tool; an active toolset has a dot. Resting the pointer on a tag brings its
//! tools forward in the list, which is how the overlap between toolsets is seen.
//!
//! Activation is derived: the active toolsets' tools, minus what is pinned off, plus what is
//! pinned on (ADR-0011). So a row says *why* its tool is on, and a second click on a pinned
//! tool drops the pin rather than adding another.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Window, div, px,
};

use crate::app::ReactorApp;
use crate::panels::{
    close_button, closer, install_row, loading_row, mcp_badge, mcp_toolset_badge,
    panel_title_closing, refresh_button, scope_header, search_box,
};

pub struct ToolboxPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
    /// Whether tools that are not installed on this machine are listed. Off by default: the
    /// catalogue is a menu of what REactor knows, and on a fresh machine most of it is not
    /// installed, which buries the tools there are behind rows that cannot be acted on.
    show_uninstalled: bool,
    /// What is typed in the search box: tools and toolsets are cut to what matches.
    query: Entity<InputState>,
    /// The toolset the pointer rests on: its tools are brought forward.
    hovered: Option<String>,
}

impl ToolboxPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            focus: cx.focus_handle(),
            show_uninstalled: false,
            query: search_box("Search tools and toolsets", window, cx),
            hovered: None,
        }
    }
}

impl Focusable for ToolboxPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for ToolboxPanel {}

impl gpui_kit::base::dock::Panel for ToolboxPanel {
    fn panel_name(&self) -> &'static str {
        "toolbox"
    }
}

impl Panel for ToolboxPanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(IconName::Wrench, "Toolbox", closer(&self.app, cx.entity()))
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
            refresh_button("refresh-toolbox", loading, move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.mcp_reconnect_failed(cx);
                        app.refresh_catalogue(cx);
                    });
                }
            }),
            close_button(&self.app, cx.entity()),
        ])
    }
}

/// Why a tool is on or off, for the end of its row: the active toolsets that include it, or
/// the person's own pin (the `bool`).
fn standing(
    tool: &reactor_client::ToolRow,
    toolsets: Option<&reactor_client::ToolsetsPayload>,
) -> Option<(String, bool)> {
    match tool.override_kind.as_deref() {
        Some("on") => return Some(("pinned on".to_string(), true)),
        Some("off") => return Some(("pinned off".to_string(), true)),
        _ => {}
    }
    if !tool.active {
        return None;
    }
    let via: Vec<&str> = toolsets?
        .toolsets
        .iter()
        // `all` is every tool: naming it beside a narrower toolset says nothing.
        .filter(|s| s.active && s.tools.iter().any(|t| t == &tool.id))
        .map(|s| s.id.as_str())
        .collect();
    let named: Vec<&str> = match via.iter().filter(|s| **s != "all").count() {
        0 => via,
        _ => via.into_iter().filter(|s| *s != "all").collect(),
    };
    (!named.is_empty()).then(|| (format!("via {}", named.join(", ")), false))
}

/// The toolsets a tool is in, for its tooltip.
fn memberships<'a>(
    tool: &reactor_client::ToolRow,
    toolsets: Option<&'a reactor_client::ToolsetsPayload>,
) -> Vec<&'a str> {
    toolsets
        .map(|p| {
            p.toolsets
                .iter()
                .filter(|s| s.id != "all" && s.tools.iter().any(|t| t == &tool.id))
                .map(|s| s.id.as_str())
                .collect()
        })
        .unwrap_or_default()
}

impl Render for ToolboxPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let (tools, toolsets, loading, scope) = match self.app.upgrade() {
            Some(app) => {
                let read = app.read(cx);
                (
                    read.catalogue.clone(),
                    read.toolsets.clone(),
                    read.catalogue_loading,
                    read.activation_scope.clone(),
                )
            }
            None => (None, None, false, None),
        };
        let query = self.query.read(cx).value().to_string();
        let searching = !query.trim().is_empty();

        // -- the toolsets, as tags --
        let mut tags = h_flex().px_2().pt_2().gap_1().flex_wrap();
        let mut any_tag = false;
        if let Some(payload) = &toolsets {
            for row in &payload.toolsets {
                // By name, description, or any tool in it.
                let members = row.tools.join(" ");
                if !crate::quick_filter::matches(&query, &[&row.id, &row.desc, &members]) {
                    continue;
                }
                any_tag = true;
                let installed = row
                    .tools
                    .iter()
                    .filter(|id| {
                        tools.as_ref().is_some_and(|p| {
                            p.tools
                                .iter()
                                .any(|t| &t.id == *id && t.status == "present")
                        })
                    })
                    .count();
                let count = row.tools.len();
                let tip = format!(
                    "{} · {count} tool{} · {installed} installed{}\n{}",
                    row.id,
                    if count == 1 { "" } else { "s" },
                    if row.desc.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", row.desc)
                    },
                    if row.active {
                        "Active. Click to turn it off."
                    } else {
                        "Not active. Click to activate."
                    },
                );
                let (id, active, weak_app) = (row.id.clone(), row.active, self.app.clone());
                let hover_id = row.id.clone();
                let hovered = self.hovered.as_deref() == Some(row.id.as_str());
                tags = tags.child(crate::hints::tip(
                    h_flex()
                        .id(SharedString::from(format!("toolset-{}", row.id)))
                        .h(px(22.))
                        .px_2()
                        .gap_1p5()
                        .items_center()
                        .rounded_full()
                        .border_1()
                        .border_color(if active {
                            theme.list_active_border
                        } else if hovered {
                            muted
                        } else {
                            theme.input
                        })
                        .when(active, |t| t.bg(theme.list_active))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                            let now = over.then(|| hover_id.clone());
                            // Leaving one tag for another arrives as enter-then-leave: only
                            // the tag that is hovered may clear it.
                            if *over || this.hovered.as_deref() == Some(hover_id.as_str()) {
                                this.hovered = now;
                                cx.notify();
                            }
                        }))
                        .on_click(move |_, _window, cx| {
                            if let Some(app) = weak_app.upgrade() {
                                app.update(cx, |app, cx| app.toggle_toolset(&id, !active, cx));
                            }
                        })
                        .when(active, |t| {
                            t.child(
                                div()
                                    .size(px(6.))
                                    .rounded_full()
                                    .flex_none()
                                    .bg(theme.accent),
                            )
                        })
                        .child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.font_size * 0.8)
                                .text_color(if active { theme.foreground } else { muted })
                                .child(SharedString::from(row.id.clone())),
                        )
                        .children(mcp_toolset_badge(row, tools.as_ref(), &theme)),
                    SharedString::from(tip),
                ));
            }
        }
        // The tools of the toolset the pointer rests on.
        let brought_forward: Option<Vec<String>> = self.hovered.as_ref().and_then(|id| {
            toolsets
                .as_ref()?
                .toolsets
                .iter()
                .find(|s| &s.id == id)
                .map(|s| s.tools.clone())
        });

        // -- the tools --
        let mut list = v_flex()
            .id("toolbox-list")
            .p_2()
            .gap_0p5()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();
        if let Some(payload) = &tools {
            let mut hidden = 0usize;
            let mut found = 0usize;
            for tool in &payload.tools {
                let installed = tool.status == "present";
                // Searching looks through everything the catalogue knows: how else would one
                // find a tool to install. Without a search, what is not here is hidden.
                if searching {
                    let tool_tags = tool.tags.join(" ");
                    if !crate::quick_filter::matches(
                        &query,
                        &[&tool.id, &tool.name, &tool.desc, &tool_tags],
                    ) {
                        continue;
                    }
                } else if !installed && tool.mcp.is_none() && !self.show_uninstalled {
                    hidden += 1;
                    continue;
                }
                found += 1;
                let dim = brought_forward
                    .as_ref()
                    .is_some_and(|members| !members.contains(&tool.id));
                let (tool_id, tool_active, weak_app) =
                    (tool.id.clone(), tool.active, self.app.clone());
                // An MCP tool is not installed or not: its server answers or does not, and it
                // is listed either way, tagged with what it costs.
                let row = if installed || tool.mcp.is_some() {
                    let reason = standing(tool, toolsets.as_ref());
                    let sets = memberships(tool, toolsets.as_ref());
                    let mut tip = String::new();
                    if !tool.desc.is_empty() {
                        tip.push_str(&tool.desc);
                        tip.push('\n');
                    }
                    tip.push_str(&if sets.is_empty() {
                        "In no toolset.".to_string()
                    } else {
                        format!("In toolsets: {}.", sets.join(", "))
                    });
                    tip.push('\n');
                    tip.push_str(match tool.override_kind.as_deref() {
                        Some("on") => "Pinned on by you. Click to drop the pin.",
                        Some("off") => "Pinned off by you. Click to drop the pin.",
                        _ if tool_active => "On through its active toolsets. Click to pin it off.",
                        _ => "Off. Click to pin it on.",
                    });
                    crate::hints::tip(
                        h_flex()
                            .id(SharedString::from(format!("tool-{tool_id}")))
                            .h(px(26.))
                            .px_2()
                            .gap_2()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .on_click(move |_, _window, cx| {
                                if let Some(app) = weak_app.upgrade() {
                                    // The target state is the opposite of the current one.
                                    app.update(cx, |app, cx| {
                                        app.toggle_tool(&tool_id, !tool_active, cx)
                                    });
                                }
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(if tool_active { theme.success } else { muted })
                                    .child(if tool_active { "●" } else { "○" }),
                            )
                            .child(
                                Label::new(tool.id.clone())
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(theme.mono_font_size)
                                    .text_color(if tool_active { theme.foreground } else { muted }),
                            )
                            .children(mcp_badge(tool, &theme))
                            .child(div().flex_1())
                            .when_some(reason, |row, (text, pinned)| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .max_w(relative(0.5))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(theme.font_size * 0.75)
                                        .text_color(if pinned { theme.accent } else { muted })
                                        .child(SharedString::from(text)),
                                )
                            }),
                        SharedString::from(tip),
                    )
                    .into_any_element()
                } else {
                    // Not installed: the row offers what would help, installing it in a fresh
                    // terminal where its questions can be answered.
                    let install_id = tool_id.clone();
                    install_row(&tool_id, &theme, move |window, cx| {
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
                    })
                    .into_any_element()
                };
                list = list.child(div().when(dim, |d| d.opacity(0.35)).child(row));
            }
            if payload.tools.is_empty() {
                list = list.child(div().px_2().text_color(muted).child("no tools catalogued"));
            } else if searching && found == 0 && !any_tag {
                list = list.child(
                    div()
                        .px_2()
                        .text_color(muted)
                        .child(format!("nothing matches `{}`", query.trim())),
                );
            } else if hidden > 0 || self.show_uninstalled {
                let showing = self.show_uninstalled;
                list = list.child(crate::hints::tip(
                    div()
                        .id("toggle-uninstalled-line")
                        .px_2()
                        .pt_1()
                        .cursor_pointer()
                        .text_color(muted)
                        .text_size(theme.font_size * 0.8)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.show_uninstalled = !this.show_uninstalled;
                            cx.notify();
                        }))
                        .child(if showing {
                            "hide what is not installed".to_string()
                        } else {
                            format!("{hidden} not installed")
                        }),
                    if showing {
                        "Hide tools that are not installed"
                    } else {
                        "Show tools that are not installed, to install them"
                    },
                ));
            }
        } else if loading {
            list = list.child(loading_row("loading tools…", &theme));
        } else {
            list = list.child(
                div()
                    .px_2()
                    .text_color(muted)
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
        v_flex()
            .size_full()
            .child(scope_header(&self.app, scope.as_deref(), &theme))
            .child(div().px_2().pt_2().child(Input::new(&self.query).small()))
            .child(tags)
            .when(mcp_active > 0, |el| {
                el.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_color(muted)
                        .text_size(theme.font_size * 0.8)
                        .child(format!(
                            "{mcp_active} MCP tool{} active — about {mcp_tokens} tokens in every request",
                            if mcp_active == 1 { "" } else { "s" }
                        )),
                )
            })
            .child(
                div()
                    .mt_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would come with it and shadow the standard one.
    use super::{memberships, standing};

    fn tool(id: &str, active: bool, pin: Option<&str>) -> reactor_client::ToolRow {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "desc": "", "status": "present",
            "active": active, "override": pin,
        }))
        .unwrap()
    }

    fn toolsets() -> reactor_client::ToolsetsPayload {
        serde_json::from_value(serde_json::json!({"schema": 1, "toolsets": [
            {"id": "all", "desc": "", "active": true, "tools": ["gdb", "jq", "rg"]},
            {"id": "triage", "desc": "", "active": true, "tools": ["jq", "rg"]},
            {"id": "native", "desc": "", "active": false, "tools": ["gdb", "rg"]},
        ]}))
        .unwrap()
    }

    fn says(row: &reactor_client::ToolRow) -> Option<String> {
        standing(row, Some(&toolsets())).map(|(text, _)| text)
    }

    #[test]
    fn a_row_names_the_active_toolsets_its_tool_is_on_through() {
        // `all` holds everything, so it is named only when nothing narrower is active.
        assert_eq!(says(&tool("jq", true, None)).as_deref(), Some("via triage"));
        assert_eq!(says(&tool("gdb", true, None)).as_deref(), Some("via all"));
        assert_eq!(says(&tool("gdb", false, None)), None);
    }

    #[test]
    fn a_pin_is_said_whatever_the_toolsets_would_do() {
        assert_eq!(
            says(&tool("gdb", true, Some("on"))).as_deref(),
            Some("pinned on")
        );
        assert_eq!(
            says(&tool("jq", false, Some("off"))).as_deref(),
            Some("pinned off")
        );
    }

    #[test]
    fn a_tool_is_in_every_toolset_that_lists_it_but_all_is_not_news() {
        let sets = toolsets();
        assert_eq!(
            memberships(&tool("rg", true, None), Some(&sets)),
            ["triage", "native"]
        );
        assert_eq!(
            memberships(&tool("gdb", true, None), Some(&sets)),
            ["native"]
        );
        assert!(memberships(&tool("x", false, None), Some(&sets)).is_empty());
        assert!(memberships(&tool("rg", true, None), None).is_empty());
    }
}
