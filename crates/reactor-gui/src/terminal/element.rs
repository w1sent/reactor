//! Painting a terminal: a snapshot of the grid drawn with the theme's colours.
//!
//! Nothing here holds the engine's lock. The element lays out, takes a copy of the visible rows in
//! `prepaint`, and paints from it.

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::*;

use super::glyph;
use super::metrics::{Metrics, PAD_Y, SCROLLBAR_W};
use super::mouse;
use super::palette::{Palette, Rgb, mix};
use super::rows::{self, Line, RowDraw};
use super::types::*;
use super::view::TerminalView;

/// The colours of the theme, as a palette. The sixteen use the theme's hues, and its `_light`
/// variants for the bright half; no colour is added.
pub fn palette_from_theme(theme: &Theme, min_contrast: f32) -> Palette {
    let rgb = |c: Hsla| -> Rgb {
        let c: Rgba = c.into();
        [
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        ]
    };
    let fg = rgb(theme.foreground);
    let bg = rgb(theme.background);
    Palette {
        ansi: [
            rgb(theme.secondary),
            rgb(theme.red),
            rgb(theme.green),
            rgb(theme.yellow),
            rgb(theme.blue),
            rgb(theme.magenta),
            rgb(theme.cyan),
            // White is the foreground a little dimmer, so bright white stands out from it.
            mix(fg, bg, 0.12),
            rgb(theme.muted_foreground),
            rgb(theme.red_light),
            rgb(theme.green_light),
            rgb(theme.yellow_light),
            rgb(theme.blue_light),
            rgb(theme.magenta_light),
            rgb(theme.cyan_light),
            fg,
        ],
        fg,
        bg,
        cursor: rgb(theme.caret),
        selection: rgb(theme.selection),
        min_contrast,
    }
}

fn hsla(c: Rgb) -> Hsla {
    rgb(u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2])).into()
}

pub struct TerminalElement {
    view: Entity<TerminalView>,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>) -> Self {
        Self { view }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

pub struct Prepaint {
    hitbox: Hitbox,
    content: Content,
    palette: Palette,
    metrics: Metrics,
    look_family: SharedString,
    font_size: Pixels,
    focused: bool,
    cursor_visible: bool,
    hover: Option<(usize, usize, usize)>,
    link_held: bool,
    marked: Option<String>,
    selection: Hsla,
    found: Hsla,
    link: Hsla,
    thumb: Hsla,
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let (look, focused, blink_on, hover, link_held, marked) = {
            let v = self.view.read(cx);
            (
                v.look.clone(),
                v.focused,
                v.blink_on,
                v.hover.as_ref().map(|(row, l)| (*row, l.start, l.end)),
                v.link_modifier,
                v.marked_text().map(str::to_owned),
            )
        };
        let font = font_of(&look.family, false, false);
        let font_id = window.text_system().resolve_font(&font);
        let cell_w = window
            .text_system()
            .advance(font_id, look.size, 'm')
            .map(|s| s.width)
            .unwrap_or(look.size * 0.6);
        let line_h = (look.size * 1.3).round().max(px(1.));
        let scale = window.scale_factor();
        let snap = |v: Pixels| (v.as_f32() * scale).round() / scale;
        let metrics = Metrics::new(
            snap(bounds.origin.x),
            snap(bounds.origin.y),
            bounds.size.width.as_f32(),
            bounds.size.height.as_f32(),
            cell_w.as_f32(),
            line_h.as_f32(),
        );
        self.view.update(cx, |v, cx| v.layout(metrics, cx));

        let content = self.view.read(cx).backend.snapshot();
        let theme = cx.theme();
        let palette = palette_from_theme(theme, look.min_contrast);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Prepaint {
            hitbox,
            content,
            palette,
            metrics,
            look_family: look.family,
            font_size: look.size,
            focused,
            cursor_visible: blink_on,
            hover,
            link_held,
            marked,
            selection: theme.selection,
            found: theme.yellow.opacity(0.35),
            link: theme.link,
            thumb: theme.scrollbar_thumb,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        pre: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        self.listen(pre, window);

        let modes = pre.content.modes;
        let style = if pre.link_held && pre.hover.is_some() {
            CursorStyle::PointingHand
        } else if modes.mouse != MouseMode::Off {
            CursorStyle::Arrow
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(style, &pre.hitbox);

        let m = pre.metrics;
        let content = &pre.content;
        let changed = |i: usize| content.palette_at(i);
        let cell_w = px(m.cell_w);
        let line_h = px(m.line_h);
        let x_at = |col: usize| px((m.origin_x + col as f32 * m.cell_w).round());
        let y_at = |row: usize| px(m.origin_y + row as f32 * m.line_h);
        let thickness = (pre.font_size.as_f32() / 14.0).round().max(1.0);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // The cursor of a block takes the character under it; the others go on top.
            let cursor = content.cursor;
            let cursor_shown = cursor.in_view
                && cursor.shape != CursorShape::Hidden
                && (pre.cursor_visible || !pre.focused);
            let cursor_col = (cursor_shown && cursor.shape == CursorShape::Block && pre.focused)
                .then_some(cursor.col);

            for row in 0..content.rows {
                let draw: RowDraw = rows::build(
                    content.row(row),
                    &pre.palette,
                    &changed,
                    cursor_col.filter(|_| row == cursor.row),
                );
                let y = y_at(row);

                for &(col, n, color) in &draw.backgrounds {
                    let b =
                        Bounds::from_corners(point(x_at(col), y), point(x_at(col + n), y + line_h));
                    window.paint_quad(fill(b, hsla(color)));
                }
                // Selected cells and found text, as stretches.
                if let Some(sel) = content.selection {
                    paint_stretches(
                        window,
                        content.cols,
                        |c| sel.contains(row as i32, c),
                        pre.selection,
                        &x_at,
                        y,
                        line_h,
                    );
                }
                if !content.matches.is_empty() {
                    paint_stretches(
                        window,
                        content.cols,
                        |c| content.matches.iter().any(|mr| mr.contains(row as i32, c)),
                        pre.found,
                        &x_at,
                        y,
                        line_h,
                    );
                }
                if cursor_col.is_some() && row == cursor.row {
                    let wide = content.cell(row, cursor.col).attrs.has(Attrs::WIDE);
                    let span = if wide { 2 } else { 1 };
                    let b = Bounds::from_corners(
                        point(x_at(cursor.col), y),
                        point(x_at(cursor.col + span), y + line_h),
                    );
                    window.paint_quad(fill(
                        b,
                        hsla(
                            content
                                .palette_at(PALETTE_CURSOR)
                                .unwrap_or(pre.palette.cursor),
                        ),
                    ));
                }

                for piece in &draw.pieces {
                    if let Some(rects) = glyph::rects(piece.c, m.cell_w, m.line_h, thickness) {
                        let ox = (m.origin_x + piece.col as f32 * m.cell_w).round();
                        let color = hsla(piece.fg);
                        for r in rects {
                            let b = Bounds::from_corners(
                                point(
                                    px((ox + r.x0).round()),
                                    px((m.origin_y + row as f32 * m.line_h + r.y0).round()),
                                ),
                                point(
                                    px((ox + r.x1).round()),
                                    px((m.origin_y + row as f32 * m.line_h + r.y1).round()),
                                ),
                            );
                            window.paint_quad(fill(b, color.opacity(r.alpha)));
                        }
                    }
                }

                for run in &draw.runs {
                    let font = font_of(&pre.look_family, run.bold, run.italic);
                    let text_run = TextRun {
                        len: run.text.len(),
                        font,
                        color: hsla(run.fg),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let force = if run.wide { cell_w * 2.0 } else { cell_w };
                    let shaped = window.text_system().shape_line(
                        SharedString::from(run.text.clone()),
                        pre.font_size,
                        &[text_run],
                        Some(force),
                    );
                    let _ = shaped.paint(
                        point(x_at(run.col), y),
                        line_h,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }

                for d in &draw.decorations {
                    paint_decoration(window, d, &x_at, y, line_h, thickness, m.cell_w);
                }
            }

            // A link under the pointer, while the key that follows it is held.
            if let (Some((row, start, end)), true) = (pre.hover, pre.link_held) {
                let y = y_at(row) + line_h - px(thickness + 1.0);
                let b = Bounds::from_corners(
                    point(x_at(start), y),
                    point(x_at(end + 1), y + px(thickness)),
                );
                window.paint_quad(fill(b, pre.link));
            }

            // The cursor that is not a block, and the block of a window that has lost the focus.
            if cursor_shown {
                let color = hsla(
                    content
                        .palette_at(PALETTE_CURSOR)
                        .unwrap_or(pre.palette.cursor),
                );
                let wide = content.cell(cursor.row, cursor.col).attrs.has(Attrs::WIDE);
                let (x0, x1) = (
                    x_at(cursor.col),
                    x_at(cursor.col + if wide { 2 } else { 1 }),
                );
                let (y0, y1) = (y_at(cursor.row), y_at(cursor.row) + line_h);
                let t = px(thickness.max(1.0) + 1.0);
                match cursor.shape {
                    CursorShape::Block if !pre.focused => {
                        window.paint_quad(outline(
                            Bounds::from_corners(point(x0, y0), point(x1, y1)),
                            color,
                            BorderStyle::Solid,
                        ));
                    }
                    CursorShape::Hollow => {
                        window.paint_quad(outline(
                            Bounds::from_corners(point(x0, y0), point(x1, y1)),
                            color,
                            BorderStyle::Solid,
                        ));
                    }
                    CursorShape::Underline => {
                        window.paint_quad(fill(
                            Bounds::from_corners(point(x0, y1 - t), point(x1, y1)),
                            color,
                        ));
                    }
                    CursorShape::Beam => {
                        window.paint_quad(fill(
                            Bounds::from_corners(point(x0, y0), point(x0 + t, y1)),
                            color,
                        ));
                    }
                    CursorShape::Block | CursorShape::Hidden => {}
                }
            }

            // Text an input method is composing, at the cursor.
            if let Some(text) = &pre.marked {
                let run = TextRun {
                    len: text.len(),
                    font: font_of(&pre.look_family, false, false),
                    color: hsla(pre.palette.fg),
                    background_color: Some(hsla(pre.palette.bg)),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(hsla(pre.palette.fg)),
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.clone()),
                    pre.font_size,
                    &[run],
                    Some(cell_w),
                );
                let _ = shaped.paint(
                    point(x_at(cursor.col), y_at(cursor.row)),
                    line_h,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }

            // The scrollbar: only where there is a history to scroll.
            if !modes.alt_screen
                && let Some((top, height)) = m.thumb(content.history, content.display_offset)
            {
                let right = bounds.origin.x + bounds.size.width;
                let x0 = right - px(SCROLLBAR_W - 3.0);
                let thumb = Bounds::from_corners(
                    point(x0, bounds.origin.y + px(top)),
                    point(right - px(2.0), bounds.origin.y + px(top + height)),
                );
                window.paint_quad(quad(
                    thumb,
                    px(3.),
                    pre.thumb,
                    0.,
                    transparent_black(),
                    BorderStyle::default(),
                ));
            }
            let _ = PAD_Y;
        });
    }
}

impl TerminalElement {
    /// The mouse, for the frames this element is painted in.
    fn listen(&self, pre: &Prepaint, window: &mut Window) {
        let hitbox = pre.hitbox.clone();
        let view = self.view.clone();
        window.on_mouse_event({
            let (hitbox, view) = (hitbox.clone(), view.clone());
            move |ev: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    view.update(cx, |v, cx| v.mouse_down(ev, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let (hitbox, view) = (hitbox.clone(), view.clone());
            move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let hovered = hitbox.is_hovered(window);
                    view.update(cx, |v, cx| v.mouse_move(ev, hovered, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble {
                    view.update(cx, |v, cx| v.mouse_up(ev, cx));
                }
            }
        });
        window.on_mouse_event({
            let (hitbox, view) = (hitbox, view);
            move |ev: &ScrollWheelEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                    view.update(cx, |v, cx| v.scroll_wheel(ev, cx));
                }
            }
        });
        let _ = mouse::reporting;
    }
}

fn font_of(family: &SharedString, bold: bool, italic: bool) -> Font {
    Font {
        family: family.clone(),
        // Ligatures turn `->` into one glyph, which is two cells of text in the grid.
        features: FontFeatures(std::sync::Arc::new(vec![
            ("calt".into(), 0),
            ("liga".into(), 0),
        ])),
        fallbacks: None,
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

/// Fill the stretches of a row for which `on` is true, as the fewest quads.
fn paint_stretches(
    window: &mut Window,
    cols: usize,
    on: impl Fn(usize) -> bool,
    color: Hsla,
    x_at: &dyn Fn(usize) -> Pixels,
    y: Pixels,
    line_h: Pixels,
) {
    let mut col = 0;
    while col < cols {
        if !on(col) {
            col += 1;
            continue;
        }
        let start = col;
        while col < cols && on(col) {
            col += 1;
        }
        let b = Bounds::from_corners(point(x_at(start), y), point(x_at(col), y + line_h));
        window.paint_quad(fill(b, color));
    }
}

fn paint_decoration(
    window: &mut Window,
    d: &rows::Decoration,
    x_at: &dyn Fn(usize) -> Pixels,
    y: Pixels,
    line_h: Pixels,
    thickness: f32,
    cell_w: f32,
) {
    let color = hsla(d.color);
    let x0 = x_at(d.col);
    let x1 = x_at(d.col + d.cols);
    let t = px(thickness);
    let underline_y = y + line_h - px(thickness + 2.0);
    let bar = |window: &mut Window, ax: Pixels, bx: Pixels, ay: Pixels| {
        window.paint_quad(fill(
            Bounds::from_corners(point(ax, ay), point(bx, ay + t)),
            color,
        ));
    };
    match d.line {
        Line::Strike => bar(window, x0, x1, y + line_h * 0.5),
        Line::Single => bar(window, x0, x1, underline_y),
        Line::Double => {
            bar(window, x0, x1, underline_y);
            bar(window, x0, x1, underline_y - px(thickness + 1.0));
        }
        Line::Dotted | Line::Dashed | Line::Curl => {
            // Pieces along the line: dots, dashes, or alternating heights for a wave.
            let (on, off) = match d.line {
                Line::Dotted => (thickness, thickness),
                Line::Dashed => (cell_w * 0.4, cell_w * 0.2),
                _ => (cell_w / 4.0, 0.0),
            };
            let mut x = x0.as_f32();
            let end = x1.as_f32();
            let mut up = false;
            while x < end {
                let w = on.min(end - x).max(1.0);
                let ay = if d.line == Line::Curl && up {
                    underline_y - t
                } else {
                    underline_y
                };
                bar(window, px(x), px(x + w), ay);
                x += w + off;
                up = !up;
            }
        }
    }
}
