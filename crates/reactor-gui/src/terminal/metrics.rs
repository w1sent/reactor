//! Where the grid is on screen: from a pointer to a cell, from a size to rows and columns, and
//! the scrollbar's thumb. Plain numbers in pixels, no gpui, so it is tested as it is.

use super::types::Side;

/// The width of the strip at the right edge that the scrollbar lives in.
pub const SCROLLBAR_W: f32 = 10.0;
/// The grid's margin at the top and bottom.
pub const PAD_Y: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// The element's bounds, in window coordinates.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Where cell (0, 0) starts.
    pub origin_x: f32,
    pub origin_y: f32,
    pub cell_w: f32,
    pub line_h: f32,
    pub rows: usize,
    pub cols: usize,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            origin_x: 0.0,
            origin_y: 0.0,
            cell_w: 8.0,
            line_h: 16.0,
            rows: 24,
            cols: 80,
        }
    }
}

/// How many rows and columns fit in `w` by `h` pixels, with a left margin of one cell and the
/// scrollbar's strip on the right. Never below one of each.
pub fn grid_for(w: f32, h: f32, cell_w: f32, line_h: f32) -> (usize, usize) {
    let cols = ((w - cell_w - SCROLLBAR_W) / cell_w).floor().max(1.0) as usize;
    let rows = ((h - 2.0 * PAD_Y) / line_h).floor().max(1.0) as usize;
    (rows, cols)
}

impl Metrics {
    pub fn new(x: f32, y: f32, w: f32, h: f32, cell_w: f32, line_h: f32) -> Self {
        let (rows, cols) = grid_for(w, h, cell_w, line_h);
        Self {
            x,
            y,
            w,
            h,
            origin_x: x + cell_w,
            origin_y: y + PAD_Y,
            cell_w,
            line_h,
            rows,
            cols,
        }
    }

    /// The cell under a pointer, the half of it the pointer is in, and whether the pointer is
    /// outside the grid vertically (-1 above, 1 below). The row is not clamped, so a drag past
    /// the edge says how far; the column is.
    pub fn cell_at(&self, px: f32, py: f32) -> (i32, usize, Side, i32) {
        let fx = (px - self.origin_x) / self.cell_w;
        let fy = (py - self.origin_y) / self.line_h;
        let col = fx.floor().clamp(0.0, self.cols as f32 - 1.0) as usize;
        let side = if fx - fx.floor() < 0.5 || fx < 0.0 {
            Side::Left
        } else {
            Side::Right
        };
        let row = fy.floor() as i32;
        let outside = if py < self.origin_y {
            -1
        } else if py >= self.origin_y + self.rows as f32 * self.line_h {
            1
        } else {
            0
        };
        (row, col, side, outside)
    }

    pub fn in_bounds(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn in_scrollbar(&self, px: f32, py: f32) -> bool {
        self.in_bounds(px, py) && px >= self.x + self.w - SCROLLBAR_W
    }

    /// The thumb of the scrollbar as a top and a height in pixels from the top of the bounds, or
    /// `None` when there is no history to scroll.
    pub fn thumb(&self, history: usize, display_offset: usize) -> Option<(f32, f32)> {
        if history == 0 {
            return None;
        }
        let total = (history + self.rows) as f32;
        let track = self.h - 2.0 * PAD_Y;
        let height = (self.rows as f32 / total * track).max(18.0).min(track);
        let travel = track - height;
        let top = (history - display_offset.min(history)) as f32 / history as f32 * travel;
        Some((PAD_Y + top, height))
    }

    /// The display offset a pointer at `py` on the scrollbar stands for, the thumb centred on it.
    pub fn offset_at(&self, py: f32, history: usize) -> usize {
        let Some((_, height)) = self.thumb(history, 0) else {
            return 0;
        };
        let track = self.h - 2.0 * PAD_Y;
        let travel = (track - height).max(1.0);
        let top = (py - self.y - PAD_Y - height / 2.0).clamp(0.0, travel);
        let from_top = top / travel * history as f32;
        history - (from_top.round() as usize).min(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Metrics {
        // 800 x 400 at (100, 50) with 8 x 16 cells.
        Metrics::new(100.0, 50.0, 800.0, 400.0, 8.0, 16.0)
    }

    #[test]
    fn the_grid_is_what_fits_never_below_one() {
        assert_eq!(grid_for(800.0, 400.0, 8.0, 16.0), (24, 97));
        assert_eq!(grid_for(1.0, 1.0, 8.0, 16.0), (1, 1));
        assert_eq!(grid_for(0.0, 0.0, 8.0, 16.0), (1, 1));
        let m = m();
        assert_eq!((m.rows, m.cols), (24, 97));
    }

    #[test]
    fn a_pointer_finds_its_cell_and_the_half_of_it() {
        let m = m();
        let (row, col, side, outside) = m.cell_at(m.origin_x + 1.0, m.origin_y + 1.0);
        assert_eq!((row, col, side, outside), (0, 0, Side::Left, 0));
        let (row, col, side, _) =
            m.cell_at(m.origin_x + 8.0 * 5.0 + 6.0, m.origin_y + 16.0 * 3.0 + 15.0);
        assert_eq!((row, col, side), (3, 5, Side::Right));
        // Left of the grid is column 0; right of it is the last column.
        assert_eq!(m.cell_at(m.x - 50.0, m.origin_y).1, 0);
        assert_eq!(m.cell_at(m.x + 5000.0, m.origin_y).1, 96);
    }

    #[test]
    fn a_drag_past_the_edge_says_which_edge_and_how_far() {
        let m = m();
        let (row, _, _, outside) = m.cell_at(m.origin_x, m.origin_y - 40.0);
        assert!(row < 0 && outside == -1);
        let (row, _, _, outside) = m.cell_at(m.origin_x, m.origin_y + 24.0 * 16.0 + 20.0);
        assert!(row >= 24 && outside == 1);
    }

    #[test]
    fn the_scrollbar_has_a_strip_and_a_thumb_that_moves() {
        let m = m();
        assert!(m.in_scrollbar(m.x + m.w - 2.0, m.y + 10.0));
        assert!(!m.in_scrollbar(m.x + 20.0, m.y + 10.0));
        assert_eq!(m.thumb(0, 0), None);
        let (top_at_bottom, h) = m.thumb(1000, 0).unwrap();
        let (top_at_top, _) = m.thumb(1000, 1000).unwrap();
        assert!(
            top_at_top < top_at_bottom,
            "scrolled up the thumb is higher"
        );
        assert!(h >= 18.0 && top_at_bottom + h <= m.h);
    }

    #[test]
    fn a_pointer_on_the_scrollbar_maps_back_to_an_offset() {
        let m = m();
        assert_eq!(
            m.offset_at(m.y + m.h, 1000),
            0,
            "the bottom is the live end"
        );
        assert_eq!(m.offset_at(m.y, 1000), 1000, "the top is the oldest");
        let mid = m.offset_at(m.y + m.h / 2.0, 1000);
        assert!((400..=600).contains(&mid), "{mid}");
        assert_eq!(m.offset_at(m.y, 0), 0);
    }
}
