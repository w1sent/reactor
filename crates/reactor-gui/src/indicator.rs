//! Indicators of a state — "the agent is working", "this session is working" — drawn from the
//! wall clock rather than from gpui's animation element.
//!
//! The animation element is for *transitions*, and `App::reduce_motion` (the "Interface
//! animations" setting) turns every one of them into its end state so nothing waits to settle.
//! A shimmer is not a transition: it says something is going on, and it should keep saying it.
//! So it is computed here from the time, and redrawn by the app's indicator clock
//! (`ReactorApp::start_notice_clock`) at a few frames a second instead of once per vsync —
//! which on a software rasteriser is the difference between a live window and a busy one.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{HighlightStyle, Hsla, SharedString, StyledText};

/// How far along a repeating cycle of `period` the clock is, in `0.0..1.0`. Every indicator on
/// screen reads the same clock, so they sweep together.
pub fn phase(period: Duration) -> f32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let period = period.as_millis().max(1);
    (now.as_millis() % period) as f32 / period as f32
}

/// A sweep of `highlight` over `text` once per two seconds, over whatever text colour the
/// parent sets.
pub fn shimmer(text: impl Into<SharedString>, highlight: Hsla) -> StyledText {
    shimmer_at(text, highlight, phase(Duration::from_secs(2)))
}

/// [`shimmer`] at a given point of its sweep (`0.0..1.0`).
pub fn shimmer_at(text: impl Into<SharedString>, highlight: Hsla, phase: f32) -> StyledText {
    let text: SharedString = text.into();
    let runs = shimmer_runs(&text, highlight, phase);
    StyledText::new(text).with_highlights(runs)
}

/// Half the width of the lit band, as a fraction of the text.
const SPREAD: f32 = 0.3;

/// The highlighted ranges: each character lit by how near the band's centre it is. Characters
/// the band does not reach have no run, so they take the parent's colour.
fn shimmer_runs(
    text: &str,
    highlight: Hsla,
    phase: f32,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let count = text.chars().count().max(1);
    // The band enters from beyond the left edge and leaves beyond the right, so the text is
    // dark for a moment between sweeps.
    let centre = phase * (1.0 + 2.0 * SPREAD) - SPREAD;
    text.char_indices()
        .enumerate()
        .filter_map(|(i, (start, ch))| {
            let x = if count > 1 {
                i as f32 / (count - 1) as f32
            } else {
                0.5
            };
            let distance = ((x - centre).abs() / SPREAD).min(1.0);
            // Smooth shoulder, so the band has no hard edge.
            let lit = 1.0 - distance * distance * (3.0 - 2.0 * distance);
            (lit > 0.02).then(|| {
                (
                    start..start + ch.len_utf8(),
                    HighlightStyle {
                        color: Some(highlight.opacity(lit)),
                        ..Default::default()
                    },
                )
            })
        })
        .collect()
}

/// The swell of a "this row is working" wash, `0.0..1.0`, once per 1.6 s.
pub fn pulse() -> f32 {
    0.5 - 0.5 * (phase(Duration::from_millis(1600)) * std::f32::consts::TAU).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::hsla;

    fn lit(text: &str, phase: f32) -> Vec<usize> {
        shimmer_runs(text, hsla(0.0, 0.0, 1.0, 1.0), phase)
            .into_iter()
            .map(|(range, _)| range.start)
            .collect()
    }

    #[test]
    fn the_band_travels_from_the_start_to_the_end() {
        let text = "thinking the problem over";
        let early = lit(text, 0.2);
        let late = lit(text, 0.8);
        assert!(!early.is_empty() && !late.is_empty());
        assert!(early.iter().max() < late.iter().max());
        assert!(early.iter().min() < late.iter().min());
    }

    #[test]
    fn between_sweeps_nothing_is_lit() {
        assert!(lit("working", 0.0).is_empty());
        assert!(lit("working", 1.0).is_empty());
    }

    #[test]
    fn runs_fall_on_character_boundaries() {
        let text = "tänk… klar";
        for step in 0..=10 {
            for (range, _) in shimmer_runs(text, hsla(0.0, 0.0, 1.0, 1.0), step as f32 / 10.0) {
                assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            }
        }
    }

    #[test]
    fn the_phase_is_a_fraction() {
        let p = phase(Duration::from_secs(2));
        assert!((0.0..1.0).contains(&p));
    }
}
