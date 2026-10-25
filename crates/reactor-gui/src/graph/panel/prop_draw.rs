//! The two parts of a property's value that are drawn the same in the inspector's *Details* and
//! on a card at its largest size (ADR-0067): a `verbatim` value as a code block, and a `category`
//! as a chip. Sizes come from the font size they are given, so a card at any zoom and the
//! inspector draw alike.

use gpui_kit::base::h_flex;
use gpui_kit::component::Theme;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Hsla, Pixels, SharedString, div};

use crate::graph::look;
use crate::graph::props::lines;
use reactor_db::graph::vocab::Vocabulary;

/// A command, source or record as a code block: the muted fill markdown code blocks have, the
/// text as it was written, and a gutter of line numbers. A long line wraps and its number stays
/// at its first row.
pub(super) fn verbatim_block(text: &str, small: Pixels, pad: Pixels, theme: &Theme) -> AnyElement {
    let lines = lines(text);
    let digits = lines.len().to_string().len() as f32;
    let gutter = small * (0.62 * digits + 0.55);
    div()
        .w_full()
        .rounded_md()
        .bg(theme.muted)
        .border_1()
        .border_color(theme.border)
        .py(pad)
        .font_family(theme.mono_font_family.clone())
        .text_size(small)
        .children(lines.into_iter().enumerate().map(|(n, line)| {
            h_flex()
                .items_start()
                .gap(small * 0.5)
                .child(
                    div()
                        .flex_none()
                        .w(gutter)
                        .pl(small * 0.4)
                        .flex()
                        .justify_end()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from((n + 1).to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pr(small * 0.5)
                        // An empty line keeps its height.
                        .child(SharedString::from(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line
                        })),
                )
        }))
        .into_any_element()
}

/// The colour a category chip is drawn in: the one the case gave the same text as a tag, and
/// neutral where it gave none. A category clusters nothing, so it has no outline on the plane.
pub(super) fn category_colour(vocabulary: &Vocabulary, text: &str, theme: &Theme) -> Hsla {
    if vocabulary.tag_colour(text).is_some() {
        look::tag_colour(vocabulary, text, theme.is_dark())
    } else {
        theme.muted_foreground
    }
}

/// A category as a chip.
pub(super) fn chip(text: &str, colour: Hsla, small: Pixels, height: Pixels) -> AnyElement {
    div()
        .px(small * 0.7)
        .h(height)
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(colour.opacity(0.7))
        .bg(colour.opacity(0.14))
        .text_size(small)
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

/// The shade of every second row of properties, so the eye can follow a row across the label
/// and the value: the theme's secondary fill, half as strong. The first row has none.
pub(super) fn stripe(theme: &Theme, row: usize) -> Option<Hsla> {
    (row % 2 == 1).then(|| theme.secondary.opacity(0.5))
}
