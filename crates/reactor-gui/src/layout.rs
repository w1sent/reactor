//! Where the panels sit — and the named arrangements the user can switch
//! between.
//!
//! Nothing here is fixed furniture: every panel can be closed and opened again —
//! from the Layout menu's Panels submenu, the command palette, or the button at the right
//! of its own header — and a closed panel returns to where the active preset keeps it.
//!
//! Each area of a preset is one tab group. That costs something gpui-base takes away from
//! a lone group: `TabGroupContext::draggable` is `!is_locked() && !is_alone()`, and
//! `closable` needs `draggable`, so a dock holding exactly one group can neither have its
//! panels dragged out nor closed through the tab bar's own menu. Closing therefore does not
//! go through the tab group: it removes the panel from the `DockArea` directly (the
//! `remove` hook of [`Panels`]), which has no such rule.
//!
//! What this module adds on top of that freedom is the way back: named
//! arrangements, one of which (Catalogue) puts a different panel in the centre — the
//! clearest statement that the centre is not owned by the transcript.
//!
//! Presets rebuild the layout from [`Panels`] — handles held since startup —
//! rather than constructing panels afresh. That is what keeps switching
//! cheap and, more importantly, lossless: the console's scrollback, the
//! transcript's expanded thinking blocks and every scroll position are state
//! inside those entities, and rebuilding them would quietly throw it away.

use std::sync::Arc;

use gpui_kit::base::dock::PanelView;
use gpui_kit::component::dock::{DockArea, DockLayout, DockPlacement};
use gpui_kit::{App, Context, Entity, Window, px};
use serde::Deserialize;

/// Every dockable panel, by name — what the Panels menu, the palette and each panel's close
/// button refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum PanelKind {
    Transcript,
    Tree,
    Tools,
    Toolsets,
    Context,
    Services,
    Console,
}

impl PanelKind {
    pub const ALL: [PanelKind; 7] = [
        PanelKind::Transcript,
        PanelKind::Tree,
        PanelKind::Tools,
        PanelKind::Toolsets,
        PanelKind::Context,
        PanelKind::Services,
        PanelKind::Console,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PanelKind::Transcript => "Transcript",
            PanelKind::Tree => "Session Tree",
            PanelKind::Tools => "Tools",
            PanelKind::Toolsets => "Toolsets",
            PanelKind::Context => "Context",
            PanelKind::Services => "Services",
            PanelKind::Console => "Console",
        }
    }

    /// What `/panel` takes.
    pub fn slug(self) -> &'static str {
        match self {
            PanelKind::Transcript => "transcript",
            PanelKind::Tree => "tree",
            PanelKind::Tools => "tools",
            PanelKind::Toolsets => "toolsets",
            PanelKind::Context => "context",
            PanelKind::Services => "services",
            PanelKind::Console => "console",
        }
    }

    pub fn from_slug(slug: &str) -> Option<PanelKind> {
        PanelKind::ALL
            .into_iter()
            .find(|kind| kind.slug().eq_ignore_ascii_case(slug))
    }

    /// Where it reopens when the active preset does not place it anywhere.
    fn fallback_home(self) -> DockPlacement {
        match self {
            PanelKind::Transcript => DockPlacement::Center,
            PanelKind::Console => DockPlacement::Bottom,
            _ => DockPlacement::Right,
        }
    }
}

type Remover = dyn Fn(PanelKind, &mut DockArea, &mut Window, &mut Context<DockArea>) + Send + Sync;

/// Handles to every dockable panel, so any arrangement can be rebuilt from
/// the panels that already exist.
#[derive(Clone)]
pub struct Panels {
    pub transcript: Arc<dyn PanelView>,
    pub tree: Arc<dyn PanelView>,
    pub tools: Arc<dyn PanelView>,
    pub toolsets: Arc<dyn PanelView>,
    pub context: Arc<dyn PanelView>,
    pub services: Arc<dyn PanelView>,
    pub console: Arc<dyn PanelView>,
    /// Takes a panel out of the dock. `DockArea` removes by typed entity, and the handles
    /// above have forgotten their type, so whoever built the panels supplies this.
    pub remove: Arc<Remover>,
}

impl Panels {
    pub fn view(&self, kind: PanelKind) -> &Arc<dyn PanelView> {
        match kind {
            PanelKind::Transcript => &self.transcript,
            PanelKind::Tree => &self.tree,
            PanelKind::Tools => &self.tools,
            PanelKind::Toolsets => &self.toolsets,
            PanelKind::Context => &self.context,
            PanelKind::Services => &self.services,
            PanelKind::Console => &self.console,
        }
    }

    /// Whether the panel is in the dock at all (a collapsed dock still holds its panels).
    pub fn is_open(&self, kind: PanelKind, area: &DockArea, cx: &App) -> bool {
        area.panel(self.view(kind).panel_id(cx)).is_some()
    }

    /// A tab group holding these panels, in order.
    fn tabs(&self, kinds: &[PanelKind], cx: &App) -> DockLayout {
        let mut layout = DockLayout::tabs();
        for kind in kinds {
            layout = layout.panel_view(Arc::clone(self.view(*kind)), cx);
        }
        layout
    }
}

/// Which dock a [`ToggleDockAction`] acts on.
///
/// Its own enum rather than `DockPlacement` because an action has to
/// deserialize, and because only these three are ever toggled — the centre
/// is not a dock and cannot be hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

impl DockSide {
    pub const ALL: [DockSide; 3] = [DockSide::Left, DockSide::Right, DockSide::Bottom];

    /// Generic on purpose: what lives on a side changes from preset to
    /// preset (§ above), so "Show Session Tree" would lie the moment the
    /// active preset put something else there.
    pub fn label(self) -> &'static str {
        match self {
            DockSide::Left => "Left Dock",
            DockSide::Right => "Right Dock",
            DockSide::Bottom => "Bottom Dock",
        }
    }

    fn placement(self) -> DockPlacement {
        match self {
            DockSide::Left => DockPlacement::Left,
            DockSide::Right => DockPlacement::Right,
            DockSide::Bottom => DockPlacement::Bottom,
        }
    }
}

/// A named arrangement of the window.
///
/// The centre belongs to whatever the work is, not permanently to the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum LayoutPreset {
    /// The conversation in the centre, every supporting panel tabbed on the right, the console
    /// below.
    Default,
    /// The conversation with only the console below — for reading a long answer or writing a
    /// careful prompt.
    Focus,
    /// The conversation, with the session tree on the left and the catalogue panels on the
    /// right.
    Analysis,
    /// Curating the toolbox: the tools take the centre, big enough to read descriptions rather
    /// than guess from ids; services on the left, toolsets on the right.
    Catalogue,
}

/// What a preset puts on one side: the panels (one tab group, in order) and the dock's size.
type Side = (&'static [PanelKind], f32);

impl LayoutPreset {
    pub const ALL: [LayoutPreset; 4] = [
        LayoutPreset::Default,
        LayoutPreset::Focus,
        LayoutPreset::Analysis,
        LayoutPreset::Catalogue,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayoutPreset::Default => "Default",
            LayoutPreset::Focus => "Focus",
            LayoutPreset::Analysis => "Analysis",
            LayoutPreset::Catalogue => "Catalogue",
        }
    }

    /// The centre's panels, then one slot per [`DockSide::ALL`] — an empty side is cleared.
    /// The whole arrangement is this table; a panel it does not list starts closed.
    fn plan(self) -> (&'static [PanelKind], [Side; 3]) {
        use PanelKind::*;
        const CONSOLE: Side = (&[Console], 260.);
        match self {
            LayoutPreset::Default => (
                &[Transcript],
                [
                    (&[], 0.),
                    (&[Tools, Toolsets, Context, Services, Tree], 360.),
                    CONSOLE,
                ],
            ),
            LayoutPreset::Focus => (&[Transcript], [(&[], 0.), (&[], 0.), CONSOLE]),
            LayoutPreset::Analysis => (
                &[Transcript],
                [
                    (&[Tree], 280.),
                    (&[Tools, Toolsets, Context, Services], 360.),
                    CONSOLE,
                ],
            ),
            LayoutPreset::Catalogue => (
                &[Tools],
                [(&[Services], 280.), (&[Toolsets], 360.), CONSOLE],
            ),
        }
    }

    /// Where this preset keeps the panel — and so where closing it and opening it again
    /// brings it back to.
    fn home(self, kind: PanelKind) -> (DockPlacement, f32) {
        let (center, sides) = self.plan();
        if center.contains(&kind) {
            return (DockPlacement::Center, 0.);
        }
        for (side, (kinds, size)) in DockSide::ALL.into_iter().zip(sides) {
            if kinds.contains(&kind) {
                return (side.placement(), size);
            }
        }
        match kind.fallback_home() {
            DockPlacement::Bottom => (DockPlacement::Bottom, 260.),
            placement => (placement, 360.),
        }
    }

    /// Rebuild the dock area into this arrangement.
    ///
    /// Every side is set *or* explicitly cleared — never left alone — so a
    /// switch is total: nothing a previous preset put somewhere survives a
    /// side this one does not use.
    pub fn apply(
        self,
        panels: &Panels,
        dock: &Entity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (center, sides) = self.plan();
        let center = panels.tabs(center, cx);
        let docks = sides
            .map(|(kinds, size)| (!kinds.is_empty()).then(|| (panels.tabs(kinds, cx), px(size))));
        dock.update(cx, |area, cx| {
            area.set_center(center, window, cx);
            for (side, slot) in DockSide::ALL.into_iter().zip(docks) {
                match slot {
                    Some((layout, size)) => {
                        area.set_dock(side.placement(), layout, window, cx);
                        area.set_dock_size(side.placement(), size, window, cx);
                    }
                    None => area.remove_dock(side.placement(), window, cx),
                }
            }
        });
    }
}

/// Close the panel if it is open, otherwise open it where the preset keeps it.
pub fn toggle_panel(
    kind: PanelKind,
    preset: LayoutPreset,
    panels: &Panels,
    dock: &Entity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let panels = panels.clone();
    dock.update(cx, |area, cx| {
        if panels.is_open(kind, area, cx) {
            (panels.remove)(kind, area, window, cx);
            return;
        }
        let (placement, size) = preset.home(kind);
        area.add_panel_view(
            Arc::clone(panels.view(kind)),
            placement,
            Some(px(size)),
            window,
            cx,
        );
        // A dock the user collapsed would swallow the panel they just asked for.
        if placement != DockPlacement::Center
            && area.has_dock(placement)
            && !area.is_dock_open(placement)
        {
            area.toggle_dock(placement, window, cx);
        }
    });
}

/// Show or hide one dock, leaving the rest of the arrangement alone.
pub fn toggle_dock(side: DockSide, dock: &Entity<DockArea>, window: &mut Window, cx: &mut App) {
    dock.update(cx, |area, cx| {
        area.toggle_dock(side.placement(), window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset is reachable from the menu, and none shares a label
    /// with another — a duplicate would be indistinguishable once it is a
    /// menu item.
    #[test]
    fn presets_are_distinct_and_named() {
        let mut labels: Vec<&str> = LayoutPreset::ALL.iter().map(|p| p.label()).collect();
        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count, "two presets share a label");
        assert!(labels.iter().all(|label| !label.is_empty()));
    }

    #[test]
    fn every_panel_is_named_and_addressable() {
        for kind in PanelKind::ALL {
            assert!(!kind.label().is_empty());
            assert_eq!(PanelKind::from_slug(kind.slug()), Some(kind));
        }
    }

    /// A panel sits in one place per preset, and each preset has a centre.
    #[test]
    fn presets_place_each_panel_at_most_once() {
        for preset in LayoutPreset::ALL {
            let (center, sides) = preset.plan();
            assert!(!center.is_empty(), "{preset:?} has an empty centre");
            let mut all: Vec<PanelKind> = center.to_vec();
            all.extend(sides.iter().flat_map(|(kinds, _)| kinds.iter().copied()));
            let count = all.len();
            all.dedup();
            all.sort_by_key(|k| k.slug());
            all.dedup();
            assert_eq!(all.len(), count, "{preset:?} places a panel twice");
        }
    }

    #[test]
    fn every_dock_side_is_named_and_maps_to_a_placement() {
        for side in DockSide::ALL {
            assert!(!side.label().is_empty());
        }
        assert_eq!(DockSide::Left.placement(), DockPlacement::Left);
        assert_eq!(DockSide::Right.placement(), DockPlacement::Right);
        assert_eq!(DockSide::Bottom.placement(), DockPlacement::Bottom);
    }
}
