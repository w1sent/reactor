//! Explanations that stay out of the way.
//!
//! The window carries data and controls, not prose. Anything that explains — what a bar
//! means, how a setting is changed, where something is saved — is a **tooltip** on the element
//! it is about, or, where there is no element to hover, an **(i) button** that opens the
//! explanation when clicked. (A standing design rule: no descriptive labels.)

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, StatefulInteractiveElement, div, px};

/// Show `text` when the pointer rests on `element`. The element needs an id.
pub fn tip<E: StatefulInteractiveElement>(element: E, text: impl Into<SharedString>) -> E {
    let text: SharedString = text.into();
    element.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
}

/// A small (i) button that opens `text` in a popover.
pub fn info_button(id: impl Into<SharedString>, text: impl Into<SharedString>) -> impl IntoElement {
    let id: SharedString = id.into();
    let text: SharedString = text.into();
    Popover::new(SharedString::from(format!("{id}-popover")))
        .trigger(Button::new(id).icon(IconName::Info).ghost().small())
        .content(move |_state, _window, cx| {
            div()
                .max_w(px(340.))
                .text_size(cx.theme().font_size * 0.9)
                .text_color(cx.theme().foreground)
                .child(text.clone())
        })
}
