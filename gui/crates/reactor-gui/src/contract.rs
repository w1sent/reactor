//! The extension-UI contract (gui/SPEC.md §4) — the GUI side.
//!
//! Reactor-specific, additive, transparent ([ADR-0032](../../docs/adr/0032-the-gui-extends-pis-rpc-through-existing-channels-only.md)):
//! extensions ride only channels that already exist in pi's RPC mode.
//!
//! - **Outbound**: a view is a `setWidget` whose lines carry a marker, JSON,
//!   and a readable fallback. This module detects and parses the envelope;
//!   every other client renders the fallback lines.
//! - **Inbound**: user actions are extension commands the GUI invokes
//!   through RPC `prompt` — immediate, transcript-free (verified in pi
//!   0.87.0; see "Facts for reactor-gui" in docs/pi-api-notes.md).
//! - The GUI holds no facts: it renders the envelope, dispatches actions,
//!   and the extension repaints. State lives extension-side, as ADR-0029
//!   rules for the TUI.

use serde_json::Value;

/// The marker prefixing line 0 of an envelope widget.
pub const MARKER: &str = "REACTOR-GUI-VIEW";
/// The current schema version, echoed in the marker line.
pub const VERSION: u32 = 1;
/// The `widgetKey` prefix that identifies a view envelope.
pub const KEY_PREFIX: &str = "reactor:";

/// One parsed view, from a `setWidget` extension-UI request.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// `reactor:<viewId>` — without the prefix.
    pub view_id: String,
    pub title: String,
    /// The extension's own event command, e.g. `/reactor-tools-event`. The
    /// GUI never guesses it — the envelope advertises it (ADR-0032).
    pub command: String,
    pub placement: Placement,
    pub content: ViewContent,
    pub footer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// A sheet over the transcript — selector, guide (today's TUI overlay
    /// semantics).
    Overlay,
    /// A panel in the right dock.
    Side,
}

/// Schema v1 primitives: exactly what the selector and guide need — nothing
/// fancier ships without a schema v2 (gui/SPEC.md §4.2).
#[derive(Debug, Clone, PartialEq)]
pub enum ViewContent {
    Table {
        columns: Vec<Column>,
        rows: Vec<Row>,
    },
    List {
        items: Vec<ListItem>,
    },
    Detail {
        /// Markdown body.
        body: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub id: String,
    pub title: String,
}

/// One row: cells by column id, plus the row-level actions.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub cells: BTreeCellMap,
    pub actions: Vec<Action>,
}
pub type BTreeCellMap = std::collections::BTreeMap<String, Cell>;

/// One cell: text plus an optional pi-vocabulary colour (§7's mapping).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub text: String,
    pub color: Option<String>,
}

/// One list item with optional actions.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub id: String,
    pub label: String,
    pub color: Option<String>,
    pub actions: Vec<Action>,
}

/// An action a user can click; fires the envelope's `command` with the view
/// id, action id, and the row id if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub disabled: bool,
}

impl View {
    /// Parse an envelope out of a `setWidget`'s lines. `None` when the lines
    /// are not an envelope — a plain widget renders as text, unchanged.
    pub fn parse(widget_key: &str, lines: &[String]) -> Option<View> {
        let raw_view_id = widget_key.strip_prefix(KEY_PREFIX)?;
        let first = lines.first()?;
        let json_part = first.strip_prefix(&format!("{MARKER} v{VERSION} "))?;
        let payload: Value = serde_json::from_str(json_part).ok()?;
        parse_payload(raw_view_id.to_owned(), &payload)
    }

    /// The readable fallback lines — what every other client shows. The
    /// marker line is excluded; a text client should never see it.
    pub fn fallback_lines(lines: &[String]) -> &[String] {
        if lines.len() > 1 { &lines[1..] } else { &[] }
    }
}

/// Build an event-command message for one user action — the `prompt` the GUI
/// sends back (ADR-0032's inbound channel).
pub fn event_command(command: &str, view_id: &str, action: &str, row: Option<&str>) -> String {
    let mut payload = serde_json::json!({ "view": view_id, "action": action });
    if let Some(row) = row {
        payload["row"] = serde_json::json!(row);
    }
    format!("{command} {payload}")
}

impl View {
    /// `(command, payload)` for [`ReactorApp::run_ui_event`][run_ui_event] —
    /// the split half every caller that already holds a `View` needs, so a
    /// view renderer (today's table/list/detail, and any future primitive)
    /// never re-derives [`event_command`]'s wire format by hand.
    ///
    /// [run_ui_event]: crate::app::ReactorApp::run_ui_event
    pub fn dispatch(&self, action: &str, row: Option<&str>) -> (String, String) {
        let full = event_command(&self.command, &self.view_id, action, row);
        full.split_once(' ')
            .map(|(command, payload)| (command.to_owned(), payload.to_owned()))
            .unwrap_or((self.command.clone(), String::new()))
    }
}

fn parse_payload(view_id: String, payload: &Value) -> Option<View> {
    if payload.get("v")?.as_u64()? != VERSION as u64 {
        return None;
    }
    let command = payload.get("command")?.as_str()?.to_owned();
    if !command.starts_with('/') {
        return None;
    }
    let placement = match payload.get("placement").and_then(Value::as_str) {
        Some("side") => Placement::Side,
        _ => Placement::Overlay,
    };
    let content = parse_content(payload)?;
    Some(View {
        view_id,
        title: payload
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        command,
        placement,
        content,
        footer: payload
            .get("footer")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn parse_content(payload: &Value) -> Option<ViewContent> {
    if let Some(table) = payload.get("table") {
        let columns = table
            .get("columns")?
            .as_array()?
            .iter()
            .filter_map(Column::from_value)
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for row in table.get("rows")?.as_array()? {
            let id = row.get("id").and_then(Value::as_str)?;
            let mut cells = std::collections::BTreeMap::new();
            if let Some(cells_json) = row.get("cells").and_then(Value::as_object) {
                for (col, cell) in cells_json {
                    cells.insert(col.to_owned(), Cell::from_value(cell));
                }
            }
            rows.push(Row {
                id: id.to_owned(),
                cells,
                actions: parse_actions(row.get("actions"))?,
            });
        }
        return Some(ViewContent::Table { columns, rows });
    }
    if let Some(list) = payload.get("list") {
        let items = list
            .get("items")?
            .as_array()?
            .iter()
            .filter_map(ListItem::from_value)
            .collect::<Vec<_>>();
        return Some(ViewContent::List { items });
    }
    if let Some(detail) = payload.get("detail") {
        return Some(ViewContent::Detail {
            body: detail
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        });
    }
    None
}

impl Column {
    fn from_value(v: &Value) -> Option<Column> {
        Some(Column {
            id: v.get("id")?.as_str()?.to_owned(),
            title: v
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        })
    }
}

impl Cell {
    fn from_value(v: &Value) -> Cell {
        Cell {
            text: v
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            color: v.get("color").and_then(Value::as_str).map(str::to_owned),
        }
    }
}

impl ListItem {
    fn from_value(v: &Value) -> Option<ListItem> {
        Some(ListItem {
            id: v.get("id")?.as_str()?.to_owned(),
            label: v
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            color: v.get("color").and_then(Value::as_str).map(str::to_owned),
            actions: parse_actions(v.get("actions"))?,
        })
    }
}

fn parse_actions(v: Option<&Value>) -> Option<Vec<Action>> {
    let actions = match v {
        None => return Some(Vec::new()),
        Some(v) => v.as_array()?,
    };
    Some(actions.iter().filter_map(Action::from_value).collect())
}

impl Action {
    fn from_value(v: &Value) -> Option<Action> {
        Some(Action {
            id: v.get("id")?.as_str()?.to_owned(),
            label: v
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            disabled: v.get("disabled").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The envelope round trip from §4.2 of gui/SPEC.md — the selector's
    /// table, marker line first, fallback lines second.
    #[test]
    fn envelope_line_0_is_detected_and_parsed() {
        let payload = json!({
            "v": 1,
            "view": "selector",
            "title": "Tools",
            "command": "/reactor-tools-event",
            "placement": "overlay",
            "table": {
                "columns": [
                    {"id": "tool", "title": "Tool"},
                    {"id": "state", "title": ""}
                ],
                "rows": [
                    {"id": "bn",
                     "cells": {"tool": {"text": "bn"},
                               "state": {"text": "●", "color": "success"}},
                     "actions": [{"id": "toggle", "label": "Toggle", "disabled": false}]}
                ]
            },
            "footer": "9 active · 12 catalogued"
        });
        let lines = vec![
            format!("{MARKER} v{VERSION} {}", payload),
            "bn      ● 9 active".to_owned(),
        ];
        let view = View::parse("reactor:selector", &lines).unwrap();
        assert_eq!(view.view_id, "selector");
        assert_eq!(view.command, "/reactor-tools-event");
        assert_eq!(view.placement, Placement::Overlay);
        match &view.content {
            ViewContent::Table { columns, rows } => {
                assert_eq!(columns.len(), 2);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].cells["state"].text, "●");
                assert_eq!(rows[0].cells["state"].color.as_deref(), Some("success"));
                assert_eq!(rows[0].actions[0].id, "toggle");
            }
            other => panic!("expected table, got {other:?}"),
        }
        assert_eq!(view.footer.as_deref(), Some("9 active · 12 catalogued"));
    }

    /// Transparency by construction: the fallback lines are what every other
    /// client renders, and they must be there — a marker line alone is not a
    /// valid envelope (gui/SPEC.md §4.2).
    #[test]
    fn fallback_lines_serve_every_other_client() {
        let lines = vec![
            format!("{MARKER} v{VERSION} {{}}"),
            "9 active · 12 catalogued".to_owned(),
            "bn  ●".to_owned(),
        ];
        assert_eq!(
            View::fallback_lines(&lines),
            &["9 active · 12 catalogued".to_owned(), "bn  ●".to_owned()]
        );
        // A single line = no fallback: not an envelope, render as text.
        assert!(View::parse("reactor:x", &[format!("{MARKER} v{VERSION} {{}}")]).is_none());
    }

    #[test]
    fn non_envelope_widgets_render_as_text() {
        // No `reactor:` prefix, no marker: the plain widget path, exactly
        // what pi's RPC setWidget already carries.
        assert!(View::parse("some-extension-panel", &["just text".into()]).is_none());
        assert!(View::parse("reactor:selector", &["just text".into()]).is_none());
    }

    #[test]
    fn list_and_detail_primitives_parse() {
        let list = json!({
            "v": 1, "view": "guide", "title": "Guide", "command": "/guide-event",
            "list": {"items": [{"id": "p1", "label": "Concept"}]}
        });
        let view = View::parse("reactor:guide", &[format!("{MARKER} v1 {list}")]).unwrap();
        assert_eq!(
            view.content,
            ViewContent::List {
                items: vec![ListItem {
                    id: "p1".into(),
                    label: "Concept".into(),
                    color: None,
                    actions: vec![]
                }]
            }
        );

        let detail = json!({
            "v": 1, "view": "detail", "title": "Detail", "command": "/d-event",
            "detail": {"body": "# Hello"}
        });
        let view = View::parse("reactor:detail", &[format!("{MARKER} v1 {detail}")]).unwrap();
        assert_eq!(
            view.content,
            ViewContent::Detail {
                body: "# Hello".into()
            }
        );
    }

    #[test]
    fn side_placement_parses() {
        let payload = json!({
            "v": 1, "view": "status", "title": "Status", "command": "/s-event",
            "placement": "side",
            "list": {"items": []}
        });
        let view = View::parse("reactor:status", &[format!("{MARKER} v1 {payload}")]).unwrap();
        assert_eq!(view.placement, Placement::Side);
    }

    /// Schema v2 from a future extension: parsed but not rendered as v1 —
    /// the fallback lines carry it until the GUI grows the schema.
    #[test]
    fn future_schema_versions_fall_back_to_text() {
        let payload = json!({"v": 2, "view": "fancy", "command": "/fancy-event"});
        let lines = vec![format!("{MARKER} v{VERSION} {payload}"), "fallback".into()];
        assert!(View::parse("reactor:fancy", &lines).is_none());
    }

    #[test]
    fn event_commands_carry_the_view_and_action() {
        // Compare as JSON: serde_json sorts object keys (no preserve_order),
        // so the field order in the string is not the literal's order.
        let got = event_command("/reactor-tools-event", "selector", "toggle", Some("bn"));
        let (cmd, payload) = got.split_once(' ').unwrap();
        assert_eq!(cmd, "/reactor-tools-event");
        let payload: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            payload,
            json!({ "view": "selector", "action": "toggle", "row": "bn" })
        );
        assert_eq!(
            event_command("/reactor-tree", "tree", "switch", None),
            "/reactor-tree {\"view\":\"tree\",\"action\":\"switch\"}"
        );
    }
}
