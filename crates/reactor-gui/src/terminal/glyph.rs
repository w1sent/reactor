//! Block elements, box drawing and Braille drawn as rectangles.
//!
//! A font's glyphs for these do not meet at the edges of a cell when the cell has a fractional
//! size, and a gap in a table's border or a progress bar shows. Pure: a character, a cell's size
//! and a line thickness in, rectangles out, with the cell's top left as the origin.

/// One filled rectangle of a glyph; `alpha` is below 1 for the shades.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub alpha: f32,
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect {
        x0,
        y0,
        x1,
        y1,
        alpha: 1.0,
    }
}

/// Arms of a box-drawing character: left, right, up, down. 0 none, 1 light, 2 heavy, 3 double.
fn arms(c: char) -> Option<[u8; 4]> {
    // One entry per character of U+2500 to U+2570, then U+2574 to U+257F; "L1R2" is a light arm to
    // the left and a heavy one to the right.
    const TABLE: [&str; 113] = [
        "L1R1", "L2R2", "U1D1", "U2D2", "L1R1", "L2R2", "U1D1", "U2D2", "L1R1", "L2R2", "U1D1",
        "U2D2", "R1D1", "R2D1", "R1D2", "R2D2", "L1D1", "L2D1", "L1D2", "L2D2", "R1U1", "R2U1",
        "R1U2", "R2U2", "L1U1", "L2U1", "L1U2", "L2U2", "R1U1D1", "R2U1D1", "R1U2D1", "R1U1D2",
        "R1U2D2", "R2U2D1", "R2U1D2", "R2U2D2", "L1U1D1", "L2U1D1", "L1U2D1", "L1U1D2", "L1U2D2",
        "L2U2D1", "L2U1D2", "L2U2D2", "L1R1D1", "L2R1D1", "L1R2D1", "L2R2D1", "L1R1D2", "L2R1D2",
        "L1R2D2", "L2R2D2", "L1R1U1", "L2R1U1", "L1R2U1", "L2R2U1", "L1R1U2", "L2R1U2", "L1R2U2",
        "L2R2U2", "L1R1U1D1", "L2R1U1D1", "L1R2U1D1", "L2R2U1D1", "L1R1U2D1", "L1R1U1D2",
        "L1R1U2D2", "L2R1U2D1", "L1R2U2D1", "L2R1U1D2", "L1R2U1D2", "L2R2U2D1", "L2R2U1D2",
        "L2R1U2D2", "L1R2U2D2", "L2R2U2D2", "L1R1", "L2R2", "U1D1", "U2D2", "L3R3", "U3D3", "R3D1",
        "R1D3", "R3D3", "L3D1", "L1D3", "L3D3", "R3U1", "R1U3", "R3U3", "L3U1", "L1U3", "L3U3",
        "R3U1D1", "R1U3D3", "R3U3D3", "L3U1D1", "L1U3D3", "L3U3D3", "L3R3D1", "L1R1D3", "L3R3D3",
        "L3R3U1", "L1R1U3", "L3R3U3", "L3R3U1D1", "L1R1U3D3", "L3R3U3D3", "R1D1", "L1D1", "L1U1",
        "R1U1",
    ];
    const HALVES: [&str; 12] = [
        "", "", "", "L1", "U1", "R1", "D1", "L2", "U2", "R2", "D2", "L1R2",
    ];
    const TAIL: [&str; 4] = ["U1D2", "L2R1", "U2D1", ""];

    let code = c as u32;
    let spec = match code {
        0x2500..=0x2570 => TABLE[(code - 0x2500) as usize],
        0x2574..=0x257B => HALVES[(code - 0x2574) as usize + 3],
        0x257C => HALVES[11],
        0x257D..=0x257F => TAIL[(code - 0x257D) as usize],
        _ => return None,
    };
    if spec.is_empty() {
        return None;
    }
    let mut out = [0u8; 4];
    let bytes = spec.as_bytes();
    for pair in bytes.chunks(2) {
        let weight = pair[1] - b'0';
        match pair[0] {
            b'L' => out[0] = weight,
            b'R' => out[1] = weight,
            b'U' => out[2] = weight,
            b'D' => out[3] = weight,
            _ => {}
        }
    }
    Some(out)
}

/// The rectangles that draw `c` in a cell of `w` by `h` pixels with a light line of `t`, or
/// `None` for a character the font draws.
pub fn rects(c: char, w: f32, h: f32, t: f32) -> Option<Vec<Rect>> {
    let code = c as u32;
    match code {
        0x2580..=0x259F => block(code, w, h),
        0x2500..=0x257F => lines(c, w, h, t),
        0x2800..=0x28FF => Some(braille(code, w, h)),
        _ => None,
    }
}

fn block(code: u32, w: f32, h: f32) -> Option<Vec<Rect>> {
    let eighth_h = h / 8.0;
    let eighth_w = w / 8.0;
    let r = match code {
        0x2580 => vec![rect(0.0, 0.0, w, h / 2.0)],
        0x2581..=0x2588 => {
            let n = (code - 0x2580) as f32;
            vec![rect(0.0, h - eighth_h * n, w, h)]
        }
        0x2589..=0x258F => {
            let n = (0x2590 - code) as f32;
            vec![rect(0.0, 0.0, eighth_w * n, h)]
        }
        0x2590 => vec![rect(w / 2.0, 0.0, w, h)],
        0x2591 => vec![Rect {
            alpha: 0.25,
            ..rect(0.0, 0.0, w, h)
        }],
        0x2592 => vec![Rect {
            alpha: 0.5,
            ..rect(0.0, 0.0, w, h)
        }],
        0x2593 => vec![Rect {
            alpha: 0.75,
            ..rect(0.0, 0.0, w, h)
        }],
        0x2594 => vec![rect(0.0, 0.0, w, eighth_h)],
        0x2595 => vec![rect(w - eighth_w, 0.0, w, h)],
        0x2596..=0x259F => {
            // Upper left, upper right, lower left, lower right.
            const MASK: [u8; 10] = [
                0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111,
            ];
            let m = MASK[(code - 0x2596) as usize];
            let (hw, hh) = (w / 2.0, h / 2.0);
            let mut out = Vec::new();
            if m & 0b1000 != 0 {
                out.push(rect(0.0, 0.0, hw, hh));
            }
            if m & 0b0100 != 0 {
                out.push(rect(hw, 0.0, w, hh));
            }
            if m & 0b0010 != 0 {
                out.push(rect(0.0, hh, hw, h));
            }
            if m & 0b0001 != 0 {
                out.push(rect(hw, hh, w, h));
            }
            out
        }
        _ => return None,
    };
    Some(r)
}

fn lines(c: char, w: f32, h: f32, t: f32) -> Option<Vec<Rect>> {
    let [left, right, up, down] = arms(c)?;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let heavy = (t * 2.0).max(t + 1.0);
    // How far a line of this weight reaches on either side of its axis.
    let half = |weight: u8| match weight {
        0 => 0.0,
        1 => t / 2.0,
        2 => heavy / 2.0,
        _ => t * 1.5,
    };
    let across_x = half(up).max(half(down)); // reach of the vertical arms, along x
    let across_y = half(left).max(half(right)); // reach of the horizontal arms, along y
    let mut out = Vec::new();

    // A horizontal arm: `x0..x1`, as one line, or two for a double one.
    let mut horizontal = |weight: u8, x0: f32, x1: f32| match weight {
        0 => {}
        1 => out.push(rect(x0, cy - t / 2.0, x1, cy + t / 2.0)),
        2 => out.push(rect(x0, cy - heavy / 2.0, x1, cy + heavy / 2.0)),
        _ => {
            out.push(rect(x0, cy - t * 1.5, x1, cy - t / 2.0));
            out.push(rect(x0, cy + t / 2.0, x1, cy + t * 1.5));
        }
    };
    horizontal(left, 0.0, cx + across_x);
    horizontal(right, cx - across_x, w);

    let mut vertical = |weight: u8, y0: f32, y1: f32| match weight {
        0 => {}
        1 => out.push(rect(cx - t / 2.0, y0, cx + t / 2.0, y1)),
        2 => out.push(rect(cx - heavy / 2.0, y0, cx + heavy / 2.0, y1)),
        _ => {
            out.push(rect(cx - t * 1.5, y0, cx - t / 2.0, y1));
            out.push(rect(cx + t / 2.0, y0, cx + t * 1.5, y1));
        }
    };
    vertical(up, 0.0, cy + across_y);
    vertical(down, cy - across_y, h);
    Some(out)
}

fn braille(code: u32, w: f32, h: f32) -> Vec<Rect> {
    let bits = code - 0x2800;
    // Dot n of the pattern: (column, row).
    const DOTS: [(usize, usize); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let dot = (w / 3.0).min(h / 5.5).max(1.0);
    let mut out = Vec::new();
    for (n, (col, row)) in DOTS.into_iter().enumerate() {
        if bits & (1 << n) == 0 {
            continue;
        }
        let cx = w * (0.30 + 0.40 * col as f32);
        let cy = h * (0.125 + 0.25 * row as f32);
        out.push(rect(
            cx - dot / 2.0,
            cy - dot / 2.0,
            cx + dot / 2.0,
            cy + dot / 2.0,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 8.0;
    const H: f32 = 16.0;

    fn covers(rs: &[Rect], x: f32, y: f32) -> bool {
        rs.iter()
            .any(|r| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1)
    }

    #[test]
    fn blocks_fill_the_fraction_they_name() {
        let full = rects('█', W, H, 1.0).unwrap();
        assert_eq!(full, vec![rect(0.0, 0.0, W, H)]);
        let lower_half = rects('▄', W, H, 1.0).unwrap();
        assert_eq!(lower_half, vec![rect(0.0, H / 2.0, W, H)]);
        let upper = rects('▀', W, H, 1.0).unwrap();
        assert_eq!(upper, vec![rect(0.0, 0.0, W, H / 2.0)]);
        let left_quarter = rects('▎', W, H, 1.0).unwrap();
        assert_eq!(left_quarter, vec![rect(0.0, 0.0, W / 4.0, H)]);
        let lower_eighth = rects('▁', W, H, 1.0).unwrap();
        assert_eq!(lower_eighth, vec![rect(0.0, H - H / 8.0, W, H)]);
    }

    #[test]
    fn shades_are_the_cell_at_a_quarter_a_half_and_three_quarters() {
        for (c, a) in [('░', 0.25), ('▒', 0.5), ('▓', 0.75)] {
            let r = rects(c, W, H, 1.0).unwrap();
            assert_eq!(r.len(), 1);
            assert_eq!(r[0].alpha, a);
            assert_eq!((r[0].x1, r[0].y1), (W, H));
        }
    }

    #[test]
    fn quadrants_are_the_quarters_they_show() {
        let ul = rects('▘', W, H, 1.0).unwrap();
        assert!(covers(&ul, 1.0, 1.0) && !covers(&ul, 6.0, 1.0) && !covers(&ul, 1.0, 12.0));
        let diag = rects('▚', W, H, 1.0).unwrap();
        assert!(covers(&diag, 1.0, 1.0) && covers(&diag, 6.0, 12.0));
        assert!(!covers(&diag, 6.0, 1.0) && !covers(&diag, 1.0, 12.0));
        let three = rects('▟', W, H, 1.0).unwrap();
        assert!(!covers(&three, 1.0, 1.0));
        assert!(covers(&three, 6.0, 1.0) && covers(&three, 1.0, 12.0) && covers(&three, 6.0, 12.0));
    }

    #[test]
    fn a_horizontal_line_runs_the_whole_cell_so_neighbours_join() {
        let r = rects('─', W, H, 1.0).unwrap();
        assert!(covers(&r, 0.0, H / 2.0) && covers(&r, W - 0.5, H / 2.0));
        assert!(!covers(&r, 4.0, 2.0));
    }

    #[test]
    fn a_corner_has_two_arms_and_they_meet_at_the_centre() {
        let r = rects('┌', W, H, 1.0).unwrap();
        // Right and down, not left or up.
        assert!(covers(&r, W - 0.5, H / 2.0) && covers(&r, W / 2.0, H - 0.5));
        assert!(!covers(&r, 0.5, H / 2.0) && !covers(&r, W / 2.0, 0.5));
        assert!(covers(&r, W / 2.0, H / 2.0), "no hole at the corner");
    }

    #[test]
    fn a_cross_covers_all_four_sides_and_a_heavy_line_is_thicker() {
        let cross = rects('┼', W, H, 1.0).unwrap();
        for (x, y) in [
            (0.2, H / 2.0),
            (W - 0.5, H / 2.0),
            (W / 2.0, 0.2),
            (W / 2.0, H - 0.5),
        ] {
            assert!(covers(&cross, x, y), "{x},{y}");
        }
        let thin = rects('─', W, H, 2.0).unwrap()[0];
        let thick = rects('━', W, H, 2.0).unwrap()[0];
        assert!(thick.y1 - thick.y0 > thin.y1 - thin.y0);
    }

    #[test]
    fn a_double_line_is_two_lines() {
        let r = rects('═', W, H, 2.0).unwrap();
        assert_eq!(r.len(), 4, "two lines, from each side");
        assert!(!covers(&r, 4.0, H / 2.0), "a gap between them");
    }

    #[test]
    fn mixed_weights_use_the_weight_of_each_arm() {
        let r = rects('╼', W, H, 2.0).unwrap();
        let left = r.iter().find(|r| r.x0 == 0.0).unwrap();
        let right = r.iter().find(|r| r.x1 == W).unwrap();
        assert!(right.y1 - right.y0 > left.y1 - left.y0);
    }

    #[test]
    fn every_straight_box_character_is_drawn_and_the_diagonals_are_the_fonts() {
        for code in 0x2500u32..=0x2570 {
            let c = char::from_u32(code).unwrap();
            assert!(
                rects(c, W, H, 1.0).is_some_and(|r| !r.is_empty()),
                "{code:x}"
            );
        }
        for code in 0x2574u32..=0x257F {
            let c = char::from_u32(code).unwrap();
            assert!(
                rects(c, W, H, 1.0).is_some_and(|r| !r.is_empty()),
                "{code:x}"
            );
        }
        for diagonal in ['╱', '╲', '╳'] {
            assert!(rects(diagonal, W, H, 1.0).is_none());
        }
        assert!(rects('a', W, H, 1.0).is_none());
    }

    #[test]
    fn braille_draws_one_dot_per_set_bit() {
        assert_eq!(rects('\u{2800}', W, H, 1.0).unwrap().len(), 0);
        assert_eq!(rects('\u{28ff}', W, H, 1.0).unwrap().len(), 8);
        assert_eq!(rects('\u{2803}', W, H, 1.0).unwrap().len(), 2);
    }
}
