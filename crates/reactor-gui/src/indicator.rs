//! Indicators of a state — "the agent is working", "this session is working" — drawn from the
//! wall clock rather than from gpui's animation element.
//!
//! The animation element is for *transitions*, and `App::reduce_motion` (the "Interface
//! animations" setting) turns every one of them into its end state so nothing waits to settle.
//! A shimmer is not a transition: it says something is going on, and it should keep saying it.
//! So it is computed here from the time. A shimmer asks for its own frames where drawing is
//! cheap, so it runs at the display's rate like the library's; a pulse, and every indicator in
//! the light rendering mode, is redrawn by the app's indicator clock
//! (`ReactorApp::start_notice_clock`) — about twelve a second there, which on a software
//! rasteriser is the difference between a live window and a busy one.
//!
//! What made the earlier version clunky was not the rate but the band stepping from letter to
//! letter. It is now placed in pixels and built of nested masks, as the library's is.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{
    App, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, Point, SharedString, StyledText, Window, point, px, size,
};

/// How far along a repeating cycle of `period` the clock is, in `0.0..1.0`. Every indicator on
/// screen reads the same clock, so they sweep together.
pub fn phase(period: Duration) -> f32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let period = period.as_millis().max(1);
    (now.as_millis() % period) as f32 / period as f32
}

/// How long one sweep of a shimmer takes.
const SWEEP: Duration = Duration::from_secs(2);

/// Half the width of the lit band, as a fraction of the text.
const SPREAD: f32 = 0.3;

/// How many nested masks make up the band: each adds a little light, so the band is brightest
/// in the middle and has no edge. (The same construction as the library's shimmer.)
const LAYERS: usize = 12;

/// The light at the middle of the band: what all the layers add up to.
const PEAK: f32 = 0.7;

/// A sweep of `highlight` over `text` once per two seconds, over whatever text colour the
/// parent sets.
///
/// The band is placed in pixels, not in characters, so it glides rather than stepping from
/// letter to letter. Its position is read from the clock: where drawing is cheap the element asks
/// for a frame after each one it draws, like the library's animation element; in the light
/// rendering mode it waits for the app's indicator clock instead.
pub fn shimmer(text: impl Into<SharedString>, highlight: Hsla) -> Shimmer {
    Shimmer {
        text: StyledText::new(text.into()),
        highlight,
    }
}

/// The element [`shimmer`] makes.
pub struct Shimmer {
    text: StyledText,
    highlight: Hsla,
}

impl IntoElement for Shimmer {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The horizontal extent, in `0.0..=width`, of the `layer`th mask of the band at `phase`.
/// Outer layers are the widest.
fn band(width: f32, phase: f32, layer: usize) -> Option<(f32, f32)> {
    if width <= 0.0 || layer >= LAYERS {
        return None;
    }
    let half = width * SPREAD;
    // The band enters from beyond the left edge and leaves beyond the right.
    let padding = half / width + 0.05;
    let centre = phase.mul_add(1.0 + padding * 2.0, -padding) * width;
    let radius = half * (1.0 - layer as f32 / LAYERS as f32);
    let left = (centre - radius).max(0.0);
    let right = (centre + radius).min(width);
    (right > left).then_some((left, right))
}

impl Element for Shimmer {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text
            .request_layout(global_id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text
            .prepaint(global_id, inspector_id, bounds, layout, window, cx);
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text.paint(
            global_id,
            inspector_id,
            bounds,
            layout,
            prepaint,
            window,
            cx,
        );
        // Where drawing is cheap, keep the frames coming; otherwise the app's clock redraws.
        if !crate::settings::LightRendering::on(cx) {
            window.request_animation_frame();
        }

        let width = bounds.size.width.as_f32();
        let phase = phase(SWEEP);
        let masks: Vec<Option<ContentMask<Pixels>>> = (0..LAYERS)
            .map(|layer| {
                band(width, phase, layer).map(|(left, right)| ContentMask {
                    bounds: Bounds::new(
                        point(bounds.origin.x + px(left), bounds.origin.y),
                        size(px(right - left), bounds.size.height),
                    ),
                })
            })
            .collect();
        if masks.iter().all(Option::is_none) {
            return;
        }
        let layer_opacity = 1.0 - (1.0 - PEAK).powf(1.0 / LAYERS as f32);
        let color = self.highlight.opacity(layer_opacity);

        let layout = self.text.layout();
        let line_height = layout.line_height();
        let mut line_origin = bounds.origin;
        window.paint_layer(bounds, |window| {
            for wrapped in layout.line_layouts() {
                let line = &wrapped.unwrapped_layout;
                let baseline = point(
                    px(0.),
                    (line_height - line.ascent - line.descent) / 2. + line.ascent,
                );
                let mut wraps = wrapped.wrap_boundaries.iter().peekable();
                let mut glyph_origin = point(line_origin.x, line_origin.y);
                let mut previous = Point::default();
                for (run_index, run) in line.runs.iter().enumerate() {
                    let glyph_size = cx
                        .text_system()
                        .bounding_box(run.font_id, line.font_size)
                        .size;
                    for (glyph_index, glyph) in run.glyphs.iter().enumerate() {
                        glyph_origin.x += glyph.position.x - previous.x;
                        if wraps
                            .peek()
                            .is_some_and(|w| w.run_ix == run_index && w.glyph_ix == glyph_index)
                        {
                            wraps.next();
                            glyph_origin.x = line_origin.x;
                            glyph_origin.y += line_height;
                        }
                        previous = glyph.position;
                        if glyph.is_emoji {
                            continue;
                        }
                        let glyph_bounds = Bounds::new(glyph_origin, glyph_size);
                        let at = glyph_origin + baseline + point(px(0.), glyph.position.y);
                        for mask in masks.iter().flatten() {
                            if !glyph_bounds.intersects(&mask.bounds) {
                                continue;
                            }
                            window.with_content_mask(Some(*mask), |window| {
                                let _ = window.paint_glyph(
                                    at,
                                    run.font_id,
                                    glyph.id,
                                    line.font_size,
                                    color,
                                );
                            });
                        }
                    }
                }
                line_origin.y += wrapped.size(line_height).height;
            }
        });
    }
}

/// The swell of a "this row is working" wash, `0.0..1.0`, once per 1.6 s.
pub fn pulse() -> f32 {
    0.5 - 0.5 * (phase(Duration::from_millis(1600)) * std::f32::consts::TAU).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_travels_from_the_start_to_the_end() {
        let centre = |phase: f32| band(200.0, phase, LAYERS - 1).map(|(l, r)| (l + r) / 2.0);
        let early = centre(0.3).unwrap();
        let late = centre(0.7).unwrap();
        assert!(early < late);
    }

    #[test]
    fn between_sweeps_nothing_is_lit() {
        assert!(band(200.0, 0.0, 0).is_none());
        assert!(band(200.0, 1.0, 0).is_none());
    }

    #[test]
    fn the_band_moves_by_pixels_not_by_letters() {
        // A tenth of a second of the sweep moves it by a fraction of a character.
        let step = 0.05 / 2.0;
        let (a, _) = band(100.0, 0.5, LAYERS - 1).unwrap();
        let (b, _) = band(100.0, 0.5 + step, LAYERS - 1).unwrap();
        assert!(b > a && (b - a) < 10.0);
    }

    #[test]
    fn inner_layers_are_narrower() {
        let outer = band(200.0, 0.5, 0).unwrap();
        let inner = band(200.0, 0.5, LAYERS - 1).unwrap();
        assert!(outer.0 < inner.0 && inner.1 < outer.1);
    }

    #[test]
    fn the_phase_is_a_fraction() {
        let p = phase(Duration::from_secs(2));
        assert!((0.0..1.0).contains(&p));
    }
}
