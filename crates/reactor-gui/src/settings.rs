//! The GUI's own settings: fonts and sizes per part of the window, and a few behaviours.
//!
//! These are about *this frontend* and live in `~/.reactor/gui.json` (`ui`), apart from the
//! agent's settings (`settings.json`: models, context budget) which the Context panel and the
//! slash commands own. Every field is optional: unset means "the theme's own", so a fresh
//! install looks exactly as before and "reset" is just clearing a field.
//!
//! Two of the font slots, [`Slot::Interface`] and [`Slot::Mono`], are the theme's base fonts —
//! every component reads them — and are applied to the theme ([`apply`]). The rest are
//! per-area overrides the panels read through [`UiSettings::text`].

use std::collections::BTreeMap;

use gpui_kit::component::Theme;
use gpui_kit::{App, Global, Pixels, SharedString, px};
use serde::{Deserialize, Serialize};

/// The smallest and largest size the settings accept, in pixels.
pub const MIN_SIZE: f32 = 8.0;
pub const MAX_SIZE: f32 = 40.0;

/// A part of the window with a font of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// Labels, buttons, panels: the theme's UI font.
    Interface,
    /// The conversation's text.
    Transcript,
    /// Tool calls, their arguments and their output.
    Tools,
    /// The prompt input.
    Prompt,
    /// The console panels.
    Console,
    /// The theme's monospace font: code blocks, tool and toolset names, the `/` popup.
    Mono,
}

impl Slot {
    pub const ALL: [Slot; 6] = [
        Slot::Interface,
        Slot::Transcript,
        Slot::Tools,
        Slot::Prompt,
        Slot::Console,
        Slot::Mono,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Slot::Interface => "Interface",
            Slot::Transcript => "Transcript",
            Slot::Tools => "Tool calls and output",
            Slot::Prompt => "Prompt input",
            Slot::Console => "Console",
            Slot::Mono => "Monospace (code, names)",
        }
    }
}

/// One slot's choice; `None` is the default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FontPref {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub fonts: BTreeMap<Slot, FontPref>,
    /// How much of a tool's output the transcript shows before cutting it (the session keeps it all).
    pub tool_output_chars: usize,
    /// Open thinking blocks by default.
    pub expand_thinking: bool,
    /// Dim the transcript rows a context reduction has taken out of the model's view.
    pub dim_reduced: bool,
    /// The layout a window opens with: `default`, `focus`, `analysis` or `catalogue`.
    pub start_layout: String,
    /// How long a notification popup stays, in seconds.
    pub notice_seconds: u32,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            fonts: BTreeMap::new(),
            tool_output_chars: 2000,
            expand_thinking: false,
            dim_reduced: true,
            start_layout: "default".into(),
            notice_seconds: 6,
        }
    }
}

/// How much tool output may be shown: the bounds the settings accept.
/// How long a notification popup may stay, in seconds.
pub const NOTICE_SECONDS: std::ops::RangeInclusive<u32> = 1..=120;

pub const OUTPUT_CHARS: std::ops::RangeInclusive<usize> = 200..=50_000;

impl UiSettings {
    pub fn family(&self, slot: Slot) -> Option<&str> {
        self.fonts.get(&slot).and_then(|f| f.family.as_deref())
    }

    pub fn size(&self, slot: Slot) -> Option<f32> {
        self.fonts.get(&slot).and_then(|f| f.size)
    }

    pub fn set_family(&mut self, slot: Slot, family: Option<String>) {
        let family = family
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty());
        self.fonts.entry(slot).or_default().family = family;
        self.tidy(slot);
    }

    pub fn set_size(&mut self, slot: Slot, size: Option<f32>) {
        self.fonts.entry(slot).or_default().size = size.map(|s| s.clamp(MIN_SIZE, MAX_SIZE));
        self.tidy(slot);
    }

    fn tidy(&mut self, slot: Slot) {
        if self.fonts.get(&slot) == Some(&FontPref::default()) {
            self.fonts.remove(&slot);
        }
    }

    /// Put values in range after loading a hand-edited file.
    pub fn normalize(&mut self) {
        for pref in self.fonts.values_mut() {
            pref.size = pref.size.map(|s| {
                if s.is_finite() {
                    s.clamp(MIN_SIZE, MAX_SIZE)
                } else {
                    MIN_SIZE
                }
            });
            pref.family = pref
                .family
                .take()
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty());
        }
        self.fonts.retain(|_, p| *p != FontPref::default());
        self.tool_output_chars = self
            .tool_output_chars
            .clamp(*OUTPUT_CHARS.start(), *OUTPUT_CHARS.end());
        self.notice_seconds = self
            .notice_seconds
            .clamp(*NOTICE_SECONDS.start(), *NOTICE_SECONDS.end());
        if crate::layout::LayoutPreset::ALL
            .iter()
            .all(|p| !p.label().eq_ignore_ascii_case(&self.start_layout))
        {
            self.start_layout = "default".into();
        }
    }

    pub fn start_layout(&self) -> crate::layout::LayoutPreset {
        crate::layout::LayoutPreset::ALL
            .iter()
            .copied()
            .find(|p| p.label().eq_ignore_ascii_case(&self.start_layout))
            .unwrap_or(crate::layout::LayoutPreset::Default)
    }

    /// The font family and size an area renders with: the choice, else what the theme gives
    /// that kind of area. (`Interface` and `Mono` are already in the theme — see [`apply`].)
    pub fn text(&self, slot: Slot, theme: &Theme) -> (SharedString, Pixels) {
        let (family, size) = match slot {
            Slot::Interface | Slot::Transcript | Slot::Prompt => {
                (theme.font_family.clone(), theme.font_size)
            }
            Slot::Mono => (theme.mono_font_family.clone(), theme.mono_font_size),
            // What the cards and the console used before they were configurable.
            Slot::Tools => (theme.mono_font_family.clone(), theme.mono_font_size * 0.8),
            Slot::Console => (theme.mono_font_family.clone(), theme.mono_font_size * 0.85),
        };
        (
            self.family(slot)
                .map(|f| SharedString::from(f.to_string()))
                .unwrap_or(family),
            self.size(slot).map(px).unwrap_or(size),
        )
    }
}

/// The theme's own fonts, kept so clearing a choice restores them.
#[derive(Clone)]
pub struct ThemeFonts {
    family: SharedString,
    size: Pixels,
    mono_family: SharedString,
    mono_size: Pixels,
}

impl Global for ThemeFonts {}

/// Remember the theme's fonts. Call once, after the theme is installed and before [`apply`].
pub fn capture_theme_fonts(cx: &mut App) {
    let t = Theme::global(cx);
    let fonts = ThemeFonts {
        family: t.font_family.clone(),
        size: t.font_size,
        mono_family: t.mono_font_family.clone(),
        mono_size: t.mono_font_size,
    };
    cx.set_global(fonts);
}

/// The size a slot has when nothing is chosen for it, given what else is chosen: the theme's own
/// for the two base fonts, and for the rest what the base font they follow gives.
pub fn default_size(slot: Slot, ui: &UiSettings, cx: &App) -> f32 {
    let Some(base) = cx.try_global::<ThemeFonts>() else {
        return 14.0;
    };
    let (ui_size, mono_size) = (
        ui.size(Slot::Interface).unwrap_or(f32::from(base.size)),
        ui.size(Slot::Mono).unwrap_or(f32::from(base.mono_size)),
    );
    match slot {
        Slot::Interface => f32::from(base.size),
        Slot::Mono => f32::from(base.mono_size),
        Slot::Transcript | Slot::Prompt => ui_size,
        Slot::Tools => mono_size * 0.8,
        Slot::Console => mono_size * 0.85,
    }
}

/// Write the base fonts into the theme and redraw.
pub fn apply(settings: &UiSettings, cx: &mut App) {
    let Some(base) = cx.try_global::<ThemeFonts>().cloned() else {
        return;
    };
    let theme = Theme::global_mut(cx);
    theme.font_family = settings
        .family(Slot::Interface)
        .map(|f| SharedString::from(f.to_string()))
        .unwrap_or(base.family);
    theme.font_size = settings.size(Slot::Interface).map(px).unwrap_or(base.size);
    theme.mono_font_family = settings
        .family(Slot::Mono)
        .map(|f| SharedString::from(f.to_string()))
        .unwrap_or(base.mono_family);
    theme.mono_font_size = settings.size(Slot::Mono).map(px).unwrap_or(base.mono_size);
    cx.refresh_windows();
}

/// Whether the system has a font with this name.
pub fn font_exists(name: &str, cx: &App) -> bool {
    cx.text_system()
        .all_font_names()
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_install_changes_nothing() {
        let s = UiSettings::default();
        assert!(s.fonts.is_empty());
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"fonts":{},"tool_output_chars":2000,"expand_thinking":false,"dim_reduced":true,"start_layout":"default","notice_seconds":6}"#
        );
    }

    #[test]
    fn a_missing_or_partial_file_fills_in_the_defaults() {
        let s: UiSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, UiSettings::default());
        let s: UiSettings = serde_json::from_str(
            r#"{"fonts":{"transcript":{"size":18.0}},"expand_thinking":true}"#,
        )
        .unwrap();
        assert_eq!(s.size(Slot::Transcript), Some(18.0));
        assert_eq!(s.family(Slot::Transcript), None);
        assert!(s.expand_thinking && s.dim_reduced);
    }

    #[test]
    fn sizes_are_kept_in_range_and_clearing_a_choice_removes_its_entry() {
        let mut s = UiSettings::default();
        s.set_size(Slot::Tools, Some(2.0));
        assert_eq!(s.size(Slot::Tools), Some(MIN_SIZE));
        s.set_size(Slot::Tools, Some(500.0));
        assert_eq!(s.size(Slot::Tools), Some(MAX_SIZE));
        s.set_size(Slot::Tools, None);
        assert!(
            s.fonts.is_empty(),
            "nothing set means no entry, so the file stays small"
        );
        s.set_family(Slot::Console, Some("  ".into()));
        assert!(s.fonts.is_empty(), "a blank family is the default");
    }

    #[test]
    fn a_hand_edited_file_is_put_in_range() {
        let mut s: UiSettings = serde_json::from_str(r#"{"fonts":{"mono":{"size":1000,"family":" "}},"tool_output_chars":5,"start_layout":"nonsense"}"#).unwrap();
        s.normalize();
        assert!(
            s.fonts
                .get(&Slot::Mono)
                .is_some_and(|p| p.size == Some(MAX_SIZE) && p.family.is_none())
        );
        assert_eq!(s.tool_output_chars, *OUTPUT_CHARS.start());
        assert_eq!(s.start_layout, "default");
    }

    #[test]
    fn the_start_layout_resolves_by_name_without_regard_to_case() {
        let s = UiSettings {
            start_layout: "Analysis".into(),
            ..Default::default()
        };
        assert_eq!(s.start_layout(), crate::layout::LayoutPreset::Analysis);
    }
}
