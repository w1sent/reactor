//! From a row of cells to what is drawn: backgrounds, text runs, rectangles for the characters a
//! font cannot join up, and decorations. No gpui here, so what ends up on screen is tested.

use super::glyph;
use super::palette::{Palette, Resolved, Rgb};
use super::types::{Attrs, Cell};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Single,
    Double,
    Curl,
    Dotted,
    Dashed,
    Strike,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub col: usize,
    /// How many columns the run covers; a character with marks is still one.
    pub cols: usize,
    pub text: String,
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
    /// A double-width character, shaped on its own across two cells.
    pub wide: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoration {
    pub col: usize,
    pub cols: usize,
    pub line: Line,
    pub color: Rgb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub col: usize,
    pub c: char,
    pub fg: Rgb,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RowDraw {
    /// `(first column, columns, colour)` of every stretch with a background of its own.
    pub backgrounds: Vec<(usize, usize, Rgb)>,
    pub runs: Vec<Run>,
    /// Characters drawn as rectangles.
    pub pieces: Vec<Piece>,
    pub decorations: Vec<Decoration>,
}

fn is_drawn_by_rects(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0x2800..=0x28FF)
        && glyph::rects(c, 8.0, 16.0, 1.0).is_some()
}

#[derive(PartialEq, Eq)]
struct Style {
    fg: Rgb,
    bold: bool,
    italic: bool,
}

/// What to draw for one row. `cursor_col` is the column of a block cursor on this row, whose
/// character is drawn in the background colour so it reads on top of the block.
pub fn build(
    row: &[Cell],
    palette: &Palette,
    changed: &dyn Fn(usize) -> Option<Rgb>,
    cursor_col: Option<usize>,
) -> RowDraw {
    let mut draw = RowDraw::default();
    let resolved: Vec<Resolved> = row.iter().map(|c| palette.cell(c, changed)).collect();

    // Backgrounds, merged across equal neighbours.
    let mut col = 0;
    while col < row.len() {
        let Some(bg) = resolved[col].bg else {
            col += 1;
            continue;
        };
        let start = col;
        while col < row.len() && resolved[col].bg == Some(bg) {
            col += 1;
        }
        draw.backgrounds.push((start, col - start, bg));
    }

    // Text, in runs of one style; a blank continues a run and never starts one.
    let mut current: Option<(Run, Style)> = None;
    let flush = |current: &mut Option<(Run, Style)>, draw: &mut RowDraw| {
        if let Some((mut run, _)) = current.take() {
            let kept = run.text.trim_end_matches(' ').len();
            let trimmed = run.text[kept..].chars().count();
            run.text.truncate(kept);
            run.cols -= trimmed;
            if !run.text.is_empty() {
                draw.runs.push(run);
            }
        }
    };

    for (col, cell) in row.iter().enumerate() {
        if cell.attrs.has(Attrs::SPACER) {
            continue;
        }
        let r = resolved[col];
        let fg = if cursor_col == Some(col) {
            palette.resolve(super::types::Color::DefaultBg, changed)
        } else {
            r.fg
        };

        // Decorations first: they apply to blanks too.
        let wide = cell.attrs.has(Attrs::WIDE);
        let span = if wide { 2 } else { 1 };
        let underline_color = cell
            .extra
            .as_ref()
            .and_then(|e| e.underline_color)
            .map_or(fg, |c| palette.resolve(c, changed));
        let attrs = cell.attrs;
        let line = if attrs.has(Attrs::DOUBLE_UNDERLINE) {
            Some(Line::Double)
        } else if attrs.has(Attrs::CURL) {
            Some(Line::Curl)
        } else if attrs.has(Attrs::DOTTED) {
            Some(Line::Dotted)
        } else if attrs.has(Attrs::DASHED) {
            Some(Line::Dashed)
        } else if attrs.has(Attrs::UNDERLINE) {
            Some(Line::Single)
        } else {
            None
        };
        if let Some(line) = line {
            push_decoration(&mut draw, col, span, line, underline_color);
        }
        if attrs.has(Attrs::STRIKE) {
            push_decoration(&mut draw, col, span, Line::Strike, fg);
        }

        let c = if cell.c == '\0' { ' ' } else { cell.c };
        if is_drawn_by_rects(c) && !attrs.has(Attrs::HIDDEN) {
            flush(&mut current, &mut draw);
            draw.pieces.push(Piece { col, c, fg });
            continue;
        }

        let blank = cell.is_blank();
        let style = Style {
            fg,
            bold: attrs.has(Attrs::BOLD),
            italic: attrs.has(Attrs::ITALIC),
        };

        if wide {
            flush(&mut current, &mut draw);
            if !blank {
                let mut text = String::from(c);
                if let Some(e) = &cell.extra {
                    text.extend(e.zerowidth.iter());
                }
                draw.runs.push(Run {
                    col,
                    cols: 2,
                    text,
                    fg,
                    bold: style.bold,
                    italic: style.italic,
                    wide: true,
                });
            }
            continue;
        }

        match current.as_mut() {
            Some((run, s)) if blank || *s == style => {
                run.text.push(c);
                if let Some(e) = &cell.extra {
                    run.text.extend(e.zerowidth.iter());
                }
                run.cols += 1;
            }
            _ => {
                flush(&mut current, &mut draw);
                if !blank {
                    let mut text = String::from(c);
                    if let Some(e) = &cell.extra {
                        text.extend(e.zerowidth.iter());
                    }
                    let run = Run {
                        col,
                        cols: 1,
                        text,
                        fg,
                        bold: style.bold,
                        italic: style.italic,
                        wide: false,
                    };
                    current = Some((run, style));
                }
            }
        }
    }
    flush(&mut current, &mut draw);
    draw
}

/// Add a decoration, joining the one before it when it continues it.
fn push_decoration(draw: &mut RowDraw, col: usize, cols: usize, line: Line, color: Rgb) {
    if let Some(last) = draw.decorations.last_mut()
        && last.line == line
        && last.color == color
        && last.col + last.cols == col
    {
        last.cols += cols;
        return;
    }
    draw.decorations.push(Decoration {
        col,
        cols,
        line,
        color,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::types::{Color, Extra};

    fn palette() -> Palette {
        let mut ansi = [[1, 1, 1]; 16];
        ansi[2] = [2, 2, 2];
        Palette {
            ansi,
            fg: [200; 3],
            bg: [10; 3],
            cursor: [255, 200, 0],
            selection: [0; 3],
            min_contrast: 1.0,
        }
    }

    fn none(_: usize) -> Option<Rgb> {
        None
    }

    fn row(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|c| Cell {
                c,
                ..Cell::default()
            })
            .collect()
    }

    fn texts(d: &RowDraw) -> Vec<(usize, &str)> {
        d.runs.iter().map(|r| (r.col, r.text.as_str())).collect()
    }

    #[test]
    fn plain_text_is_one_run_without_its_blanks_at_the_ends() {
        let d = build(&row("  hello world   "), &palette(), &none, None);
        assert_eq!(texts(&d), [(2, "hello world")]);
        assert_eq!(d.runs[0].cols, 11);
        assert!(d.backgrounds.is_empty());
    }

    #[test]
    fn a_change_of_colour_starts_a_new_run_and_blanks_between_stay_in_the_first() {
        let mut r = row("ab  cd");
        for c in &mut r[4..] {
            c.fg = Color::Indexed(1);
        }
        let d = build(&r, &palette(), &none, None);
        assert_eq!(texts(&d), [(0, "ab"), (4, "cd")]);
        assert_ne!(d.runs[0].fg, d.runs[1].fg);
    }

    #[test]
    fn bold_and_italic_split_runs() {
        let mut r = row("abcd");
        r[1].attrs = Attrs(Attrs::BOLD);
        r[2].attrs = Attrs(Attrs::BOLD);
        let d = build(&r, &palette(), &none, None);
        assert_eq!(texts(&d), [(0, "a"), (1, "bc"), (3, "d")]);
        assert!(d.runs[1].bold && !d.runs[0].bold);
    }

    #[test]
    fn backgrounds_merge_across_equal_neighbours() {
        let mut r = row("abcdef");
        for (i, c) in r.iter_mut().enumerate() {
            c.bg = if (1..4).contains(&i) {
                Color::Indexed(1)
            } else if i == 4 {
                Color::Indexed(2)
            } else {
                Color::DefaultBg
            };
        }
        let d = build(&r, &palette(), &none, None);
        assert_eq!(d.backgrounds, [(1, 3, [1, 1, 1]), (4, 1, [2, 2, 2])]);
    }

    #[test]
    fn a_wide_character_is_shaped_alone_across_two_columns() {
        let mut r = row("a日 b");
        r[1].attrs = Attrs(Attrs::WIDE);
        r.insert(
            2,
            Cell {
                attrs: Attrs(Attrs::SPACER),
                ..Cell::default()
            },
        );
        let d = build(&r, &palette(), &none, None);
        assert_eq!(texts(&d), [(0, "a"), (1, "日"), (4, "b")]);
        assert!(d.runs[1].wide && d.runs[1].cols == 2);
    }

    #[test]
    fn combining_marks_ride_with_their_character() {
        let mut r = row("e");
        r[0].extra = Some(Box::new(Extra {
            zerowidth: vec!['\u{301}'],
            ..Extra::default()
        }));
        let d = build(&r, &palette(), &none, None);
        assert_eq!(d.runs[0].text, "e\u{301}");
        assert_eq!(d.runs[0].cols, 1);
    }

    #[test]
    fn boxes_and_blocks_are_pieces_and_split_the_text() {
        let d = build(&row("ab─█cd"), &palette(), &none, None);
        assert_eq!(texts(&d), [(0, "ab"), (4, "cd")]);
        assert_eq!(
            d.pieces.iter().map(|p| (p.col, p.c)).collect::<Vec<_>>(),
            [(2, '─'), (3, '█')]
        );
        // A diagonal is the font's.
        let d = build(&row("a╱b"), &palette(), &none, None);
        assert!(d.pieces.is_empty());
    }

    #[test]
    fn underlines_join_across_cells_and_cover_blanks() {
        let mut r = row("ab  cd");
        for c in &mut r[..4] {
            c.attrs = Attrs(Attrs::UNDERLINE);
        }
        r[5].attrs = Attrs(Attrs::STRIKE);
        let d = build(&r, &palette(), &none, None);
        assert_eq!(d.decorations.len(), 2);
        assert_eq!(
            (
                d.decorations[0].col,
                d.decorations[0].cols,
                d.decorations[0].line
            ),
            (0, 4, Line::Single)
        );
        assert_eq!(
            (d.decorations[1].col, d.decorations[1].line),
            (5, Line::Strike)
        );
    }

    #[test]
    fn the_kinds_of_underline_are_told_apart() {
        for (flag, line) in [
            (Attrs::DOUBLE_UNDERLINE, Line::Double),
            (Attrs::CURL, Line::Curl),
            (Attrs::DOTTED, Line::Dotted),
            (Attrs::DASHED, Line::Dashed),
        ] {
            let mut r = row("a");
            r[0].attrs = Attrs(flag);
            assert_eq!(build(&r, &palette(), &none, None).decorations[0].line, line);
        }
    }

    #[test]
    fn a_block_cursor_draws_its_character_in_the_background_colour() {
        let d = build(&row("abc"), &palette(), &none, Some(1));
        assert_eq!(texts(&d), [(0, "a"), (1, "b"), (2, "c")]);
        assert_eq!(d.runs[1].fg, [10; 3]);
        assert_eq!(d.runs[0].fg, [200; 3]);
    }

    #[test]
    fn hidden_text_is_not_visible_and_an_empty_row_is_nothing() {
        let mut r = row("secret");
        for c in &mut r {
            c.attrs = Attrs(Attrs::HIDDEN);
        }
        let d = build(&r, &palette(), &none, None);
        assert!(d.runs.iter().all(|r| r.fg == [10; 3]));
        assert_eq!(
            build(&row("      "), &palette(), &none, None),
            RowDraw::default()
        );
    }
}
