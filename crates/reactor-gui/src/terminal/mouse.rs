//! The mouse as a program sees it: reports in the encoding it asked for, and what the wheel does
//! when it did not ask for any.
//!
//! Pure. Coordinates are 0-based cells here and 1-based on the wire.

use super::types::{Modes, MouseEncoding, MouseMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// Motion with no button down.
    None,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Whether a click or drag goes to the program. Shift is the way to select in a program that has
/// taken the mouse.
pub fn reporting(modes: Modes, shift: bool) -> bool {
    modes.mouse != MouseMode::Off && !shift
}

fn button_code(button: Button) -> u8 {
    match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
        Button::WheelLeft => 66,
        Button::WheelRight => 67,
    }
}

fn is_wheel(button: Button) -> bool {
    matches!(
        button,
        Button::WheelUp | Button::WheelDown | Button::WheelLeft | Button::WheelRight
    )
}

/// The report for a mouse event, or `None` when the program did not ask for this kind of event
/// or the position cannot be written in the encoding it chose.
pub fn report(
    modes: Modes,
    button: Button,
    action: Action,
    mods: Mods,
    col: usize,
    row: usize,
) -> Option<Vec<u8>> {
    match (modes.mouse, action) {
        (MouseMode::Off, _) => return None,
        (MouseMode::Click, Action::Motion) => return None,
        (MouseMode::Drag, Action::Motion) if button == Button::None => return None,
        _ => {}
    }
    // A wheel turn is a press and nothing more.
    if is_wheel(button) && action != Action::Press {
        return None;
    }

    let mut code = button_code(button);
    if action == Action::Motion {
        code += 32;
    }
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    let (x, y) = (col + 1, row + 1);

    match modes.mouse_encoding {
        MouseEncoding::Sgr => {
            let last = if action == Action::Release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{last}").into_bytes())
        }
        encoding => {
            // The old encodings have no release of a particular button.
            if action == Action::Release {
                code = (code & !0b11) | 3;
            }
            let mut out = vec![0x1b, b'[', b'M', code + 32];
            for v in [x, y] {
                let v = v + 32;
                if encoding == MouseEncoding::Utf8 {
                    let c = char::from_u32(u32::try_from(v).ok()?).filter(|_| v <= 2015)?;
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                } else {
                    // One byte: columns and rows stop at 223.
                    out.push(u8::try_from(v).ok()?);
                }
            }
            Some(out)
        }
    }
}

/// What a turn of the wheel does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    /// It is reported to the program as buttons 64 to 67.
    Report,
    /// The program is on the alternate screen and wants the wheel as arrow keys.
    Arrows,
    /// It scrolls the history.
    Scrollback,
}

pub fn wheel(modes: Modes, shift: bool) -> Wheel {
    if shift {
        Wheel::Scrollback
    } else if modes.mouse != MouseMode::Off {
        Wheel::Report
    } else if modes.alt_screen && modes.alternate_scroll {
        Wheel::Arrows
    } else {
        Wheel::Scrollback
    }
}

/// `lines` arrow presses, up when positive.
pub fn wheel_arrows(modes: Modes, lines: i32) -> Vec<u8> {
    let final_byte = if lines > 0 { b'A' } else { b'B' };
    let one: [u8; 3] = if modes.app_cursor {
        [0x1b, b'O', final_byte]
    } else {
        [0x1b, b'[', final_byte]
    };
    let mut out = Vec::with_capacity(3 * lines.unsigned_abs() as usize);
    for _ in 0..lines.unsigned_abs() {
        out.extend_from_slice(&one);
    }
    out
}

/// Add a scroll to what is left over of the last one, and take the whole lines out of it. A
/// trackpad sends fractions of a line; they are kept until they add up.
pub fn take_lines(carry: &mut f32, delta: f32) -> i32 {
    *carry += delta;
    let whole = carry.trunc();
    *carry -= whole;
    whole as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(mouse: MouseMode, encoding: MouseEncoding) -> Modes {
        Modes {
            mouse,
            mouse_encoding: encoding,
            ..Modes::default()
        }
    }

    fn rep(m: Modes, b: Button, a: Action, mods: Mods, col: usize, row: usize) -> Option<Vec<u8>> {
        report(m, b, a, mods, col, row)
    }

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    #[test]
    fn sgr_reports_one_based_cells_and_the_release_by_its_final_byte() {
        let m = modes(MouseMode::Click, MouseEncoding::Sgr);
        assert_eq!(
            rep(m, Button::Left, Action::Press, NONE, 0, 0),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        assert_eq!(
            rep(m, Button::Left, Action::Release, NONE, 0, 0),
            Some(b"\x1b[<0;1;1m".to_vec())
        );
        assert_eq!(
            rep(m, Button::Right, Action::Press, NONE, 9, 4),
            Some(b"\x1b[<2;10;5M".to_vec())
        );
        assert_eq!(
            rep(m, Button::Middle, Action::Press, NONE, 299, 99),
            Some(b"\x1b[<1;300;100M".to_vec())
        );
    }

    #[test]
    fn modifiers_add_4_8_and_16_and_motion_adds_32() {
        let m = modes(MouseMode::Motion, MouseEncoding::Sgr);
        let shift = Mods {
            shift: true,
            ..NONE
        };
        let alt = Mods { alt: true, ..NONE };
        let ctrl = Mods { ctrl: true, ..NONE };
        let all = Mods {
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(
            rep(m, Button::Left, Action::Press, shift, 0, 0),
            Some(b"\x1b[<4;1;1M".to_vec())
        );
        assert_eq!(
            rep(m, Button::Left, Action::Press, alt, 0, 0),
            Some(b"\x1b[<8;1;1M".to_vec())
        );
        assert_eq!(
            rep(m, Button::Left, Action::Press, ctrl, 0, 0),
            Some(b"\x1b[<16;1;1M".to_vec())
        );
        assert_eq!(
            rep(m, Button::Left, Action::Press, all, 0, 0),
            Some(b"\x1b[<28;1;1M".to_vec())
        );
        assert_eq!(
            rep(m, Button::Left, Action::Motion, NONE, 2, 3),
            Some(b"\x1b[<32;3;4M".to_vec())
        );
        assert_eq!(
            rep(m, Button::None, Action::Motion, NONE, 2, 3),
            Some(b"\x1b[<35;3;4M".to_vec())
        );
    }

    #[test]
    fn the_wheel_is_buttons_64_to_67_and_a_press_only() {
        let m = modes(MouseMode::Click, MouseEncoding::Sgr);
        for (b, n) in [
            (Button::WheelUp, 64),
            (Button::WheelDown, 65),
            (Button::WheelLeft, 66),
            (Button::WheelRight, 67),
        ] {
            assert_eq!(
                rep(m, b, Action::Press, NONE, 1, 1),
                Some(format!("\x1b[<{n};2;2M").into_bytes())
            );
            assert_eq!(rep(m, b, Action::Release, NONE, 1, 1), None);
        }
    }

    #[test]
    fn the_normal_encoding_adds_32_and_stops_at_223() {
        let m = modes(MouseMode::Click, MouseEncoding::Normal);
        assert_eq!(
            rep(m, Button::Left, Action::Press, NONE, 0, 0),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            rep(m, Button::Right, Action::Press, NONE, 4, 9),
            Some(vec![0x1b, b'[', b'M', 34, 37, 42])
        );
        // Release has no button of its own: 3.
        assert_eq!(
            rep(m, Button::Left, Action::Release, NONE, 0, 0),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33])
        );
        assert_eq!(
            rep(m, Button::Left, Action::Press, NONE, 222, 0),
            Some(vec![0x1b, b'[', b'M', 32, 255, 33])
        );
        assert_eq!(rep(m, Button::Left, Action::Press, NONE, 223, 0), None);
        assert_eq!(rep(m, Button::Left, Action::Press, NONE, 0, 223), None);
    }

    #[test]
    fn the_utf8_encoding_writes_the_coordinates_as_code_points() {
        let m = modes(MouseMode::Click, MouseEncoding::Utf8);
        assert_eq!(
            rep(m, Button::Left, Action::Press, NONE, 0, 0),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        // Column 300 is code point 332 (300 + 32), two bytes.
        let got = rep(m, Button::Left, Action::Press, NONE, 299, 0).unwrap();
        assert_eq!(&got[..4], &[0x1b, b'[', b'M', 32]);
        assert_eq!(std::str::from_utf8(&got[4..6]).unwrap(), "\u{14c}");
        assert_eq!(rep(m, Button::Left, Action::Press, NONE, 3000, 0), None);
    }

    #[test]
    fn what_a_program_asked_for_is_what_it_gets() {
        let click = modes(MouseMode::Click, MouseEncoding::Sgr);
        let drag = modes(MouseMode::Drag, MouseEncoding::Sgr);
        let motion = modes(MouseMode::Motion, MouseEncoding::Sgr);
        let off = modes(MouseMode::Off, MouseEncoding::Sgr);
        assert_eq!(rep(off, Button::Left, Action::Press, NONE, 0, 0), None);
        assert_eq!(rep(click, Button::Left, Action::Motion, NONE, 0, 0), None);
        assert!(rep(drag, Button::Left, Action::Motion, NONE, 0, 0).is_some());
        assert_eq!(rep(drag, Button::None, Action::Motion, NONE, 0, 0), None);
        assert!(rep(motion, Button::None, Action::Motion, NONE, 0, 0).is_some());
    }

    #[test]
    fn shift_takes_the_mouse_back() {
        let m = modes(MouseMode::Click, MouseEncoding::Sgr);
        assert!(reporting(m, false));
        assert!(!reporting(m, true));
        assert!(!reporting(Modes::default(), false));
    }

    #[test]
    fn the_wheel_scrolls_reports_or_sends_arrows() {
        let plain = Modes::default();
        assert_eq!(wheel(plain, false), Wheel::Scrollback);
        let alt = Modes {
            alt_screen: true,
            alternate_scroll: true,
            ..plain
        };
        assert_eq!(wheel(alt, false), Wheel::Arrows);
        let alt_without = Modes {
            alt_screen: true,
            alternate_scroll: false,
            ..plain
        };
        assert_eq!(wheel(alt_without, false), Wheel::Scrollback);
        // Arrows only on the alternate screen: the primary screen has history to scroll.
        let primary = Modes {
            alternate_scroll: true,
            ..plain
        };
        assert_eq!(wheel(primary, false), Wheel::Scrollback);
        let mouse = modes(MouseMode::Click, MouseEncoding::Sgr);
        assert_eq!(wheel(mouse, false), Wheel::Report);
        assert_eq!(wheel(mouse, true), Wheel::Scrollback);
        assert_eq!(wheel(alt, true), Wheel::Scrollback);
    }

    #[test]
    fn arrows_for_the_wheel_follow_the_cursor_key_mode() {
        let m = Modes::default();
        assert_eq!(wheel_arrows(m, 2), b"\x1b[A\x1b[A");
        assert_eq!(wheel_arrows(m, -1), b"\x1b[B");
        let app = Modes {
            app_cursor: true,
            ..m
        };
        assert_eq!(wheel_arrows(app, 1), b"\x1bOA");
        assert_eq!(wheel_arrows(app, 0), b"");
    }

    #[test]
    fn fractions_of_a_line_add_up() {
        let mut carry = 0.0;
        assert_eq!(take_lines(&mut carry, 0.4), 0);
        assert_eq!(take_lines(&mut carry, 0.4), 0);
        assert_eq!(take_lines(&mut carry, 0.4), 1);
        assert_eq!(take_lines(&mut carry, -2.5), -2);
        assert_eq!(take_lines(&mut carry, -0.8), -1);
    }
}
