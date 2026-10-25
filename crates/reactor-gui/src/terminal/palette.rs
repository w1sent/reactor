//! Colours: what a cell's colours are once the theme and the program's own changes are applied.
//!
//! Pure. The theme gives the sixteen and the defaults (`element.rs` reads them from its tokens;
//! no colour is added); the 256-colour cube and the grey ramp are computed.

use super::types::{Attrs, Cell, Color, PALETTE_BG, PALETTE_FG};

pub type Rgb = [u8; 3];

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// 0 to 7 normal, 8 to 15 bright.
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// 1.0 draws what the program asked for; a higher number pushes text away from its
    /// background until the contrast ratio reaches it (WCAG's, 1 to 21).
    pub min_contrast: f32,
}

/// The cell's colours as they are drawn: the text, and the background if it is not the panel's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub fg: Rgb,
    pub bg: Option<Rgb>,
}

/// The six levels each channel of the 6x6x6 cube takes.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

pub fn cube(index: u8) -> Rgb {
    let n = index.saturating_sub(16) as usize;
    [CUBE[(n / 36) % 6], CUBE[(n / 6) % 6], CUBE[n % 6]]
}

pub fn grey(index: u8) -> Rgb {
    let level = 8 + 10 * index.saturating_sub(232);
    [level; 3]
}

/// `a` moved `amount` (0 to 1) of the way to `b`.
pub fn mix(a: Rgb, b: Rgb, amount: f32) -> Rgb {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * amount).round() as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

fn luminance(c: Rgb) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `fg` pushed away from `bg`, towards white on a dark one and black on a light one, until it
/// reads at `min`. Unchanged when it already does.
pub fn ensure_contrast(fg: Rgb, bg: Rgb, min: f32) -> Rgb {
    if min <= 1.0 || contrast_ratio(fg, bg) >= min {
        return fg;
    }
    let target = if luminance(bg) < 0.4 {
        [255; 3]
    } else {
        [0; 3]
    };
    let mut out = fg;
    for step in 1..=20 {
        out = mix(fg, target, step as f32 / 20.0);
        if contrast_ratio(out, bg) >= min {
            break;
        }
    }
    out
}

impl Palette {
    /// The colour a program named, with whatever it changed (OSC 4, 10, 11) on top.
    pub fn resolve(&self, color: Color, changed: &dyn Fn(usize) -> Option<Rgb>) -> Rgb {
        match color {
            Color::Rgb(r, g, b) => [r, g, b],
            Color::DefaultFg => changed(PALETTE_FG).unwrap_or(self.fg),
            Color::DefaultBg => changed(PALETTE_BG).unwrap_or(self.bg),
            Color::Indexed(i) => changed(i as usize).unwrap_or(match i {
                0..=15 => self.ansi[i as usize],
                16..=231 => cube(i),
                _ => grey(i),
            }),
        }
    }

    pub fn cell(&self, cell: &Cell, changed: &dyn Fn(usize) -> Option<Rgb>) -> Resolved {
        let mut fg = self.resolve(cell.fg, changed);
        let mut bg = self.resolve(cell.bg, changed);
        let mut painted = cell.bg != Color::DefaultBg;
        if cell.attrs.has(Attrs::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            painted = true;
        }
        let behind = if painted {
            bg
        } else {
            self.resolve(Color::DefaultBg, changed)
        };
        if cell.attrs.has(Attrs::DIM) {
            fg = mix(fg, behind, 0.45);
        }
        if cell.attrs.has(Attrs::HIDDEN) {
            fg = behind;
        } else {
            fg = ensure_contrast(fg, behind, self.min_contrast);
        }
        Resolved {
            fg,
            bg: painted.then_some(bg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let mut ansi = [[0u8; 3]; 16];
        for (i, c) in ansi.iter_mut().enumerate() {
            *c = [i as u8 * 10, 0, 0];
        }
        Palette {
            ansi,
            fg: [200, 200, 200],
            bg: [10, 10, 10],
            cursor: [255, 200, 0],
            selection: [50, 50, 80],
            min_contrast: 1.0,
        }
    }

    fn none(_: usize) -> Option<Rgb> {
        None
    }

    #[test]
    fn the_cube_and_the_greys_are_computed() {
        assert_eq!(cube(16), [0, 0, 0]);
        assert_eq!(cube(21), [0, 0, 255]);
        assert_eq!(cube(196), [255, 0, 0]);
        assert_eq!(cube(231), [255, 255, 255]);
        assert_eq!(cube(59), [95, 95, 95]);
        assert_eq!(grey(232), [8, 8, 8]);
        assert_eq!(grey(255), [238, 238, 238]);
    }

    #[test]
    fn colours_come_from_the_theme_the_cube_or_the_program() {
        let p = palette();
        assert_eq!(p.resolve(Color::Indexed(3), &none), [30, 0, 0]);
        assert_eq!(p.resolve(Color::Indexed(200), &none), cube(200));
        assert_eq!(p.resolve(Color::Rgb(1, 2, 3), &none), [1, 2, 3]);
        assert_eq!(p.resolve(Color::DefaultFg, &none), [200; 3]);
        // The program changed colour 3 and the default foreground.
        let changed = |i: usize| match i {
            3 => Some([9, 9, 9]),
            256 => Some([1, 1, 1]),
            _ => None,
        };
        assert_eq!(p.resolve(Color::Indexed(3), &changed), [9, 9, 9]);
        assert_eq!(p.resolve(Color::DefaultFg, &changed), [1, 1, 1]);
        assert_eq!(p.resolve(Color::Indexed(4), &changed), [40, 0, 0]);
    }

    #[test]
    fn a_default_background_is_not_painted() {
        let p = palette();
        let plain = Cell::default();
        assert_eq!(
            p.cell(&plain, &none),
            Resolved {
                fg: [200; 3],
                bg: None
            }
        );
        let red_bg = Cell {
            bg: Color::Indexed(1),
            ..Cell::default()
        };
        assert_eq!(p.cell(&red_bg, &none).bg, Some([10, 0, 0]));
    }

    #[test]
    fn inverse_swaps_and_paints() {
        let p = palette();
        let cell = Cell {
            attrs: Attrs(Attrs::INVERSE),
            ..Cell::default()
        };
        assert_eq!(
            p.cell(&cell, &none),
            Resolved {
                fg: [10; 3],
                bg: Some([200; 3])
            }
        );
    }

    #[test]
    fn dim_and_hidden() {
        let p = palette();
        let dim = Cell {
            attrs: Attrs(Attrs::DIM),
            ..Cell::default()
        };
        let r = p.cell(&dim, &none);
        assert!(r.fg[0] < 200 && r.fg[0] > 10);
        let hidden = Cell {
            attrs: Attrs(Attrs::HIDDEN),
            ..Cell::default()
        };
        assert_eq!(p.cell(&hidden, &none).fg, [10; 3]);
    }

    #[test]
    fn minimum_contrast_lifts_unreadable_text_and_leaves_the_rest() {
        let bg = [10, 10, 10];
        let dark_blue = [0, 0, 60];
        assert!(contrast_ratio(dark_blue, bg) < 3.0);
        let lifted = ensure_contrast(dark_blue, bg, 4.5);
        assert!(contrast_ratio(lifted, bg) >= 4.5, "{lifted:?}");
        assert_eq!(ensure_contrast([200; 3], bg, 4.5), [200; 3]);
        assert_eq!(
            ensure_contrast(dark_blue, bg, 1.0),
            dark_blue,
            "off by default"
        );
        // On a light background it goes the other way.
        let on_light = ensure_contrast([230, 230, 160], [250, 250, 250], 4.5);
        assert!(contrast_ratio(on_light, [250, 250, 250]) >= 4.5);
        assert!(on_light[0] < 230);
    }
}
