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
use gpui_kit::component::dock::{Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyBinding,
    MouseButton, PathBuilder, Pixels, Point, ScrollDelta, SharedString, Window, actions, canvas,
    div, point, px,
};
use reactor_db::graph::edit::{self, Annotate, AnnotationKind, At, TypeChoice};
use reactor_db::graph::fields::{self, Fields};
use reactor_db::graph::layout;
use reactor_db::graph::views::{self, SavedView};
use reactor_db::graph::vocab::{self, Vocabulary};
use reactor_db::graph::{self, query};
use serde::Deserialize;

use super::arrange::{self, LayoutKind};
use super::card::{Metrics, Size};
use super::inspector::{self, Inspected};
use super::look;
use super::model::{self, Line, LineKind, Model, SourceTarget};
use super::sift::{self, Found, Mode};
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

/// A standing picked from the inspector's status menu: `active`, `conflicting` (disputed) or
/// `soft-deleted`.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct GraphSetStatus {
    pub status: String,
}

/// A hypothesis status picked from the detail box's menu.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = reactor_gui, no_json)]
pub struct GraphSetHypothesis {
    pub status: String,
}

actions!(
    reactor_graph,
    [
        /// Ctrl/Cmd+F on the plane: find nodes by what is written on them or known of them.
        GraphFind,
        /// `[`: the selected nodes say less (every node, when none is selected).
        GraphSmaller,
        /// `]`: they say more.
        GraphLarger,
        /// `0`: back to the size and width that follow from what a node says.
        GraphAutoSize,
        /// Esc: close what is open over the plane.
        GraphDismiss,
        /// Ctrl/Cmd+N: add a node of any kind by hand, from a palette.
        GraphNewNode,
        /// Up and down in the add palette's type field.
        GraphAddUp,
        GraphAddDown
    ]
);

const CONTEXT: &str = "ReactorGraph";

/// The plane's keys. The size keys are the plane's only while nothing in it is being typed
/// into; find and Esc are its own in its inputs too, over what an input does with them.
pub fn bind_keys(cx: &mut App) {
    let typing = "ReactorGraph > Input";
    let plane = "ReactorGraph && !Input";
    cx.bind_keys([
        KeyBinding::new("secondary-f", GraphFind, Some(CONTEXT)),
        KeyBinding::new("secondary-f", GraphFind, Some(typing)),
        KeyBinding::new("secondary-n", GraphNewNode, Some(CONTEXT)),
        KeyBinding::new("secondary-n", GraphNewNode, Some(typing)),
        KeyBinding::new("up", GraphAddUp, Some(typing)),
        KeyBinding::new("down", GraphAddDown, Some(typing)),
        KeyBinding::new("escape", GraphDismiss, Some(CONTEXT)),
        KeyBinding::new("escape", GraphDismiss, Some(typing)),
        KeyBinding::new("[", GraphSmaller, Some(plane)),
        KeyBinding::new("]", GraphLarger, Some(plane)),
        KeyBinding::new("0", GraphAutoSize, Some(plane)),
    ]);
}

/// What [`GraphPanel::plane_for_test`] reports.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneForTest {
    pub size: &'static str,
    pub find_open: bool,
    pub found: usize,
    pub find_at: usize,
    pub drawn: usize,
    pub layers: usize,
    pub layers_open: bool,
    pub views: Vec<String>,
    pub legend_open: bool,
    pub picking: Option<String>,
}

/// A filter layer as it is kept: what is typed in it, what it does, and whether it is on.
struct LayerRow {
    input: Entity<InputState>,
    mode: Mode,
    on: bool,
}

/// The card a type's icon and colour are chosen on, while it is open.
struct Picker {
    type_: String,
    kind: graph::Kind,
    /// Where it sits, in the drawing area: at the pointer.
    at: (f32, f32),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddMode {
    /// A node of this kind and of the type that was chosen.
    Node(graph::Kind),
    /// A free note, put where it was thought of.
    Note,
    /// Any other kind of annotation: what it is about is what is selected.
    Annotation(AnnotationKind),
}

/// What an assessment does to a hypothesis that is selected with it.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stance {
    /// Only comments on what it is about.
    #[default]
    Plain,
    Supports,
    Refutes,
}

/// A form over the drawing: adding an entity or a note, or stating a relation.
#[derive(Clone)]
enum Form {
    Add {
        spot: Spot,
        /// Where the card sits, in the drawing area: at the pointer for a right click.
        anchor: Option<(f32, f32)>,
        /// The small form of a right click, rather than the palette.
        compact: bool,
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
    /// A node's width: where the pointer went down (world x), how wide the node was, and
    /// whether moving right makes it wider (it was taken by its right half).
    Width {
        id: String,
        from: f32,
        start: f32,
        grows: bool,
    },
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
    /// The selected element as the inspector shows it, when the selection is exactly one.
    inspected: Option<Inspected>,
    /// How wide the inspector was drawn, in pixels: what a value wraps in.
    dock_width: Rc<Cell<f32>>,
    /// Which tab of the inspector is open (an index into the tabs it has for this element).
    inspector_tab: usize,
    /// The reader and editor popup, while it is open.
    reader: Option<Reader>,
    /// A property being added or edited in the inspector's *Details*.
    prop_edit: Option<inspector_ui::PropEdit>,
    show_deleted: bool,
    error: Option<String>,
    /// Where the drawing area is in the window, as of the last paint: pointer events
    /// arrive in window coordinates.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    note: Entity<InputState>,
    tag: Entity<InputState>,
    /// How the vocabulary says types are drawn: their icons and colours.
    vocabulary: Vocabulary,
    /// How property names are drawn and read (ADR-0067).
    fields: Fields,
    /// How much a node says, unless it has a size of its own.
    size: Size,
    /// The nodes with a size or a width of their own.
    own_size: HashMap<String, (Option<Size>, Option<f32>)>,
    /// How wide text runs in the fonts last drawn with: what a node's size is worked out from.
    metrics: Rc<Cell<Metrics>>,
    /// The filter layers, top to bottom, and whether their panel is open.
    layers: Vec<LayerRow>,
    layers_open: bool,
    /// The views kept with the case, and the name being typed for a new one (while saving).
    views: Vec<SavedView>,
    view_name: Entity<InputState>,
    saving_view: bool,
    /// Find: whether it is open, what is typed, where it looks, the matches (a node's id and
    /// where in it) in reading order, and which of them it is at.
    find_open: bool,
    find_input: Entity<InputState>,
    find_scope: sift::Scope,
    found: Vec<(String, Found)>,
    find_at: usize,
    legend_open: bool,
    /// A type's icon and colour being chosen, and what is typed to find an icon.
    picker: Option<Picker>,
    picker_input: Entity<InputState>,
    /// Draw an outline round the nodes that share a tag.
    show_groups: bool,
    /// How the graph is arranged: what *auto-layout* does, and how lines are drawn.
    layout_kind: LayoutKind,
    /// How much is drawn: Full until another level is picked, or `None` (auto) to follow the zoom.
    detail_pick: Option<model::Detail>,
    /// What the assessment being added does to a hypothesis selected with it.
    stance: Stance,
    /// The form that is open, and its fields.
    form: Option<Form>,
    entity_type: Entity<InputState>,
    entity_value: Entity<InputState>,
    /// Where a search looked, in the add form.
    add_where: Entity<InputState>,
    /// The tag field is open in the inspector; and the tag whose colour is being chosen.
    tag_adding: bool,
    tag_colouring: Option<String>,
    /// The note field is open in the inspector.
    note_open: bool,
    /// What can be added, for what is typed in the add form, and which of it is picked.
    add_rows: Vec<add_ui::AddRow>,
    add_at: usize,
    /// Every type there is for what a person can add, with how much it is used.
    add_choices: Vec<TypeChoice>,
    /// What a type that is not known yet will be a type of.
    add_new_kind: graph::Kind,
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

mod add_ui;
mod inspector_ui;
mod node_ui;
mod prop_draw;
mod tags_ui;
mod tools_ui;
use inspector_ui::Reader;

/// How far the view zooms: out far enough that a large graph is one picture, because
/// everything on it shrinks with it.
const MIN_ZOOM: f32 = 0.04;
const MAX_ZOOM: f32 = 3.;

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
        let tag = cx.new(|cx| InputState::new(window, cx).placeholder("tag, e.g. phase.recon"));
        let entity_type = cx.new(|cx| {
            InputState::new(window, cx).placeholder("a type, or what to add: agent, ipv4, note…")
        });
        let entity_value = cx.new(|cx| InputState::new(window, cx).placeholder("name"));
        let add_where =
            cx.new(|cx| InputState::new(window, cx).placeholder("where: a file, a URL…"));
        let relation_type =
            cx.new(|cx| InputState::new(window, cx).placeholder("type, e.g. net.connection"));
        for input in [&entity_type, &relation_type] {
            cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.refresh_suggestions(cx);
                    if matches!(this.form, Some(Form::Add { .. })) {
                        // Typing starts the list over, at its best.
                        this.add_at = 0;
                        this.rebuild_add_rows(cx);
                    }
                }
            })
            .detach();
        }
        cx.subscribe_in(
            &entity_type,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { secondary, .. } = event {
                    this.add_choose(*secondary, window, cx);
                }
            },
        )
        .detach();
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
        cx.subscribe_in(
            &tag,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_typed_tag(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )
        .detach();
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder("find in the graph"));
        cx.subscribe_in(
            &find_input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.find_changed(cx),
                InputEvent::PressEnter { shift, .. } => this.find_step(!*shift, cx),
                _ => {}
            },
        )
        .detach();
        let view_name = cx.new(|cx| InputState::new(window, cx).placeholder("name this view"));
        cx.subscribe_in(
            &view_name,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => this.save_view(cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )
        .detach();
        let picker_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("find an icon: shield, key…"));
        cx.subscribe_in(&picker_input, window, |_, _, event: &InputEvent, _, cx| {
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
            inspected: None,
            inspector_tab: 0,
            dock_width: Rc::new(Cell::new(inspector_ui::DOCK_WIDTH)),
            reader: None,
            prop_edit: None,
            show_deleted: false,
            error: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            note: cx.new(|cx| InputState::new(window, cx).placeholder("a note about this…")),
            tag,
            vocabulary: Vocabulary::default(),
            fields: Fields::default(),
            size: Size::default(),
            own_size: HashMap::new(),
            metrics: Rc::new(Cell::new(Metrics::default())),
            layers: Vec::new(),
            layers_open: false,
            views: Vec::new(),
            view_name,
            saving_view: false,
            find_open: false,
            find_input,
            find_scope: sift::Scope::default(),
            found: Vec::new(),
            find_at: 0,
            legend_open: false,
            picker: None,
            picker_input,
            show_groups: false,
            layout_kind: LayoutKind::default(),
            detail_pick: Some(model::Detail::Full),
            stance: Stance::default(),
            form: None,
            entity_type,
            entity_value,
            add_where,
            tag_adding: false,
            tag_colouring: None,
            note_open: false,
            add_rows: Vec::new(),
            add_at: 0,
            add_choices: Vec::new(),
            add_new_kind: graph::Kind::Entity,
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

    /// Select two nodes and fill the inspector, without a case: for the tests that drive the
    /// real panel in a headless window (`tests/graph_click.rs`).
    #[doc(hidden)]
    pub fn select_for_test(&mut self, cx: &mut Context<Self>) {
        self.selected = Some(Selected::Nodes(vec!["a".into(), "b".into()]));
        self.detail = vec![vec![("Selected".into(), "2 nodes".into())]];
        cx.notify();
    }

    /// Select one element by id and show it, as a click on its node does: what a reference
    /// to it opens (ADR-0066).
    pub fn select_element(&mut self, id: &str, cx: &mut Context<Self>) {
        self.select(Some(Selected::Nodes(vec![id.to_string()])), cx);
    }

    /// Select one element by id, as a click on its node does.
    #[doc(hidden)]
    pub fn select_id_for_test(&mut self, id: &str, cx: &mut Context<Self>) {
        self.select(Some(Selected::Nodes(vec![id.to_string()])), cx);
    }

    /// What is open over the plane and what it shows, for the tests that drive it:
    /// the view's size, whether find is open, how many it found and which it is at, how many
    /// nodes are drawn, whether the legend is open, and the type whose look is being chosen.
    #[doc(hidden)]
    pub fn plane_for_test(&self, cx: &App) -> PlaneForTest {
        PlaneForTest {
            size: self.size.label(),
            find_open: self.find_open,
            found: self.found.len(),
            find_at: self.find_at,
            drawn: self.shown(cx).iter().filter(|s| **s).count(),
            layers: self.layers.len(),
            layers_open: self.layers_open,
            views: self.views.iter().map(|v| v.name.clone()).collect(),
            legend_open: self.legend_open,
            picking: self.picker.as_ref().map(|p| p.type_.clone()),
        }
    }

    /// What the add form lists for what is typed, and which line it is on: `None` while it is
    /// closed.
    #[doc(hidden)]
    pub fn add_for_test(&self) -> Option<(Vec<String>, usize)> {
        matches!(self.form, Some(Form::Add { .. })).then(|| {
            (
                self.add_rows
                    .iter()
                    .map(|r| {
                        if r.new {
                            format!("new:{}", r.type_)
                        } else {
                            r.label.clone()
                        }
                    })
                    .collect(),
                self.add_at,
            )
        })
    }

    /// The element the inspector shows: its type and its title.
    #[doc(hidden)]
    pub fn inspected_for_test(&self) -> Option<(String, String)> {
        self.inspected
            .as_ref()
            .map(|i| (i.type_.clone(), i.title.clone()))
    }

    /// Where a node is drawn in the window, and how big: its centre, its width and height.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    pub fn node_for_test(&self, id: &str) -> Option<((f32, f32), (f32, f32), &'static str)> {
        let node = self.model.nodes.iter().find(|n| n.id == id)?;
        let (size, width) = self.size_of(id);
        let card = super::card::card(node, size, width, false, &self.metrics.get());
        let at = self.to_view(*self.positions.get(id)?);
        let o = self.bounds.get().origin;
        Some((
            (f32::from(o.x) + at.0, f32::from(o.y) + at.1),
            (card.w * self.zoom, card.h * self.zoom),
            size.label(),
        ))
    }

    /// Put the plane in a state by name, as the pointer and the keys would: `find:<words>`,
    /// `legend`, `size:<s|m|l|xl>`, `own:<id>:<size>`, `pick:<id>`, `select:<id>`, `fit`, `goto:<id>`, `saveview:<name>`.
    #[doc(hidden)]
    pub fn stage_for_test(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (name, rest) = what.split_once(':').unwrap_or((what, ""));
        match name {
            "find" => {
                self.open_find(window, cx);
                self.find_input.update(cx, |s, cx| {
                    s.set_value(SharedString::from(rest.to_string()), window, cx)
                });
                self.find_changed(cx);
            }
            "legend" => self.legend_open = true,
            "focus" => window.focus(&self.focus, cx),
            // `addtype:<text>` types into the add form's type field.
            "addtype" => {
                self.entity_type.update(cx, |s, cx| {
                    s.set_value(SharedString::from(rest.to_string()), window, cx)
                });
                self.add_at = 0;
                self.rebuild_add_rows(cx);
            }
            // `tag:<tag>` puts a tag on what is selected, `untag:<tag>` takes it off, and
            // `tagcolour:<tag>:<colour>` gives it one (`default` takes it back).
            "tag" => self.change_tags(&[rest.to_string()], &[], cx),
            "untag" => self.change_tags(&[], &[rest.to_string()], cx),
            "tagcolour" => {
                let (tag, colour) = rest.split_once(':').expect("tagcolour:<tag>:<colour>");
                self.set_tag_colour(tag, (colour != "default").then_some(colour), cx);
            }
            "size" => {
                self.size = Size::parse(rest).expect("a size");
                self.make_room(&[]);
            }
            "own" => {
                let (id, size) = rest.split_once(':').expect("own:<id>:<size>");
                self.own_size
                    .insert(id.to_string(), (Size::parse(size), None));
                self.make_room(&[id.to_string()]);
            }
            "pick" => {
                if let Some(node) = self.model.nodes.iter().find(|n| n.id == rest) {
                    let at = self
                        .positions
                        .get(rest)
                        .map(|p| self.to_view(*p))
                        .unwrap_or((40., 40.));
                    self.picker = Some(Picker {
                        type_: node.type_.clone(),
                        kind: node.element,
                        at: (at.0 + 30., at.1 + 30.),
                    });
                }
            }
            "select" => self.select(Some(Selected::Nodes(vec![rest.to_string()])), cx),
            "fit" => self.fit(cx),
            "saveview" => self.save_view_for_test(rest, window, cx),
            "goto" => {
                if let Some(p) = self.positions.get(rest).copied() {
                    self.zoom = 1.;
                    self.pan = (-p.0, -p.1);
                }
            }
            other => panic!("no such stage: {other}"),
        }
        cx.notify();
    }

    /// Arrange the whole graph afresh, as the toolbar's auto-layout does.
    #[doc(hidden)]
    pub fn relayout_for_test(&mut self, cx: &mut Context<Self>) {
        self.relayout(cx);
    }

    /// Put the view at this zoom and pan, for the benchmark that times frames.
    #[doc(hidden)]
    pub fn view_for_test(&mut self, zoom: f32, pan: (f32, f32), cx: &mut Context<Self>) {
        self.zoom = zoom;
        self.pan = pan;
        cx.notify();
    }

    /// Bring everything into view, as the Fit button does.
    #[doc(hidden)]
    pub fn fit_for_test(&mut self, cx: &mut Context<Self>) {
        self.fit(cx);
    }

    /// Keep what the filter layers, size and level are now as a view, as the panel's Save does.
    #[doc(hidden)]
    pub fn save_view_for_test(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.view_name.update(cx, |s, cx| {
            s.set_value(SharedString::from(name.to_string()), window, cx)
        });
        self.save_view(cx);
    }

    /// The layers as `filter|mode|on`, for comparing with what a view put back.
    #[doc(hidden)]
    pub fn layers_for_test(&self, cx: &App) -> Vec<String> {
        self.layers
            .iter()
            .map(|r| format!("{}|{}|{}", r.input.read(cx).value(), r.mode.slug(), r.on))
            .collect()
    }

    /// Add a filter layer as the panel's button and its menu would.
    #[doc(hidden)]
    pub fn add_layer_for_test(
        &mut self,
        text: &str,
        mode: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = Mode::parse(mode).expect("a layer's mode");
        self.add_layer(text, mode, window, cx);
    }

    /// Whether the inspector has an element to show.
    #[doc(hidden)]
    pub fn has_inspected(&self) -> bool {
        self.inspected.is_some()
    }

    /// Whether something is selected, so the inspector is up.
    #[doc(hidden)]
    pub fn has_selection(&self) -> bool {
        self.selected.is_some()
    }

    /// The panel reads the case through the backend. It is handed over rather than
    /// looked up, because the app is mid-update whenever it would be asked.
    pub fn set_backend(&mut self, backend: Option<Arc<Backend>>, cx: &mut Context<Self>) {
        self.backend = backend;
        // Where the nodes were left, as stored with the case.
        if let Some(b) = &self.backend {
            self.positions = layout::load(b.case.db()).unwrap_or_default();
        }
        self.load_sizes();
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
        match self.drag.take() {
            Some(Drag::Nodes { starts, .. }) => {
                let moved: Vec<String> = starts.into_iter().map(|(id, _)| id).collect();
                self.persist(&moved);
            }
            // A width set by hand is kept, and what it now overlaps gives way.
            Some(Drag::Width { id, .. }) => {
                self.persist_sizes(std::slice::from_ref(&id));
                self.make_room(std::slice::from_ref(&id));
            }
            _ => {}
        }
    }

    /// Read the graph again and place whatever is new.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else {
            return;
        };
        let db = b.case.db();
        self.scale = edit::scale(db, "confidence").unwrap_or_default();
        self.vocabulary = vocab::load(db).unwrap_or_default();
        self.fields = fields::load(db).unwrap_or_default();
        self.views = views::list(db).unwrap_or_default();
        self.entity_choices = edit::type_choices(db, &[graph::Kind::Entity]).unwrap_or_default();
        self.add_choices = edit::type_choices(
            db,
            &[
                graph::Kind::Entity,
                graph::Kind::Event,
                graph::Kind::Observation,
                graph::Kind::Annotation,
            ],
        )
        .unwrap_or_default();
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
                self.model = model::build(&elements, &edges, &from_roles, &self.fields);
                // What has no place yet is placed now, and keeps it.
                let unplaced: Vec<String> = self
                    .model
                    .nodes
                    .iter()
                    .filter(|n| !self.positions.contains_key(&n.id))
                    .map(|n| n.id.clone())
                    .collect();
                // Nodes are as big as what they say, and the layout leaves each its room. What
                // is new makes room for itself among what was there, which stays.
                let cards = self.cards(false, &self.metrics.get());
                let sizes: Vec<(f32, f32)> = cards.iter().map(|c| (c.w, c.h)).collect();
                let mut placed = model::layout(&self.model, &self.positions, &sizes);
                if !unplaced.is_empty() {
                    let fixed: Vec<bool> = self
                        .model
                        .nodes
                        .iter()
                        .map(|n| self.positions.contains_key(&n.id))
                        .collect();
                    model::separate(&mut placed, &sizes, &fixed, arrange::MIN_GAP);
                }
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
                self.inspected = None;
                return;
            }
            Some(Selected::Nodes(ids)) => ids.clone(),
            Some(Selected::Line(claims)) => claims.clone(),
            None => {
                self.detail.clear();
                self.inspected = None;
                return;
            }
        };
        let elements: Vec<query::Element> = ids
            .iter()
            .filter_map(|id| query::get(db, id).ok().flatten())
            .collect();
        self.detail = elements.iter().map(|e| rows(db, e)).collect();
        let before = self.inspected.as_ref().map(|i| i.id.clone());
        self.inspected = match elements.as_slice() {
            [one] => Some(inspector::inspect(db, one, &self.scale)),
            _ => None,
        };
        // Another element starts at its first tab; the same one, changed, stays where it was.
        if self.inspected.as_ref().map(|i| &i.id) != before.as_ref() {
            self.inspector_tab = 0;
        }
        if self.detail.is_empty() {
            self.selected = None;
        }
    }

    fn select(&mut self, what: Option<Selected>, cx: &mut Context<Self>) {
        self.selected = what.filter(|s| !matches!(s, Selected::Nodes(ids) if ids.is_empty()));
        // A property being typed belongs to the element it was started on.
        self.prop_edit = None;
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

    /// The standing every selected element has, or `active` when they differ.
    fn common_status(&self) -> String {
        let ids = self.selected_ids();
        let mut standings = ids.iter().filter_map(|id| {
            self.model
                .nodes
                .iter()
                .find(|n| n.id == *id)
                .map(|n| n.status.clone())
        });
        match standings.next() {
            Some(first) if standings.all(|s| s == first) => first,
            _ => "active".to_string(),
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
            self.note_open = false;
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

    fn set_status(&mut self, status: &str, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        self.edit(cx, |db, at| edit::set_status(db, at, &ids, status));
    }

    fn set_confidence(&mut self, value: &str, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        self.edit(cx, |db, at| edit::set_confidence(db, at, &ids, Some(value)));
    }

    fn clear_confidence(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        self.edit(cx, |db, at| edit::set_confidence(db, at, &ids, None));
    }

    fn set_hypothesis(&mut self, status: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.single() {
            self.edit(cx, |db, at| {
                edit::set_hypothesis_status(db, at, &id, status)
            });
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

    /// Open the form for adding a node: the palette, or (from a right click) its small form.
    fn open_add_form(
        &mut self,
        spot: Spot,
        anchor: Option<(f32, f32)>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.relating = None;
        self.picker = None;
        self.form = Some(Form::Add {
            spot,
            anchor,
            compact,
        });
        for input in [&self.entity_type, &self.entity_value, &self.add_where] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.add_at = 0;
        self.rebuild_add_rows(cx);
        self.entity_type.update(cx, |s, cx| s.focus(window, cx));
        self.refresh_suggestions(cx);
        cx.notify();
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
            Form::Add { spot, .. } => {
                let value = text(&self.entity_value, cx);
                self.spot = Some(spot);
                let Some(row) = self.add_row().cloned() else {
                    self.error = Some("Pick what to add from the list first".to_string());
                    self.form = Some(form);
                    cx.notify();
                    return;
                };
                let made = match row.mode {
                    AddMode::Node(kind) => {
                        let ty = row.type_.clone();
                        self.edit(cx, |db, at| edit::add_element(db, at, kind, &ty, &value))
                    }
                    AddMode::Note => self.edit(cx, |db, at| edit::note(db, at, &[], &value)),
                    AddMode::Annotation(kind) => {
                        // What it is about is what is selected; a hypothesis among them
                        // is what an assessment supports or refutes.
                        let selected = self.selected_nodes();
                        let is_hypothesis = |id: &String| {
                            self.model
                                .nodes
                                .iter()
                                .any(|n| n.id == *id && n.type_ == "analysis.hypothesis")
                        };
                        let (hypotheses, others): (Vec<String>, Vec<String>) =
                            selected.iter().cloned().partition(|id| is_hypothesis(id));
                        let stance = if hypotheses.is_empty() || others.is_empty() {
                            Stance::Plain
                        } else {
                            self.stance
                        };
                        let (about, supports, refutes): (Vec<String>, Vec<String>, Vec<String>) =
                            match (kind, stance) {
                                (AnnotationKind::Judgement, Stance::Supports) => {
                                    (others, hypotheses, Vec::new())
                                }
                                (AnnotationKind::Judgement, Stance::Refutes) => {
                                    (others, Vec::new(), hypotheses)
                                }
                                (AnnotationKind::Judgement | AnnotationKind::SameAs, _) => {
                                    (selected.clone(), Vec::new(), Vec::new())
                                }
                                _ => (Vec::new(), Vec::new(), Vec::new()),
                            };
                        let members = if kind == AnnotationKind::Cluster {
                            selected.clone()
                        } else {
                            Vec::new()
                        };
                        // Where a search looked: words, as typed.
                        let sources: Vec<String> = if kind == AnnotationKind::SearchNegative {
                            text(&self.add_where, cx)
                                .split_whitespace()
                                .map(str::to_string)
                                .collect()
                        } else {
                            Vec::new()
                        };
                        self.edit(cx, |db, at| {
                            edit::annotate(
                                db,
                                at,
                                kind,
                                &Annotate {
                                    text: &value,
                                    about: &about,
                                    supports: &supports,
                                    refutes: &refutes,
                                    members: &members,
                                    sources: &sources,
                                    confidence: None,
                                },
                            )
                        })
                    }
                };
                match made {
                    Some(id) => {
                        for input in [&self.entity_value, &self.add_where] {
                            input.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                        // What was added opens in the inspector, to be given its text, its
                        // confidence and its tags there.
                        window.focus(&self.focus, cx);
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

    /// Lay the whole graph out afresh, forgetting where things were -- including where
    /// they were dragged to -- and bring it into view.
    fn relayout(&mut self, cx: &mut Context<Self>) {
        // Every arrangement leaves each node the room its own size takes.
        let cards = self.cards(false, &self.metrics.get());
        let sizes: Vec<(f32, f32)> = cards.iter().map(|c| (c.w, c.h)).collect();
        let placed = arrange::layout_as(&self.model, self.layout_kind, &sizes);
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

    /// Bring everything into view at a readable size: every node whole, clear of the toolbar.
    fn fit(&mut self, cx: &mut Context<Self>) {
        self.pan = (0., 0.);
        self.zoom = 1.;
        let cards = self.cards(false, &self.metrics.get());
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for (node, card) in self.model.nodes.iter().zip(&cards) {
            let Some(p) = self.positions.get(&node.id) else {
                continue;
            };
            lo = (lo.0.min(p.0 - card.w / 2.), lo.1.min(p.1 - card.h / 2.));
            hi = (hi.0.max(p.0 + card.w / 2.), hi.1.max(p.1 + card.h / 2.));
        }
        if lo.0 <= hi.0 {
            let c = self.centre();
            // Room at the edges, and at the top for the toolbar.
            let (side, top) = (36., 64.);
            let (w, h) = (hi.0 - lo.0, hi.1 - lo.1);
            if c.0 > 0. && c.1 > 0. {
                self.zoom = ((c.0 * 2. - 2. * side) / w.max(1.))
                    .min((c.1 * 2. - top - side) / h.max(1.))
                    .clamp(MIN_ZOOM, 1.);
            }
            self.pan = (
                -(lo.0 + hi.0) / 2. * self.zoom,
                -(lo.1 + hi.1) / 2. * self.zoom + (top - side) / 2.,
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
                .tooltip("Add a node of any kind by hand (Ctrl/Cmd+N) — or right-click the plane to add one right there")
                .on_click(cx.listener(|this, _, window, cx| {
                    let spot = Spot::Near(this.to_world(this.centre()));
                    this.open_add_form(spot, None, false, window, cx)
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
    /// The lengths of dash and gap, in turn; none for a solid line.
    dash: Vec<f32>,
    arrow: bool,
    /// The arrowhead's scale: the zoom.
    head: f32,
}

impl Render for GraphPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let metrics = self.measure(window, &theme);
        // Shown: what the filter layers let through and the level draws.
        let sifted = sift::sift(&self.model, &self.layers(cx));
        let shown: Vec<bool> = sifted
            .nodes
            .iter()
            .zip(&disclosure.shown)
            .map(|(d, level)| d.shown && *level)
            .collect();
        // What each node says at its size, and how big that makes it.
        let cards = self.cards(plain, &metrics);
        // What find matches, as of now: the graph and the layers may have changed under it.
        let query = self.find_query(cx);
        self.found = self.find_now(cx);
        self.find_at = self.find_at.min(self.found.len().saturating_sub(1));
        let hits: HashMap<&str, bool> = self
            .found
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (id.as_str(), i == self.find_at))
            .collect();
        let z = self.zoom;
        let font = |factor: f32| theme.font_size * 0.85 * factor * z;
        let readable = f32::from(font(1.)) >= 6.;
        let labels_shown = z >= 0.6;
        let selected_nodes = self.selected_nodes();

        // Where each node is in the drawing area, worked out once for the lines and the nodes.
        let view_at: Vec<(f32, f32)> = self
            .model
            .nodes
            .iter()
            .map(|n| self.to_view(self.positions.get(&n.id).copied().unwrap_or((0., 0.))))
            .collect();
        let selected_set: HashSet<&str> = selected_nodes.iter().map(String::as_str).collect();
        let area = self.bounds.get().size;
        let (area_w, area_h) = (f32::from(area.width), f32::from(area.height));

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
            let (a, b) = (view_at[line.from], view_at[line.to]);
            let half = |index: usize| {
                let (w, h) = (cards[index].w, cards[index].h);
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
            // What is wholly out of sight is not built, as nodes are not: the box round the
            // line and its label against the drawing area.
            if area_w > 0. {
                let (mut lo, mut hi) = (from, from);
                for p in bends.iter().chain(std::iter::once(&to)) {
                    lo = (lo.0.min(p.0), lo.1.min(p.1));
                    hi = (hi.0.max(p.0), hi.1.max(p.1));
                }
                let margin = 220. * z.max(0.6) / 2. + 40.;
                if hi.0 + margin < 0.
                    || lo.0 - margin > area_w
                    || hi.1 + margin < 0.
                    || lo.1 - margin > area_h
                {
                    continue;
                }
            }
            let selected = line_selected(line);
            let disputed = line.status == "conflicting" && !plain;
            // A disputed relation says so on its label; the lines of a disputed node only
            // take its colour, since the node says it.
            let says_disputed = disputed && matches!(line.kind, LineKind::Relation { .. });
            // What a line is shows in how it is drawn: a relation solid, a part in a claim
            // thin, what a note is about dashed -- green where it supports, red where it
            // refutes -- and anything disputed orange, in long and short dashes.
            let base = match (line.kind, line.label.as_str()) {
                (LineKind::Relation { .. }, _) => theme.foreground.opacity(0.7),
                (LineKind::Role, _) => theme.muted_foreground.opacity(0.75),
                (LineKind::Annotation, "supports") => theme.success,
                (LineKind::Annotation, "refutes") => theme.danger,
                (LineKind::Annotation, _) => theme.muted_foreground,
            };
            let colour = if selected {
                theme.primary
            } else {
                status_colour(&line.status, base)
            };
            // A line to something dimmed is dim with it.
            let colour = if sifted.nodes[line.from].dim || sifted.nodes[line.to].dim {
                colour.opacity(0.35)
            } else {
                colour
            };
            let thick = match line.kind {
                _ if selected || disputed => 2.5,
                LineKind::Role => 1.,
                _ => 1.5,
            };
            strokes.push(Stroke {
                from,
                bends: bends.clone(),
                to,
                colour,
                width: (thick * z).max(if selected { 1.5 } else { 0.8 }),
                head: z,
                dash: if disputed {
                    vec![
                        10. * z.max(0.4),
                        4. * z.max(0.4),
                        2. * z.max(0.4),
                        4. * z.max(0.4),
                    ]
                } else if line.kind == LineKind::Annotation {
                    vec![5., 4.]
                } else {
                    Vec::new()
                },
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
            let (lw, lh) = (220. * z.max(0.6), 18. * z.max(0.6));
            // How sure a relation is, where its claims agree on it.
            let sure = line
                .confidence
                .as_ref()
                .filter(|_| !plain)
                .and_then(|c| self.scale.iter().position(|s| s == c))
                .map(|i| i + 1);
            let label = if labels_shown {
                let unit = f32::from(font(1.));
                h_flex()
                    .gap(px(unit * 0.35))
                    .items_center()
                    .px_1()
                    .rounded_sm()
                    .bg(theme.background.opacity(0.9))
                    .when(says_disputed, |d| {
                        d.border_1()
                            .border_dashed()
                            .border_color(theme.warning)
                            .child(node_ui::icon(
                                look::icon_path("triangle-alert"),
                                unit,
                                theme.warning,
                            ))
                    })
                    .text_size(font(1.))
                    .text_color(colour)
                    .whitespace_nowrap()
                    .when(is_relation, |d| d.cursor_pointer())
                    .child(text.clone())
                    .when(says_disputed, |d| d.child("· disputed"))
                    .when_some(sure, |d, n| {
                        d.child(node_ui::meter(
                            n,
                            self.scale.len().max(n),
                            colour,
                            theme.border,
                            unit,
                        ))
                    })
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
            let sizes: Vec<(f32, f32)> = cards.iter().map(|c| (c.w, c.h)).collect();
            for g in model::tag_groups(&self.model, &self.positions, &sizes, &shown, 12) {
                // The same tag is the same colour, whichever other tags are on; one that
                // was given a colour is drawn in it.
                let colour = look::tag_colour(&self.vocabulary, &g.tag, theme.is_dark());
                // Round the boxes of the nodes, each as big as it is drawn, standing well off them.
                let outline: Vec<(f32, f32)> = model::padded_hull(&g.boxes, 46.)
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
                    if !s.dash.is_empty() {
                        let dash: Vec<Pixels> = s.dash.iter().map(|d| px(*d)).collect();
                        path = path.dash_array(&dash);
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

        // Nodes: a card each, as big as what it says. What is out of sight is not built.
        let alone = match selected_nodes.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        };
        let mut nodes = Vec::new();
        for (i, node) in self.model.nodes.iter().enumerate() {
            if !shown[i] {
                continue;
            }
            let at = view_at[i];
            let (w, h) = (cards[i].w * z, cards[i].h * z);
            if area_w > 0.
                && (at.0 + w < -40.
                    || at.0 - w > area_w + 40.
                    || at.1 + h < -40.
                    || at.1 - h > area_h + 40.)
            {
                continue;
            }
            let look = look::look(&self.vocabulary, &node.type_, node.element);
            let selected = selected_set.contains(node.id.as_str());
            nodes.push(self.node_element(
                node,
                &look,
                node_ui::NodeView {
                    index: i,
                    card: &cards[i],
                    at,
                    zoom: z,
                    plain,
                    readable,
                    selected,
                    alone: readable && alone.as_deref() == Some(node.id.as_str()),
                    drawn: sifted.nodes[i],
                    hit: hits.get(node.id.as_str()).copied(),
                    query: query.as_deref(),
                },
                cx,
            ));
        }

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
            Some(Form::Add {
                anchor, compact, ..
            }) => {
                anchor_at = *anchor;
                Some(self.add_card(*compact, &theme, cx))
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
            // The palette sits in the middle of the plane, a little above its centre.
            None if matches!(self.form, Some(Form::Add { .. })) => (
                ((f32::from(area.width) - add_ui::PALETTE_WIDTH) / 2.).max(8.),
                ((f32::from(area.height) - 420.) / 2.).max(8.),
            ),
            // Below the header card.
            None => (8., 60.),
        };

        let empty = self.model.nodes.is_empty();
        let nodes_shown = shown.iter().filter(|s| **s).count();
        // Nodes no line reaches sit apart from the rest (in rows, at the edge of the plane):
        // say how many, so that a tall pile of them is not mistaken for the graph itself.
        let unconnected = {
            let mut touched = vec![false; self.model.nodes.len()];
            for l in &self.model.lines {
                touched[l.from] = true;
                touched[l.to] = true;
            }
            touched.iter().filter(|t| !**t).count()
        };
        let counts = format!(
            "{}{} node{} · {} line{}{}{}",
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
            if unconnected > 0 && unconnected < self.model.nodes.len() {
                format!(" · {unconnected} unconnected")
            } else {
                String::new()
            },
            // Text is drawn only from about 50%: say so, so the percentage is not read as
            // something about the layout.
            if (self.zoom - 1.).abs() > 0.01 {
                format!(" · zoom {:.0}%", self.zoom * 100.)
            } else {
                String::new()
            }
        );

        let picked = self.selected_nodes();
        let is_node = picked.len() == 1;
        let several = picked.len() >= 2;
        let detail = (self.selected.is_some() && !self.detail.is_empty()).then(|| {
            // Docked at the left, beside the graph rather than over it: what is selected, in
            // one structure whatever it is (`inspector_ui`), and what can be done with it.
            let (head, body) = match &self.inspected {
                Some(i) => (
                    self.inspector_head(i, &theme, cx),
                    self.inspector_body(i, &theme, cx),
                ),
                None => (
                    h_flex()
                        .p_3()
                        .justify_between()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(format!("{} selected", self.selected_ids().len()))
                        .child(self.status_menu(&self.common_status(), &theme))
                        .child(
                            Button::new("graph-deselect")
                                .small()
                                .ghost()
                                .icon(IconName::X)
                                .tooltip("Close")
                                .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
                        )
                        .into_any_element(),
                    self.multi_body(&theme),
                ),
            };
            let footer = v_flex()
                .flex_none()
                .p_3()
                .gap_2()
                .border_t_1()
                .border_color(theme.border)
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
                                .w(px(inspector_ui::DOCK_WIDTH - 60.))
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
                        .child(
                            Button::new("graph-note-toggle")
                                .small()
                                .label("Add note")
                                .tooltip("Attach a note about everything selected — recorded as yours")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.note_open = !this.note_open;
                                    if this.note_open {
                                        this.note.update(cx, |s, cx| s.focus(window, cx));
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .when(self.note_open, |c| {
                    c.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().child(Input::new(&self.note).small()))
                            .child(
                                Button::new("graph-note")
                                    .small()
                                    .primary()
                                    .label("Add")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_note(window, cx)
                                    })),
                            ),
                    )
                });
            let tags = self.tags_section(&theme, cx);
            let measured = self.dock_width.clone();
            v_flex()
                .id("graph-detail")
                .size_full()
                .relative()
                .bg(theme.popover)
                // Its width as drawn, for what has to wrap in a width of pixels.
                .child(
                    canvas(move |bounds, _, _| measured.set(f32::from(bounds.size.width)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(head)
                .child(tags)
                .child(body)
                .child(footer)
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

        // What sits over the plane: the toolbar, and what is open of the legend, the filter
        // layers, find, and the card a type's icon is chosen on.
        let toolbar = self.toolbar(counts, level, cx);
        let legend = self.legend_open.then(|| self.legend(&shown, plain, cx));
        let layers_panel = self
            .layers_open
            .then(|| self.layers_panel(&sifted, &shown, cx));
        // Find sits at the top right, clear of the layers at the bottom right.
        let find_right = 12.;
        // Under the toolbar where there is no room beside it.
        let find_top = if area_w - find_right - 452. < 860. {
            62.
        } else {
            12.
        };
        let find_bar = self
            .find_open
            .then(|| self.find_bar(find_right, find_top, cx));
        let picker = self.picker_card(cx);

        let graph = div()
            .id("graph-area")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(theme.background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    let at = this.local(event.position);
                    // The plane has the keys from here on, and what was open over it for a
                    // moment is done with.
                    window.focus(&this.focus, cx);
                    this.picker = None;
                    // Shift sweeps out an area to select; otherwise the plane is dragged.
                    this.drag = Some(if event.modifiers.shift {
                        Drag::Marquee { from: at, to: at }
                    } else {
                        Drag::View {
                            from: at,
                            pan: this.pan,
                        }
                    });
                    // The selection stays: the inspector is closed with its X, not by a click
                    // that was meant to pan the plane or to start a sweep.
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    // Add a node exactly where the pointer is.
                    let at = this.local(event.position);
                    let spot = Spot::Exact(this.to_world(at));
                    this.open_add_form(spot, Some(at), true, window, cx);
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
                    Some(Drag::Width {
                        id,
                        from,
                        start,
                        grows,
                    }) => {
                        let (id, from, start, grows) = (id.clone(), *from, *start, *grows);
                        let x = this.to_world(at).0;
                        this.drag_width(&id, from, start, grows, x);
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
            .on_action(cx.listener(|this, _: &GraphFind, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &GraphSmaller, _, cx| this.step_size(false, cx)))
            .on_action(cx.listener(|this, _: &GraphLarger, _, cx| this.step_size(true, cx)))
            .on_action(cx.listener(|this, _: &GraphAutoSize, _, cx| this.auto_size(cx)))
            .on_action(cx.listener(|this, _: &GraphDismiss, window, cx| this.dismiss(window, cx)))
            .on_action(cx.listener(|this, _: &GraphNewNode, window, cx| {
                let spot = Spot::Near(this.to_world(this.centre()));
                this.open_add_form(spot, None, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GraphAddUp, _, cx| {
                if matches!(this.form, Some(Form::Add { .. })) {
                    this.add_move(false, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &GraphAddDown, _, cx| {
                if matches!(this.form, Some(Form::Add { .. })) {
                    this.add_move(true, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, a: &GraphSetStatus, _, cx| {
                if a.status == "soft-deleted" {
                    this.soft_delete(cx);
                } else {
                    this.set_status(&a.status, cx);
                }
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
            .child(toolbar)
            .children(legend)
            .children(layers_panel)
            .children(find_bar)
            .children(picker)
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
                        .bottom_2()
                        .left_2()
                        .max_w(px(520.))
                        .p_2()
                        .rounded_md()
                        .bg(theme.popover)
                        .text_size(small)
                        .text_color(theme.danger)
                        .child(error),
                )
            });

        // The inspector docked at the left, and the graph in the rest; the divider between
        // them is dragged, and the width is kept while the window is open.
        match detail {
            Some(inspector) => div()
                .size_full()
                .child(
                    h_resizable("graph-split")
                        .child(
                            resizable_panel()
                                .size(px(inspector_ui::DOCK_WIDTH))
                                .size_range(px(300.)..px(900.))
                                .child(inspector),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(240.)..px(4000.))
                                .child(graph),
                        ),
                )
                .into_any_element(),
            None => graph.into_any_element(),
        }
    }
}
