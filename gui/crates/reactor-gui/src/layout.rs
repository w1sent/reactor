//! Where the panels sit — and the named arrangements the user can switch
//! between.
//!
//! Nothing here is fixed furniture. Every panel is draggable between docks
//! and droppable onto another's tab bar (gpui-kit's dock does that on its
//! own once an area has more than one *group*) — except that gpui-base
//! deliberately refuses to drag a group's last panel out when that group has
//! no sibling: `TabGroupContext::draggable` is `!is_locked() && !is_alone()`,
//! and a dock holding exactly one bare tab group has nothing beside it to be
//! not-alone with. A single-panel dock or centre is therefore stuck by
//! construction, no matter what the panel itself allows — this is why every
//! preset below pairs each area with a sibling rather than leaving any of
//! them as one lone group: it is the only way every panel ends up
//! rearrangeable, not a stylistic choice.
//!
//! `Focus` is the deliberate exception: showing the transcript alone with
//! nothing else on screen is the point of it, so nothing there needs
//! anywhere to go.
//!
//! What this module adds on top of that freedom is the way back: named
//! arrangements, each putting a *different* panel in the centre — the
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
use gpui_kit::{App, Entity, Pixels, Window, px};
use serde::Deserialize;

/// Handles to every dockable panel, so any arrangement can be rebuilt from
/// the panels that already exist.
#[derive(Clone)]
pub struct Panels {
    pub transcript: Arc<dyn PanelView>,
    pub tree: Arc<dyn PanelView>,
    pub tools: Arc<dyn PanelView>,
    pub toolsets: Arc<dyn PanelView>,
    pub views: Arc<dyn PanelView>,
    pub services: Arc<dyn PanelView>,
    pub console: Arc<dyn PanelView>,
}

impl Panels {
    /// A tab group holding these panels, in order.
    fn tabs(&self, panels: &[&Arc<dyn PanelView>], cx: &App) -> DockLayout {
        let mut layout = DockLayout::tabs();
        for panel in panels {
            layout = layout.panel_view(Arc::clone(panel), cx);
        }
        layout
    }

    /// Two groups side by side, each other's sibling — see the module doc:
    /// this is what makes both halves draggable.
    fn split(
        &self,
        a: &[&Arc<dyn PanelView>],
        b: &[&Arc<dyn PanelView>],
        second: Pixels,
        cx: &App,
    ) -> DockLayout {
        DockLayout::v_split()
            .child(self.tabs(a, cx), None)
            .child(self.tabs(b, cx), Some(second))
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
/// Each one puts a different panel in the centre, which is the point: the
/// centre belongs to whatever the work is, not permanently to the
/// transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum LayoutPreset {
    /// Conversation and session tree share the centre; the catalogue and the
    /// console pair off with their own supporting panel on either side.
    Default,
    /// The transcript alone, every dock gone — for reading a long answer or
    /// writing a careful prompt. The one preset that is not rearrangeable,
    /// deliberately: there is nowhere for anything to go.
    Focus,
    /// Driving tools: the console and the conversation share the centre,
    /// catalogue and session context on the left, live status on the right.
    Analysis,
    /// Curating the toolbox: the catalogue takes the centre, big enough to
    /// read descriptions rather than guess from ids.
    Catalogue,
}

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
        let center = self.center(panels, cx);
        let docks = self.docks(panels, cx);
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

    fn center(self, panels: &Panels, cx: &App) -> DockLayout {
        match self {
            LayoutPreset::Default => {
                panels.split(&[&panels.transcript], &[&panels.tree], px(320.), cx)
            }
            LayoutPreset::Focus => panels.tabs(&[&panels.transcript], cx),
            LayoutPreset::Analysis => {
                panels.split(&[&panels.console], &[&panels.transcript], px(480.), cx)
            }
            LayoutPreset::Catalogue => panels.split(
                &[&panels.tools, &panels.toolsets, &panels.views],
                &[&panels.tree],
                px(280.),
                cx,
            ),
        }
    }

    /// One slot per [`DockSide::ALL`], in order — `None` clears that side.
    fn docks(self, panels: &Panels, cx: &App) -> [Option<(DockLayout, Pixels)>; 3] {
        match self {
            LayoutPreset::Default => [
                None,
                Some((
                    panels.split(
                        &[&panels.tools, &panels.toolsets],
                        &[&panels.views],
                        px(240.),
                        cx,
                    ),
                    px(360.),
                )),
                Some((
                    panels.split(&[&panels.console], &[&panels.services], px(200.), cx),
                    px(260.),
                )),
            ],
            LayoutPreset::Focus => [None, None, None],
            LayoutPreset::Analysis => [
                Some((
                    panels.split(
                        &[&panels.tools, &panels.toolsets],
                        &[&panels.views],
                        px(240.),
                        cx,
                    ),
                    px(320.),
                )),
                Some((
                    panels.split(&[&panels.tree], &[&panels.services], px(220.), cx),
                    px(280.),
                )),
                None,
            ],
            LayoutPreset::Catalogue => [
                None,
                Some((
                    panels.split(&[&panels.console], &[&panels.services], px(200.), cx),
                    px(420.),
                )),
                None,
            ],
        }
    }
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
    fn every_dock_side_is_named_and_maps_to_a_placement() {
        for side in DockSide::ALL {
            assert!(!side.label().is_empty());
        }
        assert_eq!(DockSide::Left.placement(), DockPlacement::Left);
        assert_eq!(DockSide::Right.placement(), DockPlacement::Right);
        assert_eq!(DockSide::Bottom.placement(), DockPlacement::Bottom);
    }
}
