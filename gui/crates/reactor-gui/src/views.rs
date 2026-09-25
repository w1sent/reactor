//! Rendering for the extension-UI envelope contract (gui/SPEC.md §4.2,
//! [ADR-0032](../../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)) —
//! the one place a parsed [`contract::View`] becomes pixels.
//!
//! **This is the seam future visualizations grow from.** A view arrives as
//! one `contract::View`, parsed on the wire in `contract.rs`, and is turned
//! into an element here by matching on [`ViewContent`]. Today's three
//! primitives — `Table`, `List`, `Detail` — are schema v1 (gui/SPEC.md
//! §4.2); a schema v2 primitive (a timeline, a graph, a syntax-highlighted
//! code view, …) is:
//!
//! 1. a new `ViewContent` variant in `contract.rs`,
//! 2. a new parse arm in `contract::parse_content`,
//! 3. one new `render_*` function here plus a match arm in
//!    [`render_content`].
//!
//! Nothing else changes: [`render_content`] is the only place a
//! `ViewContent` becomes UI, whether the view is docked to the side panel
//! ([`crate::panels::ExtensionViewsPanel`]) or floated over the transcript
//! as an overlay sheet ([`crate::app::ReactorApp::render_overlay`]). Both
//! call sites also share [`view_chrome`] for the title/dismiss row, so a new
//! primitive automatically looks consistent in either placement.
//!
//! This module knows nothing about `ReactorApp` or RPC — it is handed a
//! `View`/`ViewContent` and an `on_action` callback, and turns out an
//! element. That keeps it testable and reusable independent of where a view
//! ends up docked.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::label::Label;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, Sizable as _, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, SharedString, Window, div};

use crate::contract::{Column, ListItem, Row, View, ViewContent};

/// One user action, fired from a row/item button: the action id, and the row
/// id when the click came from a table row rather than a list item.
pub type OnAction = dyn Fn(&str, Option<&str>, &mut Window, &mut App) + 'static;

/// The title + dismiss row every placement draws above a view's body.
pub fn view_chrome(
    view: &View,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    h_flex()
        .justify_between()
        .items_center()
        .px_2()
        .gap_2()
        .child(Label::new(if view.title.is_empty() {
            view.view_id.clone()
        } else {
            view.title.clone()
        }))
        .child(
            Button::new(SharedString::from(format!("dismiss-{}", view.view_id)))
                .icon(IconName::Close)
                .ghost()
                .small()
                .on_click(move |_, window, cx| on_dismiss(window, cx)),
        )
}

/// The footer line an envelope may carry ("9 active · 12 catalogued").
pub fn view_footer(footer: &str, theme: &Theme) -> impl IntoElement {
    div()
        .px_2()
        .pb_1()
        .text_color(theme.muted_foreground)
        .text_size(theme.font_size * 0.8)
        .child(footer.to_owned())
}

/// Render one view's body. `on_action` is cloned per row/item action button;
/// it is the caller's job to close over the `View`'s own `command` and
/// `view_id` (via [`View::dispatch`]) and the `WeakEntity<ReactorApp>` that
/// actually sends the RPC prompt — this module never touches either.
pub fn render_content(
    content: &ViewContent,
    theme: &Theme,
    on_action: impl Fn(&str, Option<&str>, &mut Window, &mut App) + Clone + 'static,
) -> AnyElement {
    match content {
        ViewContent::Table { columns, rows } => {
            render_table(columns, rows, theme, on_action).into_any_element()
        }
        ViewContent::List { items } => render_list(items, theme, on_action).into_any_element(),
        ViewContent::Detail { body } => render_detail(body, theme).into_any_element(),
    }
}

fn action_buttons(
    actions: &[crate::contract::Action],
    id_prefix: &str,
    row_id: Option<&str>,
    on_action: impl Fn(&str, Option<&str>, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let mut group = h_flex().gap_1();
    for action in actions {
        let on_action = on_action.clone();
        let action_id = action.id.clone();
        let row_id = row_id.map(str::to_owned);
        group = group.child(
            Button::new(SharedString::from(format!("{id_prefix}-{}", action.id)))
                .label(action.label.clone())
                .small()
                .disabled(action.disabled)
                .on_click(move |_, window, cx| {
                    on_action(&action_id, row_id.as_deref(), window, cx)
                }),
        );
    }
    group
}

fn render_table(
    columns: &[Column],
    rows: &[Row],
    theme: &Theme,
    on_action: impl Fn(&str, Option<&str>, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let mut table = v_flex().gap_1();
    if !columns.is_empty() {
        let mut header = h_flex().gap_3().px_2();
        for column in columns {
            header = header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(theme.muted_foreground)
                    .text_size(theme.font_size * 0.75)
                    .child(column.title.clone()),
            );
        }
        table = table.child(header);
    }
    for row in rows {
        let mut cells = h_flex().flex_1().min_w_0().gap_3();
        for column in columns {
            let cell = row.cells.get(&column.id);
            let color = cell
                .and_then(|c| c.color.as_deref())
                .map(|name| crate::theme::map_pi_color(name).resolve(theme))
                .unwrap_or(theme.foreground);
            cells = cells.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(color)
                    .child(cell.map(|c| c.text.clone()).unwrap_or_default()),
            );
        }
        table = table.child(
            h_flex()
                .justify_between()
                .items_center()
                .gap_3()
                .px_2()
                .py_1()
                .rounded_md()
                .hover(|style| style.bg(theme.list_hover))
                .child(cells)
                .child(action_buttons(
                    &row.actions,
                    &format!("row-{}", row.id),
                    Some(&row.id),
                    on_action.clone(),
                )),
        );
    }
    if rows.is_empty() {
        table = table.child(
            div()
                .px_2()
                .text_color(theme.muted_foreground)
                .child("no rows"),
        );
    }
    table
}

fn render_list(
    items: &[ListItem],
    theme: &Theme,
    on_action: impl Fn(&str, Option<&str>, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let mut list = v_flex().gap_1();
    for item in items {
        let color = item
            .color
            .as_deref()
            .map(|name| crate::theme::map_pi_color(name).resolve(theme))
            .unwrap_or(theme.foreground);
        list = list.child(
            h_flex()
                .justify_between()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .hover(|style| style.bg(theme.list_hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(color)
                        .child(item.label.clone()),
                )
                .child(action_buttons(
                    &item.actions,
                    &format!("item-{}", item.id),
                    Some(&item.id),
                    on_action.clone(),
                )),
        );
    }
    if items.is_empty() {
        list = list.child(
            div()
                .px_2()
                .text_color(theme.muted_foreground)
                .child("no items"),
        );
    }
    list
}

fn render_detail(body: &str, theme: &Theme) -> impl IntoElement {
    div().px_2().child(
        TextView::markdown("view-detail", body.to_owned())
            .style(crate::theme::text_view_style(theme)),
    )
}
