//! The graph panel: the case's graph, drawn, and edited by hand
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! Lines are painted on one canvas; nodes and line labels are ordinary elements above
//! it, so they get the theme, the pointer and text for free. Positions are in *world*
//! units and the view is a pan and a zoom over them: zooming moves things apart
//! without shrinking the text. What goes where is [`super::model`]'s — this file only
//! draws it and turns clicks into changes, which go through the same write path as the
//! agent's and are recorded as the analyst's.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton,
    PathBuilder, Pixels, Point, ScrollDelta, SharedString, Window, canvas, div, point, px,
};
use reactor_db::graph::edit::{self, At, TypeChoice};
use reactor_db::graph::layout;
use reactor_db::graph::{self, query};
use serde::Deserialize;

use super::arrange::{self, LayoutKind};
use super::model::{self, Line, LineKind, Model, NodeKind, SourceTarget};
use crate::app::ReactorApp;
use crate::backend::Backend;
use crate::hints;
use crate::panels::{close_button, closer, panel_title_closing};

/// A confidence picked from the detail box's menu; empty clears it.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct GraphSetConfidence {
    pub value: String,
}

/// A way of arranging the graph, picked from the toolbar's layout menu.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct GraphSetLayout {
    pub kind: String,
}

/// How much of the graph to draw: `auto`, `basic`, `analysis` or `full`.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct GraphSetDetail {
    pub level: String,
}

/// A hypothesis status picked from the detail box's menu.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct GraphSetHypothesis {
    pub status: String,
}

/// Where something added by hand goes.
#[derive(Clone, Copy)]
enum Spot {
    /// Exactly here (world units): where the pointer was for a right click.
    Exact((f32, f32)),
    /// As near here as there is room.
    Near((f32, f32)),
    /// Beside what is selected, else in the middle of the view.
    Auto,
}

/// What the add form adds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddMode {
    Entity,
    Note,
}

/// A form over the drawing: adding an entity or a note, or stating a relation.
#[derive(Clone)]
enum Form {
    Add {
        mode: AddMode,
        spot: Spot,
        /// Where the card sits, in the drawing area: at the pointer for a right click.
        anchor: Option<(f32, f32)>,
    },
    /// A relation between these, in the order they were selected: two are related
    /// by two roles, more are the members of one claim.
    Relation { ids: Vec<String> },
}

/// What is being dragged.
enum Drag {
    /// The whole view: where the pointer went down, and the pan then.
    View { from: (f32, f32), pan: (f32, f32) },
    /// The selected nodes: where the pointer went down (world units) and where each was.
    Nodes {
        from: (f32, f32),
        starts: Vec<(String, (f32, f32))>,
    },
    /// An area being swept out to select what is in it (drawing-area coordinates).
    Marquee { from: (f32, f32), to: (f32, f32) },
}

#[derive(Clone, PartialEq)]
enum Selected {
    /// Nodes, in the order they were picked.
    Nodes(Vec<String>),
    /// A drawn relation: the claims it stands for.
    Line(Vec<String>),
}

pub struct GraphPanel {
    app: gpui_kit::WeakEntity<ReactorApp>,
    focus: FocusHandle,
    backend: Option<Arc<Backend>>,
    model: Model,
    /// Where each node is, in world units. Kept across refreshes so the picture a
    /// person has learned stays put; a node they dragged stays where they left it.
    positions: HashMap<String, (f32, f32)>,
    pan: (f32, f32),
    zoom: f32,
    drag: Option<Drag>,
    selected: Option<Selected>,
    /// The selection in full: one list of label and value rows per element.
    detail: Vec<Vec<(String, String)>>,
    show_deleted: bool,
    error: Option<String>,
    /// Where the drawing area is in the window, as of the last paint: pointer events
    /// arrive in window coordinates.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    note: Entity<InputState>,
    tag: Entity<InputState>,
    /// Which nodes to show, as typed (`crate::graph::model::parse_filter`).
    filter: Entity<InputState>,
    /// Draw an outline round the nodes that share a tag.
    show_groups: bool,
    /// How the graph is arranged: what *auto-layout* does, and how lines are drawn.
    layout_kind: LayoutKind,
    /// How much is drawn, if picked; else it follows the zoom.
    detail_pick: Option<model::Detail>,
    /// The form that is open, and its fields.
    form: Option<Form>,
    entity_type: Entity<InputState>,
    entity_value: Entity<InputState>,
    relation_type: Entity<InputState>,
    from_role: Entity<InputState>,
    to_role: Entity<InputState>,
    /// A relation is being made from this element: the next one clicked is its other end.
    relating: Option<String>,
    /// Taking back the latest version of an element, previewed and awaiting an answer.
    pending_undo: Option<(String, String)>,
    /// The confidence scale, weakest first.
    scale: Vec<String>,
    /// Types known for entities and for relations, offered as they are typed.
    entity_choices: Vec<TypeChoice>,
    relation_choices: Vec<TypeChoice>,
    /// What is offered for what is typed now: the type and why it is worth a look.
    entity_suggest: Vec<(String, String)>,
    relation_suggest: Vec<(String, String)>,
    /// Where the next thing added goes, when it is not the default.
    spot: Option<Spot>,
}

/// The detail box: its width, the label column, and what is left for a value once the
/// box's padding, the gap between the columns and the scrollbar are taken off.
const DETAIL_WIDTH: f32 = 560.;
const DETAIL_LABEL: f32 = 112.;
const DETAIL_VALUE: f32 = DETAIL_WIDTH - DETAIL_LABEL - 48.;

/// How far the view zooms: out far enough that a large graph is one picture, because
/// everything on it shrinks with it.
const MIN_ZOOM: f32 = 0.04;
const MAX_ZOOM: f32 = 3.;

fn node_size(kind: NodeKind) -> (f32, f32) {
    match kind {
        NodeKind::Entity => (170., 30.),
        NodeKind::Claim => (130., 24.),
        NodeKind::Annotation => (220., 44.),
    }
}

/// An element as label and value rows: what it is, when, who recorded it, what it
/// points at (its source among them) and what points at it.
fn rows(db: &reactor_db::Db, e: &query::Element) -> Vec<(String, String)> {
    use reactor_agent::tools::graph::name;
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut add = |label: &str, value: String| rows.push((label.to_string(), value));
    add("Type", format!("{} · {}", e.type_, e.kind.as_str()));
    if let Some(v) = &e.content.value {
        add("Value", v.clone());
    }
    if e.content.status != "active" {
        add("Status", e.content.status.clone());
    }
    if let Some(c) = &e.content.confidence {
        add("Confidence", c.clone());
    }
    if !e.content.tags.is_empty() {
        add("Tags", e.content.tags.join(", "));
    }
    let t = &e.content.time;
    if let Some(at) = &t.occurred_at {
        add("Happened", at.clone());
    }
    if t.valid_from.is_some() || t.valid_to.is_some() {
        add(
            "Held",
            format!(
                "{} → {}",
                t.valid_from.as_deref().unwrap_or("?"),
                t.valid_to.as_deref().unwrap_or("open")
            ),
        );
    }
    if let Some(raw) = &t.raw {
        add(
            "As written",
            format!(
                "{raw} (zone {})",
                t.tz_certainty.as_deref().unwrap_or("unknown")
            ),
        );
    }
    for (key, value) in &e.content.props {
        let value = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        add(key, value);
    }
    for g in query::edges_from(db, &e.id).unwrap_or_default() {
        let mut value = name(db, &g.to_id);
        if let Some(l) = &g.locator {
            value.push_str(&format!(" @ {l}"));
        }
        if let Some(v) = g.pinned_ver {
            value.push_str(&format!(" (its version {v})"));
        }
        add(&format!("→ {}", g.role), value);
    }
    for g in query::edges_to(db, &e.id).unwrap_or_default() {
        add(&format!("← {}", g.role), name(db, &g.from_id));
    }
    add(
        "Recorded",
        format!("version {} · {} · by {}", e.ver, e.recorded_at, e.author),
    );
    add("Id", e.id.clone());
    rows
}

/// Where the segment from a box's centre towards `toward` leaves the box.
fn edge_of_box(centre: (f32, f32), half: (f32, f32), toward: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (toward.0 - centre.0, toward.1 - centre.1);
    if dx.abs() < 0.01 && dy.abs() < 0.01 {
        return centre;
    }
    let scale = (half.0 / dx.abs().max(0.01)).min(half.1 / dy.abs().max(0.01));
    (centre.0 + dx * scale.min(1.), centre.1 + dy * scale.min(1.))
}

impl GraphPanel {
    pub fn new(
        app: gpui_kit::WeakEntity<ReactorApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entity_type =
            cx.new(|cx| InputState::new(window, cx).placeholder("type, e.g. net.ipv4"));
        let entity_value = cx.new(|cx| InputState::new(window, cx).placeholder("value"));
        let relation_type =
            cx.new(|cx| InputState::new(window, cx).placeholder("type, e.g. net.connection"));
        for input in [&entity_type, &relation_type] {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.refresh_suggestions(cx);
                }
            })
            .detach();
        }
        cx.subscribe_in(
            &entity_value,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit_form(window, cx);
                }
            },
        )
        .detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("filter nodes"));
        cx.subscribe_in(&filter, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        })
        .detach();
        Self {
            app,
            focus: cx.focus_handle(),
            backend: None,
            model: Model::default(),
            positions: HashMap::new(),
            pan: (0., 0.),
            zoom: 1.,
            drag: None,
            selected: None,
            detail: Vec::new(),
            show_deleted: false,
            error: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            note: cx.new(|cx| InputState::new(window, cx).placeholder("a note about this…")),
            tag: cx.new(|cx| InputState::new(window, cx).placeholder("tag, e.g. phase.recon")),
            filter,
            show_groups: false,
            layout_kind: LayoutKind::default(),
            detail_pick: None,
            form: None,
            entity_type,
            entity_value,
            relation_type,
            from_role: cx.new(|cx| InputState::new(window, cx).placeholder("role of the first")),
            to_role: cx.new(|cx| InputState::new(window, cx).placeholder("role of the second")),
            relating: None,
            pending_undo: None,
            scale: Vec::new(),
            entity_choices: Vec::new(),
            relation_choices: Vec::new(),
            entity_suggest: Vec::new(),
            relation_suggest: Vec::new(),
            spot: None,
        }
    }

    /// The panel reads the case through the backend. It is handed over rather than
    /// looked up, because the app is mid-update whenever it would be asked.
    pub fn set_backend(&mut self, backend: Option<Arc<Backend>>, cx: &mut Context<Self>) {
        self.backend = backend;
        // Where the nodes were left, as stored with the case.
        if let Some(b) = &self.backend {
            self.positions = layout::load(b.case.db()).unwrap_or_default();
        }
        self.refresh(cx);
    }

    /// Store where these nodes are, so that they are there next time. A failure to store is
    /// not worth interrupting for: the picture is only drawn again afresh.
    fn persist(&self, ids: &[String]) {
        let (Some(b), false) = (&self.backend, ids.is_empty()) else {
            return;
        };
        let rows: Vec<(String, (f32, f32))> = ids
            .iter()
            .filter_map(|id| self.positions.get(id).map(|p| (id.clone(), *p)))
            .collect();
        let _ = layout::save(b.case.db(), &rows);
    }

    /// The pointer is up: if nodes were being dragged, keep where they were left.
    fn end_drag(&mut self) {
        if let Some(Drag::Nodes { starts, .. }) = self.drag.take() {
            let moved: Vec<String> = starts.into_iter().map(|(id, _)| id).collect();
            self.persist(&moved);
        }
    }

    /// Read the graph again and place whatever is new.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let db = b.case.db();
        self.scale = edit::scale(db, "confidence").unwrap_or_default();
        self.entity_choices = edit::type_choices(db, &[graph::Kind::Entity]).unwrap_or_default();
        self.relation_choices =
            edit::type_choices(db, &[graph::Kind::Observation, graph::Kind::Event])
                .unwrap_or_default();
        match query::snapshot(db, self.show_deleted) {
            Ok((elements, edges)) => {
                // Which way a relation is drawn is the vocabulary's to say.
                let from_roles: Vec<String> = db
                    .read(|conn| {
                        let mut stmt =
                            conn.prepare("SELECT name FROM known_role WHERE direction = 'from'")?;
                        let rows = stmt
                            .query_map([], |r| r.get(0))?
                            .collect::<std::result::Result<_, _>>()?;
                        Ok(rows)
                    })
                    .unwrap_or_else(|_| model::FROM_ROLES.iter().map(|r| r.to_string()).collect());
                self.model = model::build(&elements, &edges, &from_roles);
                // What has no place yet is placed now, and keeps it.
                let unplaced: Vec<String> = self
                    .model
                    .nodes
                    .iter()
                    .filter(|n| !self.positions.contains_key(&n.id))
                    .map(|n| n.id.clone())
                    .collect();
                let placed = model::layout(&self.model, &self.positions);
                // Positions of what is hidden are kept: it comes back where it was.
                for (n, p) in self.model.nodes.iter().zip(placed) {
                    self.positions.insert(n.id.clone(), p);
                }
                self.persist(&unplaced);
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.describe();
        cx.notify();
    }

    /// Fill [`Self::detail`] for what is selected, dropping a selection that is gone.
    fn describe(&mut self) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let db = b.case.db();
        if let Some(Selected::Nodes(ids)) = &mut self.selected {
            let nodes = &self.model.nodes;
            ids.retain(|id| nodes.iter().any(|n| n.id == *id));
        }
        let ids: Vec<String> = match &self.selected {
            Some(Selected::Nodes(ids)) if ids.len() > 1 => {
                // Many at once: what they are, not each one in full.
                let mut rows = vec![("Selected".to_string(), format!("{} nodes", ids.len()))];
                for id in ids.iter().take(12) {
                    if let Some(n) = self.model.nodes.iter().find(|n| n.id == *id) {
                        rows.push((n.type_.clone(), n.label.clone()));
                    }
                }
                if ids.len() > 12 {
                    rows.push(("…".to_string(), format!("and {} more", ids.len() - 12)));
                }
                self.detail = vec![rows];
                return;
            }
            Some(Selected::Nodes(ids)) => ids.clone(),
            Some(Selected::Line(claims)) => claims.clone(),
            None => {
                self.detail.clear();
                return;
            }
        };
        self.detail = ids
            .iter()
            .filter_map(|id| query::get(db, id).ok().flatten())
            .map(|e| rows(db, &e))
            .collect();
        if self.detail.is_empty() {
            self.selected = None;
        }
    }

    fn select(&mut self, what: Option<Selected>, cx: &mut Context<Self>) {
        self.selected = what.filter(|s| !matches!(s, Selected::Nodes(ids) if ids.is_empty()));
        self.describe();
        cx.notify();
    }

    /// Pick a node up or put it down without touching the rest of the selection.
    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut ids = self.selected_nodes();
        match ids.iter().position(|x| x == id) {
            Some(i) => {
                ids.remove(i);
            }
            None => ids.push(id.to_string()),
        }
        self.select(Some(Selected::Nodes(ids)), cx);
    }

    /// The nodes that are selected.
    fn selected_nodes(&self) -> Vec<String> {
        match &self.selected {
            Some(Selected::Nodes(ids)) => ids.clone(),
            _ => Vec::new(),
        }
    }

    /// The ids a change to the selection applies to.
    fn selected_ids(&self) -> Vec<String> {
        match &self.selected {
            Some(Selected::Nodes(ids)) | Some(Selected::Line(ids)) => ids.clone(),
            None => Vec::new(),
        }
    }

    /// The one element the selection is, if it is exactly one: a node, or a line that
    /// stands for a single claim.
    fn single(&self) -> Option<String> {
        match &self.selected {
            Some(Selected::Nodes(ids)) | Some(Selected::Line(ids)) if ids.len() == 1 => {
                Some(ids[0].clone())
            }
            _ => None,
        }
    }

    /// Where the person is in the case: the session, and its latest entry.
    fn at(&self) -> Option<(Arc<Backend>, At)> {
        let b = self.backend.clone()?;
        let entry = b.store().lock().unwrap().head();
        let at = At::new(&b.session_id, entry);
        Some((b, at))
    }

    /// Make a change as the analyst. A refusal is shown as the reason it gave; either
    /// way the graph is read again.
    fn edit<T>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&reactor_db::Db, &At) -> reactor_db::Result<T>,
    ) -> Option<T> {
        let (b, at) = self.at()?;
        let before: HashSet<String> = self.model.nodes.iter().map(|n| n.id.clone()).collect();
        let spot = self.spot.take().unwrap_or(Spot::Auto);
        let result = f(b.case.db(), &at);
        self.refresh(cx);
        self.place_new(&before, spot);
        if let Err(e) = &result {
            self.error = Some(e.to_string());
            cx.notify();
        }
        result.ok()
    }

    /// Attach what is typed as a note about the selection.
    fn add_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.note.read(cx).value().trim().to_string();
        let ids = self.selected_ids();
        if text.is_empty() || ids.is_empty() {
            return;
        }
        if self
            .edit(cx, |db, at| edit::add_note(db, at, &ids, &text))
            .is_some()
        {
            self.note
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    /// Hide the selection: a new version with the status, nothing lost.
    fn soft_delete(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        self.selected = None;
        self.edit(cx, |db, at| edit::set_status(db, at, &ids, "soft-deleted"));
    }

    fn set_status(&mut self, status: &'static str, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        self.edit(cx, |db, at| edit::set_status(db, at, &ids, status));
    }

    fn set_confidence(&mut self, value: &str, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        self.edit(cx, |db, at| edit::set_confidence(db, at, &ids, Some(value)));
    }

    fn set_hypothesis(&mut self, status: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.single() {
            self.edit(cx, |db, at| {
                edit::set_hypothesis_status(db, at, &id, status)
            });
        }
    }

    /// Add (or take off) the tag that is typed, on everything selected.
    fn tag_selection(&mut self, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        let tag = self.tag.read(cx).value().trim().to_string();
        let ids = self.selected_ids();
        if tag.is_empty() || ids.is_empty() {
            return;
        }
        let tags = [tag];
        let done = if add {
            self.edit(cx, |db, at| edit::tag(db, at, &ids, &tags, &[]))
        } else {
            self.edit(cx, |db, at| edit::tag(db, at, &ids, &[], &tags))
        };
        if done.is_some() {
            self.tag
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    /// Preview taking back the latest version of the selected element.
    fn preview_undo(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.single() else { return };
        let Some((b, _)) = self.at() else { return };
        match edit::undo_preview(b.case.db(), &id) {
            Ok(plan) => {
                let mut text = String::from("Take back its latest version? This would take:");
                for e in &plan.elements {
                    text.push_str(&format!(
                        "\n• {} {} — {}; by {}",
                        e.type_,
                        if e.back_to == 0 {
                            "out of the graph".to_string()
                        } else {
                            format!("back to version {}", e.back_to)
                        },
                        e.why,
                        e.authors.join(", ")
                    ));
                }
                self.error = None;
                self.pending_undo = Some((id, text));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    fn confirm_undo(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.pending_undo.take() {
            self.edit(cx, |db, at| edit::undo(db, at, &id));
        }
    }

    /// What is typed in a type field, offered the known types it could be.
    fn refresh_suggestions(&mut self, cx: &App) {
        let offer = |choices: &[TypeChoice], typed: &str| -> Vec<(String, String)> {
            edit::suggest_types(choices, typed, 8)
                .into_iter()
                .map(|name| {
                    let c = choices.iter().find(|c| c.name == name);
                    let mut why = match c.map(|c| c.uses).unwrap_or(0) {
                        0 => "known, not used here yet".to_string(),
                        1 => "used once in this case".to_string(),
                        n => format!("used {n} times in this case"),
                    };
                    if let Some(c) = c.filter(|c| !c.aliases.is_empty()) {
                        why.push_str(&format!(" · also written {}", c.aliases.join(", ")));
                    }
                    (name, why)
                })
                .collect()
        };
        self.entity_suggest = offer(
            &self.entity_choices,
            self.entity_type.read(cx).value().as_ref(),
        );
        self.relation_suggest = offer(
            &self.relation_choices,
            self.relation_type.read(cx).value().as_ref(),
        );
    }

    /// Open the form for adding an entity or a note.
    fn open_add_form(
        &mut self,
        mode: AddMode,
        spot: Spot,
        anchor: Option<(f32, f32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.relating = None;
        self.form = Some(Form::Add { mode, spot, anchor });
        self.focus_add(mode, window, cx);
        self.refresh_suggestions(cx);
        cx.notify();
    }

    fn focus_add(&mut self, mode: AddMode, window: &mut Window, cx: &mut Context<Self>) {
        match mode {
            AddMode::Entity => self.entity_type.update(cx, |s, cx| s.focus(window, cx)),
            AddMode::Note => self.entity_value.update(cx, |s, cx| s.focus(window, cx)),
        }
    }

    /// Start a relation from the selected element: the next one clicked is the other end.
    fn start_relating(&mut self, cx: &mut Context<Self>) {
        if let Some(Selected::Nodes(ids)) = &self.selected
            && let [id] = ids.as_slice()
        {
            self.relating = Some(id.clone());
            self.form = None;
            cx.notify();
        }
    }

    fn open_relation_form(
        &mut self,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.relating = None;
        let (first, second) = if ids.len() == 2 {
            ("src", "dst")
        } else {
            ("member", "")
        };
        self.form = Some(Form::Relation { ids });
        self.from_role
            .update(cx, |s, cx| s.set_value(first, window, cx));
        self.to_role
            .update(cx, |s, cx| s.set_value(second, window, cx));
        self.relation_type.update(cx, |s, cx| s.focus(window, cx));
        self.refresh_suggestions(cx);
        cx.notify();
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = |input: &Entity<InputState>, cx: &App| input.read(cx).value().trim().to_string();
        let Some(form) = self.form.take() else { return };
        match form.clone() {
            Form::Add { mode, spot, .. } => {
                let value = text(&self.entity_value, cx);
                self.spot = Some(spot);
                let made = match mode {
                    AddMode::Entity => {
                        let ty = text(&self.entity_type, cx);
                        self.edit(cx, |db, at| edit::add_entity(db, at, &ty, &value))
                    }
                    AddMode::Note => self.edit(cx, |db, at| edit::note(db, at, &[], &value)),
                };
                match made {
                    Some(id) => {
                        self.entity_value
                            .update(cx, |s, cx| s.set_value("", window, cx));
                        self.select(Some(Selected::Nodes(vec![id])), cx);
                    }
                    None => self.form = Some(form),
                }
            }
            Form::Relation { ids } => {
                let (ty, fr, tr) = (
                    text(&self.relation_type, cx),
                    text(&self.from_role, cx),
                    text(&self.to_role, cx),
                );
                if ids.len() == 2 {
                    let ends = ids.clone();
                    match self.edit(cx, |db, at| {
                        edit::relate(db, at, &ends[0], &ends[1], &ty, &fr, &tr)
                    }) {
                        Some(claim) => {
                            self.relation_type
                                .update(cx, |s, cx| s.set_value("", window, cx));
                            // A relation shows as a line, which is selected by its claims.
                            self.select(Some(Selected::Line(vec![claim])), cx);
                        }
                        None => self.form = Some(form),
                    }
                } else {
                    // Members of one claim: a node of its own, among them.
                    self.spot = self.centroid(&ids).map(Spot::Near);
                    let members = ids.clone();
                    match self.edit(cx, |db, at| edit::relate_many(db, at, &members, &ty, &fr)) {
                        Some(claim) => {
                            self.relation_type
                                .update(cx, |s, cx| s.set_value("", window, cx));
                            self.select(Some(Selected::Nodes(vec![claim])), cx);
                        }
                        None => self.form = Some(form),
                    }
                }
            }
        }
        self.refresh_suggestions(cx);
        cx.notify();
    }

    // -- placement ----------------------------------------------------------------------------

    /// The middle of these nodes, in world units.
    fn centroid(&self, ids: &[String]) -> Option<(f32, f32)> {
        let points: Vec<(f32, f32)> = ids
            .iter()
            .filter_map(|id| self.positions.get(id).copied())
            .collect();
        (!points.is_empty()).then(|| {
            let n = points.len() as f32;
            (
                points.iter().map(|p| p.0).sum::<f32>() / n,
                points.iter().map(|p| p.1).sum::<f32>() / n,
            )
        })
    }

    /// Put what has just appeared where a person can see it. Nodes the graph already had
    /// stay as they are; new ones go where they were asked for (or beside what is selected,
    /// or in the middle of the view) rather than wherever the layout would drift them to,
    /// and the view moves to them if they would be out of sight.
    fn place_new(&mut self, before: &HashSet<String>, spot: Spot) {
        let fresh: Vec<String> = self
            .model
            .nodes
            .iter()
            .filter(|n| !before.contains(&n.id))
            .map(|n| n.id.clone())
            .collect();
        if fresh.is_empty() {
            return;
        }
        let want = match spot {
            Spot::Exact(p) | Spot::Near(p) => p,
            Spot::Auto => match self.centroid(&self.selected_nodes()) {
                Some(c) => (c.0, c.1 + 90.),
                None => self.to_world(self.centre()),
            },
        };
        let mut taken: Vec<(f32, f32)> = self
            .model
            .nodes
            .iter()
            .filter(|n| !fresh.contains(&n.id))
            .filter_map(|n| self.positions.get(&n.id).copied())
            .collect();
        for (i, id) in fresh.iter().enumerate() {
            let at = match spot {
                Spot::Exact(p) if i == 0 => p,
                _ => model::free_spot(&taken, want, 150.),
            };
            self.positions.insert(id.clone(), at);
            taken.push(at);
        }
        self.persist(&fresh);
        self.reveal(&fresh);
    }

    /// Pan to these nodes if any of them is out of view (and zoom in to a readable size
    /// if they would be too small to read).
    fn reveal(&mut self, ids: &[String]) {
        let size = self.bounds.get().size;
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        if w <= 0. || h <= 0. {
            return;
        }
        let margin = 40.;
        let inside = ids
            .iter()
            .filter_map(|id| self.positions.get(id))
            .map(|p| self.to_view(*p))
            .all(|p| p.0 >= margin && p.0 <= w - margin && p.1 >= margin && p.1 <= h - margin);
        if inside {
            return;
        }
        if let Some(c) = self.centroid(ids) {
            self.zoom = self.zoom.max(0.5);
            self.pan = (-c.0 * self.zoom, -c.1 * self.zoom);
        }
    }

    // -- finding where something came from ---------------------------------------------------

    /// Open the source of the selected element: a page in the browser, a file of the case in
    /// its viewer (at the line, if the locator names one), or the session it was said in.
    fn open_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some(b)) = (self.single(), self.backend.clone()) else {
            return;
        };
        let sources = query::sources_of(b.case.db(), &id).unwrap_or_default();
        let Some(first) = sources.first() else {
            self.error = Some(
                "It has no source of its own: an entity is not a statement. Its claims have one."
                    .into(),
            );
            cx.notify();
            return;
        };
        self.error = None;
        match model::source_target(&first.uri, first.locator.as_deref()) {
            SourceTarget::Url(url) => cx.open_url(&url),
            SourceTarget::Session { id, .. } => {
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| app.open_session(Some(id), window, cx));
                }
            }
            SourceTarget::File { path, line } => {
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.open_reference(&path, line, Default::default(), window, cx)
                    });
                }
            }
        }
        cx.notify();
    }

    // -- geometry ---------------------------------------------------------------------------

    fn local(&self, at: Point<Pixels>) -> (f32, f32) {
        let o = self.bounds.get().origin;
        (f32::from(at.x - o.x), f32::from(at.y - o.y))
    }

    fn centre(&self) -> (f32, f32) {
        let s = self.bounds.get().size;
        (f32::from(s.width) / 2., f32::from(s.height) / 2.)
    }

    /// World to the drawing area.
    fn to_view(&self, p: (f32, f32)) -> (f32, f32) {
        let c = self.centre();
        (
            c.0 + self.pan.0 + p.0 * self.zoom,
            c.1 + self.pan.1 + p.1 * self.zoom,
        )
    }

    fn to_world(&self, v: (f32, f32)) -> (f32, f32) {
        let c = self.centre();
        (
            (v.0 - c.0 - self.pan.0) / self.zoom,
            (v.1 - c.1 - self.pan.1) / self.zoom,
        )
    }

    fn node_at(&self, index: usize) -> (f32, f32) {
        self.to_view(
            self.positions
                .get(&self.model.nodes[index].id)
                .copied()
                .unwrap_or((0., 0.)),
        )
    }

    /// Lay the whole graph out afresh, forgetting where things were -- including where
    /// they were dragged to -- and bring it into view.
    fn relayout(&mut self, cx: &mut Context<Self>) {
        let placed = arrange::layout_as(&self.model, self.layout_kind);
        self.positions = self
            .model
            .nodes
            .iter()
            .zip(placed)
            .map(|(n, p)| (n.id.clone(), p))
            .collect();
        // The old arrangement is given up for good: stored positions too.
        if let Some(b) = &self.backend {
            let _ = layout::clear(b.case.db());
        }
        let all: Vec<String> = self.model.nodes.iter().map(|n| n.id.clone()).collect();
        self.persist(&all);
        self.fit(cx);
    }

    /// Arrange the whole graph the chosen way, and keep that way for the next auto-layout.
    fn set_layout(&mut self, kind: LayoutKind, cx: &mut Context<Self>) {
        self.layout_kind = kind;
        self.relayout(cx);
    }

    /// Bring everything into view at a readable size.
    fn fit(&mut self, cx: &mut Context<Self>) {
        self.pan = (0., 0.);
        self.zoom = 1.;
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for p in self
            .model
            .nodes
            .iter()
            .filter_map(|n| self.positions.get(&n.id))
        {
            lo = (lo.0.min(p.0), lo.1.min(p.1));
            hi = (hi.0.max(p.0), hi.1.max(p.1));
        }
        if lo.0 <= hi.0 {
            let c = self.centre();
            let (w, h) = (hi.0 - lo.0 + 260., hi.1 - lo.1 + 120.);
            if c.0 > 0. && c.1 > 0. {
                self.zoom = (c.0 * 2. / w).min(c.1 * 2. / h).clamp(MIN_ZOOM, 1.);
            }
            self.pan = (
                -(lo.0 + hi.0) / 2. * self.zoom,
                -(lo.1 + hi.1) / 2. * self.zoom,
            );
        }
        cx.notify();
    }
}

impl Focusable for GraphPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for GraphPanel {}

impl gpui_kit::base::dock::Panel for GraphPanel {
    fn panel_name(&self) -> &'static str {
        "graph"
    }
}

impl Panel for GraphPanel {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel_title_closing(IconName::GitBranch, "Graph", closer(&self.app, cx.entity()))
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let shown = self.show_deleted;
        Some(vec![
            Button::new("graph-refresh")
                .icon(IconName::RotateCw)
                .ghost()
                .small()
                .tooltip("Read the case's graph again")
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            Button::new("graph-add")
                .icon(IconName::Plus)
                .ghost()
                .small()
                .tooltip("Add an entity or a note by hand — or right-click the plane to add one right there")
                .on_click(cx.listener(|this, _, window, cx| {
                    let spot = Spot::Near(this.to_world(this.centre()));
                    this.open_add_form(AddMode::Entity, spot, None, window, cx)
                })),
            Button::new("graph-relayout")
                .icon(IconName::LayoutDashboard)
                .ghost()
                .small()
                .tooltip(format!(
                    "Auto-layout: arrange the whole graph afresh, {} (pick another way in the box at the top; positions you dragged are given up)",
                    self.layout_kind.label().to_lowercase()
                ))
                .on_click(cx.listener(|this, _, _, cx| this.relayout(cx))),
            Button::new("graph-fit")
                .icon(IconName::Layers)
                .ghost()
                .small()
                .tooltip("Fit everything into view")
                .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            Button::new("graph-groups")
                .icon(IconName::Tags)
                .ghost()
                .small()
                .tooltip(if self.show_groups {
                    "Outlines round the nodes that share a tag are shown — click to hide them"
                } else {
                    "Draw an outline round the nodes that share a tag"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_groups = !this.show_groups;
                    cx.notify();
                })),
            Button::new("graph-deleted")
                .icon(if shown {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                })
                .ghost()
                .small()
                .tooltip(if shown {
                    "Soft-deleted elements are shown — click to hide them"
                } else {
                    "Soft-deleted elements are hidden — click to show them"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_deleted = !this.show_deleted;
                    this.refresh(cx);
                })),
            close_button(&self.app, cx.entity()),
        ])
    }
}

/// One line, ready to paint: both ends in the drawing area's own coordinates.
/// A group's outline, ready to paint: its corners in the drawing area's coordinates.
struct Hull {
    points: Vec<(f32, f32)>,
    colour: Hsla,
    width: f32,
}

struct Stroke {
    from: (f32, f32),
    /// Corners between `from` and `to`, for a right-angled connector; none for a straight line.
    bends: Vec<(f32, f32)>,
    to: (f32, f32),
    colour: Hsla,
    width: f32,
    dashed: bool,
    arrow: bool,
    /// The arrowhead's scale: the zoom.
    head: f32,
}

impl Render for GraphPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let small = theme.font_size * 0.85;

        // How much is drawn: the level the person picked, or the one the zoom gives. What a
        // level leaves out is not hidden from the filter, the detail box or the tools: it is
        // only not drawn.
        let level = self
            .detail_pick
            .unwrap_or_else(|| model::Detail::for_zoom(self.zoom));
        let disclosure = model::disclose(&self.model, level);
        let plain = disclosure.plain;
        let status_colour = |status: &str, normal: Hsla| match status {
            // The plain picture has no status: it is how things are related, nothing more.
            "conflicting" if plain => normal,
            "conflicting" => theme.warning,
            "soft-deleted" => theme.muted_foreground.opacity(0.5),
            _ => normal,
        };
        let line_selected = |line: &Line| match &self.selected {
            Some(Selected::Line(claims)) => !line.claims.is_empty() && *claims == line.claims,
            _ => false,
        };
        // Everything drawn scales with the zoom, so zooming out brings things closer
        // together *and* makes them smaller: that is what makes a bigger picture.
        let filter = model::parse_filter(self.filter.read(cx).value().as_ref());
        // Shown: what the filter lets through and the level draws.
        let shown: Vec<bool> = model::visible(&self.model, &filter)
            .into_iter()
            .zip(&disclosure.shown)
            .map(|(a, b)| a && *b)
            .collect();
        let z = self.zoom;
        let font = |factor: f32| theme.font_size * 0.85 * factor * z;
        let readable = f32::from(font(1.)) >= 6.;
        let labels_shown = z >= 0.6;
        let selected_nodes = self.selected_nodes();

        // Lines: painted under everything, each ending at the edge of the node it
        // points to so an arrowhead is not hidden under the box.
        let mut strokes = Vec::with_capacity(self.model.lines.len());
        let mut labels = Vec::new();
        // The model's lines, and those that stand in for what the level does not draw.
        for (i, line) in self.model.lines.iter().chain(&disclosure.extra).enumerate() {
            // A line is drawn between two nodes that are shown.
            if !(shown[line.from] && shown[line.to]) {
                continue;
            }
            let (a, b) = (self.node_at(line.from), self.node_at(line.to));
            let half = |index: usize| {
                let (w, h) = node_size(self.model.nodes[index].kind);
                ((w / 2. + 3.) * z, (h / 2. + 3.) * z)
            };
            // Straight, from edge to edge of the boxes; or, in the orthogonal layout, out of
            // the side that faces, round two right-angled corners, into the side that faces.
            let (from, bends, to) = if self.layout_kind.right_angled() {
                let r = arrange::orthogonal_route(a, half(line.from), b, half(line.to));
                (r[0], vec![r[1], r[2]], r[3])
            } else {
                (
                    edge_of_box(a, half(line.from), b),
                    Vec::new(),
                    edge_of_box(b, half(line.to), a),
                )
            };
            let selected = line_selected(line);
            let base = match line.kind {
                LineKind::Relation { .. } => theme.foreground.opacity(0.7),
                LineKind::Role => theme.muted_foreground,
                LineKind::Annotation => theme.info.opacity(0.8),
            };
            let colour = if selected {
                theme.primary
            } else {
                status_colour(&line.status, base)
            };
            strokes.push(Stroke {
                from,
                bends: bends.clone(),
                to,
                colour,
                width: (if selected { 2.5 } else { 1.5 } * z).max(if selected { 1.5 } else { 0.8 }),
                head: z,
                dashed: line.kind == LineKind::Annotation,
                arrow: !matches!(line.kind, LineKind::Relation { directed: false }),
            });
            // The label sits in the middle of the line: of its middle stretch, if it has corners.
            let (m0, m1) = match bends.as_slice() {
                [b1, b2] => (*b1, *b2),
                _ => (from, to),
            };
            let mid = ((m0.0 + m1.0) / 2., (m0.1 + m1.1) / 2.);
            let claims = line.claims.clone();
            let is_relation = matches!(line.kind, LineKind::Relation { .. });
            let text = SharedString::from(line.label.clone());
            // Zoomed far out the words would be specks: a dot on the line stands for
            // them, with the words in its tooltip.
            let (lw, lh) = (140. * z.max(0.6), 18. * z.max(0.6));
            let label = if labels_shown {
                div()
                    .px_1()
                    .rounded_sm()
                    .bg(theme.background.opacity(0.85))
                    .text_size(font(1.))
                    .text_color(colour)
                    .whitespace_nowrap()
                    .when(is_relation, |d| d.cursor_pointer())
                    .child(text.clone())
                    .into_any_element()
            } else {
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(colour)
                    .when(is_relation, |d| d.cursor_pointer())
                    .into_any_element()
            };
            let holder = div()
                .id(("graph-line", i))
                .absolute()
                .left(px(mid.0 - lw / 2.))
                .top(px(mid.1 - lh / 2.))
                .w(px(lw))
                .h(px(lh))
                .flex()
                .justify_center()
                .items_center()
                .child(label)
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .when(is_relation, |d| {
                    d.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select(Some(Selected::Line(claims.clone())), cx);
                        }),
                    )
                });
            labels.push(if labels_shown {
                holder
            } else {
                hints::tip(holder, text)
            });
        }

        // Hulls: an outline round the shown nodes that share a tag (two or more; the twelve
        // most shared), painted under everything, in the tag's own colour. The tag's name
        // sits above the highest point of it.
        let mut hulls: Vec<Hull> = Vec::new();
        let mut hull_labels = Vec::new();
        if self.show_groups {
            for g in model::tag_groups(&self.model, &self.positions, &shown, 12) {
                // The same tag is the same colour, whichever other tags are on.
                let hue = (g
                    .tag
                    .bytes()
                    .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)))
                    % 360) as f32
                    / 360.;
                let colour = Hsla {
                    h: hue,
                    s: 0.55,
                    l: 0.55,
                    a: 1.,
                };
                // Round the boxes of the nodes (the widest a node is drawn, so none pokes
                // out), standing well off them.
                let outline: Vec<(f32, f32)> = model::padded_hull(&g.points, (110., 26.), 46.)
                    .into_iter()
                    .map(|p| self.to_view(p))
                    .collect();
                let Some(top) = outline.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)) else {
                    continue;
                };
                let label_h = 24. * z.max(0.7);
                let label = div()
                    .id(SharedString::from(format!("graph-group-{}", g.tag)))
                    .absolute()
                    .left(px(top.0 - 90.))
                    .top(px(top.1 - label_h))
                    .w(px(180.))
                    .h(px(label_h))
                    .flex()
                    .justify_center()
                    .items_center()
                    .child(
                        div()
                            .px_2()
                            .rounded_sm()
                            .bg(theme.background.opacity(0.85))
                            .text_size(font(0.9))
                            .text_color(colour)
                            .whitespace_nowrap()
                            .when(!readable, |d| d.invisible())
                            .child(SharedString::from(format!("{} · {}", g.tag, g.members))),
                    );
                // Zoomed out the name is too small to read: the tooltip has it.
                hull_labels.push(hints::tip(
                    label,
                    SharedString::from(format!("tag {} — {} nodes", g.tag, g.members)),
                ));
                hulls.push(Hull {
                    points: outline,
                    colour,
                    width: (1.5 * z).max(1.),
                });
            }
        }

        let cell = self.bounds.clone();
        let lines = canvas(
            move |bounds, _, _| cell.set(bounds),
            move |bounds, _, window, _| {
                let at =
                    |p: (f32, f32)| point(bounds.origin.x + px(p.0), bounds.origin.y + px(p.1));
                for h in &hulls {
                    if h.points.len() < 3 {
                        continue;
                    }
                    let corners: Vec<_> = h.points.iter().map(|p| at(*p)).collect();
                    let mut fill = PathBuilder::fill();
                    fill.add_polygon(&corners, true);
                    if let Ok(fill) = fill.build() {
                        window.paint_path(fill, h.colour.opacity(0.08));
                    }
                    let mut edge = PathBuilder::stroke(px(h.width));
                    edge.move_to(corners[0]);
                    for c in &corners[1..] {
                        edge.line_to(*c);
                    }
                    edge.line_to(corners[0]);
                    if let Ok(edge) = edge.build() {
                        window.paint_path(edge, h.colour.opacity(0.55));
                    }
                }
                for s in &strokes {
                    let mut path = PathBuilder::stroke(px(s.width));
                    if s.dashed {
                        path = path.dash_array(&[px(5.), px(4.)]);
                    }
                    path.move_to(at(s.from));
                    for corner in &s.bends {
                        path.line_to(at(*corner));
                    }
                    path.line_to(at(s.to));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, s.colour);
                    }
                    if s.arrow {
                        // The head points the way the last stretch goes.
                        let last = s.bends.last().copied().unwrap_or(s.from);
                        let (dx, dy) = (s.to.0 - last.0, s.to.1 - last.1);
                        let len = (dx * dx + dy * dy).sqrt();
                        let size = (9. * s.head).max(4.);
                        if len > size + 3. {
                            let (ux, uy) = (dx / len, dy / len);
                            let back = (s.to.0 - ux * size, s.to.1 - uy * size);
                            let wing = size * 0.45;
                            let mut head = PathBuilder::fill();
                            head.add_polygon(
                                &[
                                    at(s.to),
                                    at((back.0 - uy * wing, back.1 + ux * wing)),
                                    at((back.0 + uy * wing, back.1 - ux * wing)),
                                ],
                                true,
                            );
                            if let Ok(head) = head.build() {
                                window.paint_path(head, s.colour);
                            }
                        }
                    }
                }
            },
        )
        .absolute()
        .size_full();

        let nodes: Vec<_> = self
            .model
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| shown[*i])
            .map(|(i, node)| {
                let (x, y) = self.node_at(i);
                let (w, h) = node_size(node.kind);
                let (w, h) = (w * z, h * z);
                let selected = selected_nodes.contains(&node.id);
                let accent = match node.kind {
                    NodeKind::Entity => theme.border,
                    NodeKind::Claim => theme.muted_foreground,
                    NodeKind::Annotation => theme.info,
                };
                let border = if selected {
                    theme.primary
                } else {
                    status_colour(&node.status, accent)
                };
                let text = match node.kind {
                    NodeKind::Claim => node.type_.clone(),
                    _ => node.label.clone(),
                };
                let sub = match node.kind {
                    NodeKind::Annotation => {
                        Some(node.type_.trim_start_matches("analysis.").to_string())
                    }
                    _ => None,
                };
                // What the box cannot say at this size, its tooltip does.
                let full = match node.kind {
                    NodeKind::Claim => node.type_.clone(),
                    _ => format!("{} · {}", node.type_, node.label),
                };
                let id = node.id.clone();
                let holder = div()
                    .id(("graph-node", i))
                    .absolute()
                    .left(px(x - w / 2.))
                    .top(px(y - h / 2.))
                    .w(px(w))
                    .h(px(h))
                    .px(px(8. * z))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .overflow_hidden()
                    .border_1()
                    .border_color(border)
                    .when(selected, |d| d.border_2())
                    .when(node.kind == NodeKind::Entity, |d| d.rounded(px(6. * z)))
                    .when(node.kind == NodeKind::Claim, |d| d.rounded_full())
                    .bg(match node.kind {
                        NodeKind::Entity => theme.secondary,
                        NodeKind::Claim => theme.background,
                        NodeKind::Annotation => theme.popover,
                    })
                    .when(node.status == "soft-deleted", |d| d.opacity(0.45))
                    .cursor_pointer()
                    .text_size(font(1.))
                    .when(readable, |d| {
                        d.when_some(sub, |d, sub| {
                            d.child(
                                div()
                                    .text_size(font(0.85))
                                    .text_color(theme.info)
                                    .whitespace_nowrap()
                                    .child(if plain {
                                        // No confidence, no author: the plain picture.
                                        sub.clone()
                                    } else {
                                        format!(
                                            "{sub}{}{}",
                                            node.confidence
                                                .as_deref()
                                                .map(|c| format!(" · {c}"))
                                                .unwrap_or_default(),
                                            if node.by_agent { " · agent" } else { "" }
                                        )
                                    }),
                            )
                        })
                        .child(
                            div()
                                .truncate()
                                .text_color(status_colour(&node.status, theme.foreground))
                                .child(SharedString::from(text)),
                        )
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // Making a relation: this is its other end.
                            if let Some(from) = this.relating.clone()
                                && from != id
                            {
                                this.open_relation_form(vec![from, id.clone()], window, cx);
                                return;
                            }
                            // Shift or Ctrl/Cmd picks it up or puts it down.
                            let m = event.modifiers;
                            if m.shift || m.secondary() {
                                this.toggle(&id, cx);
                                return;
                            }
                            // Pressing one of several that are selected drags them all;
                            // pressing another selects just it.
                            if !this.selected_nodes().contains(&id) {
                                this.select(Some(Selected::Nodes(vec![id.clone()])), cx);
                            }
                            let from = this.to_world(this.local(event.position));
                            let starts = this
                                .selected_nodes()
                                .into_iter()
                                .filter_map(|n| this.positions.get(&n).map(|p| (n, *p)))
                                .collect();
                            this.drag = Some(Drag::Nodes { from, starts });
                        }),
                    );
                hints::tip(holder, full)
            })
            .collect();

        let name_of = |id: &str| {
            self.model
                .nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.label.clone())
                .unwrap_or_else(|| id.to_string())
        };
        let field = |label: &'static str, input: &Entity<InputState>| {
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w(px(84.))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(div().w(px(300.)).child(Input::new(input).small()))
        };
        // The known types that fit what is typed: a click takes one.
        let chips = |prefix: &'static str,
                     offered: &[(String, String)],
                     target: &Entity<InputState>,
                     next: &Entity<InputState>| {
            h_flex()
                .flex_wrap()
                .gap_1()
                .pl(px(92.))
                .max_w(px(404.))
                .children(offered.iter().enumerate().map(|(i, (name, why))| {
                    let (name, target, next) = (name.clone(), target.clone(), next.clone());
                    let chip = div().id((prefix, i)).child(
                        Button::new((prefix, i))
                            .small()
                            .outline()
                            .label(SharedString::from(name.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                target.update(cx, |s, cx| {
                                    s.set_value(SharedString::from(name.clone()), window, cx)
                                });
                                next.update(cx, |s, cx| s.focus(window, cx));
                                this.refresh_suggestions(cx);
                                cx.notify();
                            })),
                    );
                    hints::tip(chip, SharedString::from(why.clone()))
                }))
        };
        let form_buttons = |submit: &'static str| {
            h_flex()
                .gap_2()
                .child(
                    Button::new("graph-form-ok")
                        .small()
                        .primary()
                        .label(submit)
                        .on_click(cx.listener(|this, _, window, cx| this.submit_form(window, cx))),
                )
                .child(
                    Button::new("graph-form-cancel")
                        .small()
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.form = None;
                            cx.notify();
                        })),
                )
        };
        let card = || {
            v_flex()
                .id("graph-form")
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_size(small)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        };
        let title = |text: String, info: &'static str| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().text_color(theme.foreground).child(text))
                .child(hints::info_button("graph-form-info", info))
        };
        let mut anchor_at = None;
        let overlay = match &self.form {
            Some(Form::Add { mode, anchor, .. }) => {
                anchor_at = *anchor;
                let mode = *mode;
                let mode_button = |id: &'static str, label: &'static str, this_mode: AddMode| {
                    let b = Button::new(id).small().label(label);
                    let b = if mode == this_mode {
                        b.primary()
                    } else {
                        b.ghost()
                    };
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(Form::Add { mode, .. }) = &mut this.form {
                            *mode = this_mode;
                        }
                        this.focus_add(this_mode, window, cx);
                        cx.notify();
                    }))
                };
                Some(
                    card()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(mode_button("graph-add-entity", "Entity", AddMode::Entity))
                                .child(mode_button("graph-add-note", "Note", AddMode::Note))
                                .child(hints::info_button(
                                    "graph-form-info",
                                    "An entity is a thing that can show up in many places: the same value is always the same node. A note is free text, put where it was thought of, and can be connected to nodes later.",
                                )),
                        )
                        .when(mode == AddMode::Entity, |c| {
                            c.child(field("Type", &self.entity_type))
                                .child(chips(
                                    "graph-entity-type",
                                    &self.entity_suggest,
                                    &self.entity_type,
                                    &self.entity_value,
                                ))
                                .child(field("Value", &self.entity_value))
                        })
                        .when(mode == AddMode::Note, |c| {
                            c.child(field("Note", &self.entity_value))
                        })
                        .child(form_buttons("Add")),
                )
            }
            Some(Form::Relation { ids }) if ids.len() == 2 => Some(
                card()
                    .child(title(
                        format!("{} and {}", name_of(&ids[0]), name_of(&ids[1])),
                        "Roles say which end is which: src → dst, actor → target, query → answer. Recorded as an observation sourced to this session.",
                    ))
                    .child(field("Type", &self.relation_type))
                    .child(chips(
                        "graph-relation-type",
                        &self.relation_suggest,
                        &self.relation_type,
                        &self.from_role,
                    ))
                    .child(field("First is", &self.from_role))
                    .child(field("Second is", &self.to_role))
                    .child(form_buttons("Connect")),
            ),
            Some(Form::Relation { ids }) => Some(
                card()
                    .child(title(
                        format!("{} nodes", ids.len()),
                        "One claim that all of them take part in, each with the same role. Recorded as an observation sourced to this session.",
                    ))
                    .child(field("Type", &self.relation_type))
                    .child(chips(
                        "graph-relation-type",
                        &self.relation_suggest,
                        &self.relation_type,
                        &self.from_role,
                    ))
                    .child(field("Each is", &self.from_role))
                    .child(form_buttons("Connect")),
            ),
            None => self.relating.as_ref().map(|from| {
                card()
                    .child(format!(
                        "Relating {} to… click the other end.",
                        name_of(from)
                    ))
                    .child(
                        Button::new("graph-relate-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.relating = None;
                                cx.notify();
                            })),
                    )
            }),
        };
        let area = self.bounds.get().size;
        let overlay_at = match anchor_at {
            Some((x, y)) => (
                x.min(f32::from(area.width) - 440.).max(8.),
                y.min(f32::from(area.height) - 260.).max(8.),
            ),
            // Below the header card.
            None => (8., 60.),
        };

        let empty = self.model.nodes.is_empty();
        let nodes_shown = shown.iter().filter(|s| **s).count();
        let counts = format!(
            "{}{} node{} · {} line{}{}",
            // "N of M" whenever something is left out: by the filter or by the level.
            if nodes_shown == self.model.nodes.len() {
                String::new()
            } else {
                format!("{nodes_shown} of ")
            },
            self.model.nodes.len(),
            if self.model.nodes.len() == 1 { "" } else { "s" },
            self.model.lines.len(),
            if self.model.lines.len() == 1 { "" } else { "s" },
            if (self.zoom - 1.).abs() > 0.01 {
                format!(" · {:.0}%", self.zoom * 100.)
            } else {
                String::new()
            }
        );

        let is_single = self.single().is_some();
        let picked = self.selected_nodes();
        let is_node = picked.len() == 1;
        let several = picked.len() >= 2;
        let is_hypothesis = match picked.as_slice() {
            [id] => self
                .model
                .nodes
                .iter()
                .any(|n| n.id == *id && n.type_ == "analysis.hypothesis"),
            _ => false,
        };
        let detail = (self.selected.is_some() && !self.detail.is_empty()).then(|| {
            // A two-column list: the labels in a fixed column, the values wrapping in
            // the rest, so nothing runs out of the box.
            let mut list = DescriptionList::new()
                .columns(1)
                .bordered(false)
                .label_width(px(DETAIL_LABEL))
                .small();
            for (i, element) in self.detail.iter().enumerate() {
                if i > 0 {
                    list = list.separator();
                }
                for (label, value) in element {
                    list = list.item(
                        SharedString::from(label.clone()),
                        // A width in pixels, not "the rest": text is measured before
                        // the row is laid out, and with no width to wrap in it reports
                        // one line's height and then paints over what is below it.
                        div()
                            .w(px(DETAIL_VALUE))
                            .flex_none()
                            .child(SharedString::from(value.clone()))
                            .into_any_element(),
                        1,
                    );
                }
            }
            v_flex()
                .id("graph-detail")
                .absolute()
                .left_2()
                .bottom_2()
                .w(px(DETAIL_WIDTH))
                .p_2()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    // Its own height, up to a limit, then it scrolls. Not `flex_1`: in a
                    // box whose height comes from its content that share is nothing, and
                    // the list would be drawn zero pixels tall.
                    div()
                        .w_full()
                        .max_h(px(340.))
                        .pb_1()
                        .text_size(small)
                        .overflow_y_scrollbar()
                        .child(list),
                )
                .children(self.pending_undo.as_ref().map(|(_, text)| {
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.warning)
                        .text_size(small)
                        .child(
                            div()
                                .w(px(DETAIL_WIDTH - 40.))
                                .child(SharedString::from(text.clone())),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("graph-undo-yes")
                                        .small()
                                        .danger()
                                        .label("Take it back")
                                        .on_click(cx.listener(|this, _, _, cx| this.confirm_undo(cx))),
                                )
                                .child(
                                    Button::new("graph-undo-no")
                                        .small()
                                        .ghost()
                                        .label("Keep it")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.pending_undo = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                }))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child({
                            let (focus, scale) = (self.focus.clone(), self.scale.clone());
                            Button::new("graph-confidence")
                                .small()
                                .label("Confidence")
                                .tooltip("How sure: calibrated wording, kept as a new version")
                                .dropdown_menu(move |mut menu, _, _| {
                                    menu = menu.action_context(focus.clone());
                                    for value in &scale {
                                        menu = menu.menu(
                                            value.clone(),
                                            Box::new(GraphSetConfidence {
                                                value: value.clone(),
                                            }),
                                        );
                                    }
                                    menu
                                })
                        })
                        .child(
                            Button::new("graph-dispute")
                                .small()
                                .label("Dispute")
                                .tooltip("Mark it conflicting: it stays visible, flagged. Say why in a note")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_status("conflicting", cx)
                                })),
                        )
                        .child(
                            Button::new("graph-restore")
                                .small()
                                .label("Restore")
                                .tooltip("Mark it active again, whatever it was")
                                .on_click(cx.listener(|this, _, _, cx| this.set_status("active", cx))),
                        )
                        .when(is_hypothesis, |row| {
                            let focus = self.focus.clone();
                            row.child(
                                Button::new("graph-hypothesis")
                                    .small()
                                    .label("Hypothesis")
                                    .tooltip("Open, supported or rejected")
                                    .dropdown_menu(move |mut menu, _, _| {
                                        menu = menu.action_context(focus.clone());
                                        for status in edit::HYPOTHESIS_STATUSES {
                                            menu = menu.menu(
                                                status,
                                                Box::new(GraphSetHypothesis {
                                                    status: status.to_string(),
                                                }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                        })
                        .when(is_node, |row| {
                            row.child(
                                Button::new("graph-relate")
                                    .small()
                                    .label("Relate to…")
                                    .tooltip("State a relation to another element: then click the other end")
                                    .on_click(cx.listener(|this, _, _, cx| this.start_relating(cx))),
                            )
                        })
                        .when(several, |row| {
                            row.child(
                                Button::new("graph-connect")
                                    .small()
                                    .label("Connect…")
                                    .tooltip("State a relation between the selected nodes: two get a role each, more are members of one claim")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let ids = this.selected_nodes();
                                        this.open_relation_form(ids, window, cx)
                                    })),
                            )
                        })
                        .when(is_single, |row| {
                            row.child(
                                Button::new("graph-undo")
                                    .small()
                                    .label("Undo last change")
                                    .tooltip("Take back its latest version. Previews what would go, and is refused if something builds on it")
                                    .on_click(cx.listener(|this, _, _, cx| this.preview_undo(cx))),
                            )
                            .child(
                                Button::new("graph-source")
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label("Source")
                                    .tooltip("Open where this came from: the file, the page, or the session it was said in")
                                    .on_click(cx.listener(|this, _, window, cx| this.open_source(window, cx))),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(300.)).child(Input::new(&self.tag).small()))
                        .child(
                            Button::new("graph-tag")
                                .small()
                                .label("Tag")
                                .tooltip("Add this tag to every selected node")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.tag_selection(true, window, cx)
                                })),
                        )
                        .child(
                            Button::new("graph-untag")
                                .small()
                                .ghost()
                                .label("Untag")
                                .tooltip("Take this tag off every selected node")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.tag_selection(false, window, cx)
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(300.)).child(Input::new(&self.note).small()))
                        .child(
                            Button::new("graph-note")
                                .small()
                                .label("Add note")
                                .tooltip("Attach what you typed as one note about everything selected — recorded as yours")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_note(window, cx)
                                })),
                        )
                        .child(
                            Button::new("graph-delete")
                                .small()
                                .danger()
                                .label("Soft-delete")
                                .tooltip("Hide it. Nothing is lost: it is a new version, and the eye in the toolbar shows it again")
                                .on_click(cx.listener(|this, _, _, cx| this.soft_delete(cx))),
                        )
                        .child(
                            Button::new("graph-deselect")
                                .small()
                                .ghost()
                                .icon(IconName::X)
                                .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
                        ),
                )
        });

        let marquee = match self.drag {
            Some(Drag::Marquee { from, to }) => Some(
                div()
                    .absolute()
                    .left(px(from.0.min(to.0)))
                    .top(px(from.1.min(to.1)))
                    .w(px((from.0 - to.0).abs()))
                    .h(px((from.1 - to.1).abs()))
                    .border_1()
                    .border_color(theme.primary)
                    .bg(theme.primary.opacity(0.1)),
            ),
            _ => None,
        };

        div()
            .id("graph-area")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(theme.background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    let at = this.local(event.position);
                    // Shift sweeps out an area to select; otherwise the plane is dragged.
                    this.drag = Some(if event.modifiers.shift {
                        Drag::Marquee { from: at, to: at }
                    } else {
                        Drag::View {
                            from: at,
                            pan: this.pan,
                        }
                    });
                    this.select(None, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    // Add a node exactly where the pointer is.
                    let at = this.local(event.position);
                    let spot = Spot::Exact(this.to_world(at));
                    this.open_add_form(AddMode::Entity, spot, Some(at), window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    this.end_drag();
                    return;
                }
                let at = this.local(event.position);
                match &this.drag {
                    Some(Drag::View { from, pan }) => {
                        this.pan = (pan.0 + at.0 - from.0, pan.1 + at.1 - from.1);
                    }
                    Some(Drag::Nodes { from, starts }) => {
                        let w = this.to_world(at);
                        let (dx, dy) = (w.0 - from.0, w.1 - from.1);
                        let moved: Vec<(String, (f32, f32))> = starts
                            .iter()
                            .map(|(id, p)| (id.clone(), (p.0 + dx, p.1 + dy)))
                            .collect();
                        this.positions.extend(moved);
                    }
                    Some(Drag::Marquee { from, .. }) => {
                        let from = *from;
                        this.drag = Some(Drag::Marquee { from, to: at });
                        let (lo, hi) = (
                            (from.0.min(at.0), from.1.min(at.1)),
                            (from.0.max(at.0), from.1.max(at.1)),
                        );
                        let inside: Vec<String> = this
                            .model
                            .nodes
                            .iter()
                            .filter_map(|n| {
                                let p = this.to_view(this.positions.get(&n.id).copied()?);
                                (p.0 >= lo.0 && p.0 <= hi.0 && p.1 >= lo.1 && p.1 <= hi.1)
                                    .then(|| n.id.clone())
                            })
                            .collect();
                        this.selected = (!inside.is_empty()).then_some(Selected::Nodes(inside));
                        this.describe();
                    }
                    None => return,
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.end_drag()),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui_kit::ScrollWheelEvent, _, cx| {
                let steps = match event.delta {
                    ScrollDelta::Lines(l) => l.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.,
                };
                if steps == 0. {
                    return;
                }
                // Zoom about the pointer: what is under it stays under it.
                let at = this.local(event.position);
                let world = this.to_world(at);
                this.zoom = (this.zoom * 1.12f32.powf(steps)).clamp(MIN_ZOOM, MAX_ZOOM);
                let now = this.to_view(world);
                this.pan = (this.pan.0 + at.0 - now.0, this.pan.1 + at.1 - now.1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, a: &GraphSetConfidence, _, cx| {
                this.set_confidence(&a.value, cx)
            }))
            .on_action(cx.listener(|this, a: &GraphSetHypothesis, _, cx| {
                this.set_hypothesis(&a.status, cx)
            }))
            .on_action(cx.listener(|this, a: &GraphSetDetail, _, cx| {
                this.detail_pick = match a.level.as_str() {
                    "basic" => Some(model::Detail::Basic),
                    "analysis" => Some(model::Detail::Analysis),
                    "full" => Some(model::Detail::Full),
                    _ => None,
                };
                cx.notify();
            }))
            .on_action(cx.listener(|this, a: &GraphSetLayout, _, cx| {
                if let Some(kind) = LayoutKind::parse(&a.kind) {
                    this.set_layout(kind, cx);
                }
            }))
            .children(hull_labels)
            .child(lines)
            .children(labels)
            .children(nodes)
            .children(marquee)
            .children(overlay.map(|o| {
                div()
                    .absolute()
                    .left(px(overlay_at.0))
                    .top(px(overlay_at.1))
                    .child(o)
            }))
            .child(
                h_flex()
                    .id("graph-header")
                    .absolute()
                    // Off the edges, and a card of its own: what is drawn behind it (a tag
                    // box's label, a node) does not show through, and a click on it is not
                    // a click on the plane.
                    .top_3()
                    .left_3()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .text_size(small)
                    .text_color(theme.muted_foreground)
                    .child(hints::info_button(
                        "graph-help",
                        "Click selects; Ctrl/Cmd+click adds a node to the selection or takes it out; Shift+drag sweeps an area; dragging a selected node moves them all. Right-click the plane adds a node there. Scroll zooms (everything shrinks with it); drag the plane to pan. Relations show as lines: click the word on one to select it. Dashed lines are notes.",
                    ))
                    .child(counts)
                    .child({
                        // How the graph is arranged: a menu of the ways, the current one marked.
                        let (focus, current) = (self.focus.clone(), self.layout_kind);
                        hints::tip(
                            div().id("graph-layout-holder").child(
                                Button::new("graph-layout")
                                    .small()
                                    .icon(IconName::LayoutDashboard)
                                    .label(current.label())
                                    .dropdown_menu(move |mut menu, _, _| {
                                        menu = menu.action_context(focus.clone());
                                        for kind in LayoutKind::ALL {
                                            let mark = if kind == current { "● " } else { "   " };
                                            menu = menu.menu(
                                                format!("{mark}{}", kind.label()),
                                                Box::new(GraphSetLayout {
                                                    kind: kind.slug().to_string(),
                                                }),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                            SharedString::from(format!(
                                "How the graph is arranged. {}. Picking one arranges the whole graph afresh",
                                current.tip()
                            )),
                        )
                    })
                    .child({
                        // How much is drawn: a level, or "auto", which follows the zoom.
                        let (focus, pick, now) = (self.focus.clone(), self.detail_pick, level);
                        let label = match pick {
                            Some(level) => format!("Detail: {}", level.label()),
                            None => format!("Detail: auto · {}", now.label().to_lowercase()),
                        };
                        hints::tip(
                            div().id("graph-detail-holder").child(
                                Button::new("graph-detail")
                                    .small()
                                    .icon(IconName::Layers)
                                    .label(label)
                                    .dropdown_menu(move |mut menu, _, _| {
                                        menu = menu.action_context(focus.clone());
                                        let mark = |on: bool| if on { "● " } else { "   " };
                                        menu = menu.menu(
                                            format!("{}Auto (by zoom)", mark(pick.is_none())),
                                            Box::new(GraphSetDetail { level: "auto".into() }),
                                        );
                                        for level in [
                                            model::Detail::Basic,
                                            model::Detail::Analysis,
                                            model::Detail::Full,
                                        ] {
                                            menu = menu.menu(
                                                format!("{}{}", mark(pick == Some(level)), level.label()),
                                                Box::new(GraphSetDetail {
                                                    level: level.label().to_lowercase(),
                                                }),
                                            );
                                        }
                                        menu
                                    }),
                            ),
                            SharedString::from(format!(
                                "How much is drawn. Basic: {}. Analysis: {}. Full: {}. Auto follows the zoom: zoomed out the overview, zoomed in the detail",
                                model::Detail::Basic.tip().to_lowercase(),
                                model::Detail::Analysis.tip().to_lowercase(),
                                model::Detail::Full.tip().to_lowercase(),
                            )),
                        )
                    })
                    .child(div().w(px(220.)).child(Input::new(&self.filter).small()))
                    .child(hints::info_button(
                        "graph-filter-help",
                        "Show only some of the nodes. Words must all be in a node's label or type. type:net. (a type that starts so, or has a dotted part that does: type:ipv4), tag:phase. (a tag that starts so), by:agent or by:person, kind:entity, kind:claim or kind:note, status:conflicting. A leading - means not: -tag:c2. Lines are drawn between nodes that are shown.",
                    )),
            )
            .when(empty, |d| {
                d.child(
                    h_flex()
                        .absolute()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_color(theme.muted_foreground)
                        .child("Nothing in the graph yet")
                        .child(hints::info_button(
                            "graph-empty-info",
                            "It fills as the agent records what it finds, or pulls rows in from a data table. Right-click the plane to add a node by hand.",
                        )),
                )
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .absolute()
                        .top_2()
                        .right_2()
                        .max_w(px(520.))
                        .p_2()
                        .rounded_md()
                        .bg(theme.popover)
                        .text_size(small)
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .children(detail)
    }
}
