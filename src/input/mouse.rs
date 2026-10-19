//! What a mouse event over a terminal does: report it to a program that asked
//! for the mouse, or scroll paddock's own scrollback.

use crate::terminal::emulator::{Modes, MouseMode};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// Lines scrolled per wheel notch.
pub const WHEEL_LINES: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseAction {
    /// Send these bytes to the program.
    Report(Vec<u8>),
    /// Scroll paddock's own view by this many lines (positive = into history).
    Scroll(i32),
    Ignore,
}

/// Decide what to do with a mouse event at (`col`, `row`) inside the terminal.
/// Holding Shift always keeps the event in paddock.
pub fn route(ev: &MouseEvent, col: u16, row: u16, modes: Modes) -> MouseAction {
    let wanted = match ev.kind {
        MouseEventKind::Moved => modes.mouse == MouseMode::Motion,
        MouseEventKind::Drag(_) => matches!(modes.mouse, MouseMode::Drag | MouseMode::Motion),
        _ => modes.mouse != MouseMode::Off,
    };
    if wanted && !ev.modifiers.contains(KeyModifiers::SHIFT) {
        return MouseAction::Report(encode(ev.kind, col, row, ev.modifiers, modes.sgr_mouse));
    }
    match ev.kind {
        MouseEventKind::ScrollUp => MouseAction::Scroll(WHEEL_LINES),
        MouseEventKind::ScrollDown => MouseAction::Scroll(-WHEEL_LINES),
        _ => MouseAction::Ignore,
    }
}

fn button_code(b: MouseButton) -> u16 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// SGR (1006) report, or the legacy X10 form for programs that did not ask for SGR.
fn encode(kind: MouseEventKind, col: u16, row: u16, m: KeyModifiers, sgr: bool) -> Vec<u8> {
    let mut mods = 0;
    if m.contains(KeyModifiers::SHIFT) {
        mods |= 4;
    }
    if m.contains(KeyModifiers::ALT) {
        mods |= 8;
    }
    if m.contains(KeyModifiers::CONTROL) {
        mods |= 16;
    }
    let (code, press) = match kind {
        MouseEventKind::Down(b) => (button_code(b), true),
        MouseEventKind::Up(b) => (button_code(b), false),
        MouseEventKind::Drag(b) => (button_code(b) + 32, true),
        MouseEventKind::Moved => (35, true),
        MouseEventKind::ScrollUp => (64, true),
        MouseEventKind::ScrollDown => (65, true),
        MouseEventKind::ScrollLeft => (66, true),
        MouseEventKind::ScrollRight => (67, true),
    };
    if sgr {
        let fin = if press { 'M' } else { 'm' };
        return format!("\x1b[<{};{};{}{fin}", code + mods, col + 1, row + 1).into_bytes();
    }
    // X10: release is button 3; coordinates are offset by 32 and capped.
    let code = if press { code } else { 3 };
    let byte = |v: u16| u8::try_from((32 + v).min(255)).unwrap_or(255);
    vec![
        0x1b,
        b'[',
        b'M',
        byte(code + mods),
        byte(col + 1),
        byte(row + 1),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: MouseEventKind, modifiers: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers,
        }
    }

    fn modes(mouse: MouseMode, sgr: bool) -> Modes {
        Modes {
            mouse,
            sgr_mouse: sgr,
            ..Modes::default()
        }
    }

    #[test]
    fn wheel_scrolls_paddock_when_the_program_has_no_mouse() {
        let m = modes(MouseMode::Off, false);
        assert_eq!(
            route(&ev(MouseEventKind::ScrollUp, KeyModifiers::NONE), 1, 1, m),
            MouseAction::Scroll(3)
        );
        assert_eq!(
            route(&ev(MouseEventKind::ScrollDown, KeyModifiers::NONE), 1, 1, m),
            MouseAction::Scroll(-3)
        );
        assert_eq!(
            route(
                &ev(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE),
                1,
                1,
                m
            ),
            MouseAction::Ignore
        );
    }

    #[test]
    fn wheel_goes_to_programs_that_ask_for_the_mouse_unless_shift() {
        let m = modes(MouseMode::Press, true);
        assert_eq!(
            route(&ev(MouseEventKind::ScrollUp, KeyModifiers::NONE), 4, 9, m),
            MouseAction::Report(b"\x1b[<64;5;10M".to_vec())
        );
        assert_eq!(
            route(&ev(MouseEventKind::ScrollUp, KeyModifiers::SHIFT), 4, 9, m),
            MouseAction::Scroll(3)
        );
    }

    #[test]
    fn motion_and_drag_follow_the_requested_mode() {
        let press = modes(MouseMode::Press, true);
        let drag = ev(MouseEventKind::Drag(MouseButton::Left), KeyModifiers::NONE);
        assert_eq!(route(&drag, 0, 0, press), MouseAction::Ignore);
        assert_eq!(
            route(&drag, 0, 0, modes(MouseMode::Drag, true)),
            MouseAction::Report(b"\x1b[<32;1;1M".to_vec())
        );
        let moved = ev(MouseEventKind::Moved, KeyModifiers::NONE);
        assert_eq!(
            route(&moved, 0, 0, modes(MouseMode::Drag, true)),
            MouseAction::Ignore
        );
        assert_eq!(
            route(&moved, 0, 0, modes(MouseMode::Motion, true)),
            MouseAction::Report(b"\x1b[<35;1;1M".to_vec())
        );
    }

    #[test]
    fn sgr_release_and_legacy_encoding() {
        let up = ev(
            MouseEventKind::Up(MouseButton::Right),
            KeyModifiers::CONTROL,
        );
        assert_eq!(
            route(&up, 0, 0, modes(MouseMode::Press, true)),
            MouseAction::Report(b"\x1b[<18;1;1m".to_vec())
        );
        let down = ev(MouseEventKind::Down(MouseButton::Left), KeyModifiers::NONE);
        assert_eq!(
            route(&down, 2, 3, modes(MouseMode::Press, false)),
            MouseAction::Report(vec![0x1b, b'[', b'M', 32, 35, 36])
        );
    }
}
