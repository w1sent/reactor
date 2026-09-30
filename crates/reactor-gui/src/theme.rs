//! The Ayu Dark theme (SPEC.md §7).
//!
//! One shipped theme in v0.1 — [Ayu Dark](https://github.com/ayu-theme/ayu-colors),
//! ported from the upstream `themes/dark.yaml` palette: a warm gold accent
//! (`#E6B450`) against a cool near-black, with success/warning/danger/info
//! each its own hue (green/orange/red/blue) rather than overloading the
//! accent — a redesign from the original single-hue "cyber dark" theme,
//! whose accent-green did double duty as success too. The token set rides
//! gpui-kit's semantic tokens, so a light theme later is data, not code.
//!
//! The JSON is a gpui-component `ThemeSet` — the official format, loaded
//! through `ThemeRegistry` and applied through `Theme::change`, so the
//! theme switcher and every component agree. The exact `ThemeColor` field
//! names were verified against the vendored gpui-component 0.6.6 source
//! (`src/theme/default-theme.json` ships the shape).

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};

/// The palette embedded at build time — no theme import.
pub const AYU_DARK: &str = include_str!("ayu-dark.json");

/// Load "Ayu Dark" into the registry, make it the dark default, and apply.
///
/// The registry's own observer resolves themes by name the same way; setting
/// `dark_theme` directly is the documented path for an application that
/// ships its own theme instead of asking the user to pick one at startup.
pub fn install_ayu_dark(cx: &mut gpui_kit::App) {
    ThemeRegistry::global_mut(cx)
        .load_themes_from_str(AYU_DARK)
        .expect("embedded Ayu Dark theme must parse");

    if let Some(ayu) = gpui_kit::component::ThemeRegistry::global(cx)
        .themes()
        .get("Ayu Dark")
        .cloned()
    {
        gpui_kit::component::Theme::global_mut(cx).dark_theme = ayu;
    }
    Theme::change(ThemeMode::Dark, None, cx);
}

/// A per-instance override for `TextView::markdown(..).style(..)`, toning
/// down inline code's fill.
///
/// gpui-component's `TextView` (the component-level wrapper this crate uses,
/// not `gpui_base`'s own) always re-derives its rich-text style fresh from
/// `Theme::global` on every render and *folds* this override on top of it —
/// it never consults `gpui_base::TextViewDefaults`, so installing a global
/// default there (an earlier, dead-end attempt at this fix) has no effect on
/// it at all. The themed default paints inline code (single backtick spans)
/// with `background_color: theme.accent` — reasonable for a button, way too
/// loud for a file path or identifier sitting in the middle of a sentence,
/// and on Ayu Dark's gold accent the body-text foreground on top of it reads
/// as barely-there (the "highlighted text is not readable, over the top"
/// bug). Every other field here stays at its default, which per
/// `TextViewStyle`'s own contract keeps the themed value rather than
/// overriding it with a neutral one — only `inline_code` changes, to the
/// same muted fill code *blocks* already use: "a grayish background, just
/// slightly different."
pub fn text_view_style(theme: &Theme) -> gpui_kit::component::text::TextViewStyle {
    gpui_kit::component::text::TextViewStyle::default().inline_code(gpui_kit::HighlightStyle {
        background_color: Some(theme.muted),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded theme must parse against gpui-kit's own schema — a typo
    /// in a colour key would otherwise surface as a runtime panic at startup
    /// only.
    #[test]
    fn ayu_dark_theme_set_parses() {
        let set: gpui_kit::component::ThemeSet = serde_json::from_str(AYU_DARK).unwrap();
        assert_eq!(set.name, "REactor");
        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.themes[0].name, "Ayu Dark");
        assert!(set.themes[0].mode.is_dark());
    }
}
