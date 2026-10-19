//! Encode keys from the outer terminal into the bytes the program in a
//! terminal expects, given the keyboard modes it asked for.

use crate::terminal::key_modes::KeyModes;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Kitty "disambiguate escape codes" flag.
const KITTY_DISAMBIGUATE: u8 = 1;

/// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4) + super(8).
fn modifier_param(m: KeyModifiers) -> u8 {
    let bits = [
        (KeyModifiers::SHIFT, 1),
        (KeyModifiers::ALT, 2),
        (KeyModifiers::CONTROL, 4),
        (KeyModifiers::SUPER, 8),
    ];
    1 + bits
        .iter()
        .filter(|(k, _)| m.contains(*k))
        .map(|(_, b)| b)
        .sum::<u8>()
}

fn csi_u(code: u32, mods: u8) -> Vec<u8> {
    if mods == 1 {
        format!("\x1b[{code}u").into_bytes()
    } else {
        format!("\x1b[{code};{mods}u").into_bytes()
    }
}

fn modify_other_keys(code: u32, mods: u8) -> Vec<u8> {
    format!("\x1b[27;{mods};{code}~").into_bytes()
}

fn cursor_key(letter: char, mods: u8, app_cursor: bool) -> Vec<u8> {
    match (mods, app_cursor) {
        (1, true) => format!("\x1bO{letter}"),
        (1, false) => format!("\x1b[{letter}"),
        _ => format!("\x1b[1;{mods}{letter}"),
    }
    .into_bytes()
}

fn tilde_key(n: u8, mods: u8) -> Vec<u8> {
    if mods == 1 {
        format!("\x1b[{n}~").into_bytes()
    } else {
        format!("\x1b[{n};{mods}~").into_bytes()
    }
}

/// Enter, Tab and Backspace: legacy bytes cannot carry modifiers, so use the
/// richest encoding the program asked for.
fn modified_special(code: u32, legacy: &[u8], m: KeyModifiers, km: KeyModes) -> Vec<u8> {
    let mods = modifier_param(m);
    if mods == 1 {
        legacy.to_vec()
    } else if km.kitty_flags & KITTY_DISAMBIGUATE != 0 {
        csi_u(code, mods)
    } else if km.modify_other_keys >= 1 {
        modify_other_keys(code, mods)
    } else if m == KeyModifiers::ALT {
        [b"\x1b", legacy].concat()
    } else {
        legacy.to_vec()
    }
}

/// Bytes for a key press, or None for keys that are not sent (releases, unknown keys).
pub fn encode_key(ev: &KeyEvent, km: KeyModes) -> Option<Vec<u8>> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let m = ev.modifiers;
    let mods = modifier_param(m);
    let kitty = km.kitty_flags & KITTY_DISAMBIGUATE != 0;
    let bytes = match ev.code {
        KeyCode::Enter => modified_special(13, b"\r", m, km),
        KeyCode::Tab => modified_special(9, b"\t", m, km),
        KeyCode::Backspace => modified_special(127, b"\x7f", m, km),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc if kitty => csi_u(27, mods),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up => cursor_key('A', mods, km.app_cursor),
        KeyCode::Down => cursor_key('B', mods, km.app_cursor),
        KeyCode::Right => cursor_key('C', mods, km.app_cursor),
        KeyCode::Left => cursor_key('D', mods, km.app_cursor),
        KeyCode::Home => cursor_key('H', mods, km.app_cursor),
        KeyCode::End => cursor_key('F', mods, km.app_cursor),
        KeyCode::Insert => tilde_key(2, mods),
        KeyCode::Delete => tilde_key(3, mods),
        KeyCode::PageUp => tilde_key(5, mods),
        KeyCode::PageDown => tilde_key(6, mods),
        KeyCode::F(n @ 1..=4) => {
            let letter = ['P', 'Q', 'R', 'S'][usize::from(n - 1)];
            if mods == 1 {
                format!("\x1bO{letter}").into_bytes()
            } else {
                format!("\x1b[1;{mods}{letter}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => {
            tilde_key([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)], mods)
        }
        KeyCode::Char(c) => encode_char(c, m, km)?,
        _ => return None,
    };
    Some(bytes)
}

/// The C0 byte for Ctrl plus a character, as terminals send it.
fn control_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => (c.to_ascii_lowercase() as u8) & 0x1f,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

fn encode_char(c: char, m: KeyModifiers, km: KeyModes) -> Option<Vec<u8>> {
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    if (ctrl || alt) && km.kitty_flags & KITTY_DISAMBIGUATE != 0 {
        return Some(csi_u(c.to_ascii_lowercase() as u32, modifier_param(m)));
    }
    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    if ctrl {
        match control_byte(c) {
            Some(b) => out.push(b),
            None if km.modify_other_keys >= 2 => {
                return Some(modify_other_keys(
                    c as u32,
                    modifier_param(m - KeyModifiers::SHIFT),
                ))
            }
            None => return None,
        }
    } else {
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: m,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn enc(code: KeyCode, m: KeyModifiers, km: KeyModes) -> Vec<u8> {
        encode_key(&key(code, m), km).unwrap()
    }

    const LEGACY: KeyModes = KeyModes {
        app_cursor: false,
        kitty_flags: 0,
        modify_other_keys: 0,
    };
    const KITTY: KeyModes = KeyModes {
        app_cursor: false,
        kitty_flags: 5,
        modify_other_keys: 2,
    };
    const MOK: KeyModes = KeyModes {
        app_cursor: false,
        kitty_flags: 0,
        modify_other_keys: 2,
    };

    #[test]
    fn shift_enter_uses_the_richest_requested_encoding() {
        assert_eq!(
            enc(KeyCode::Enter, KeyModifiers::SHIFT, KITTY),
            b"\x1b[13;2u"
        );
        assert_eq!(
            enc(KeyCode::Enter, KeyModifiers::SHIFT, MOK),
            b"\x1b[27;2;13~"
        );
        assert_eq!(enc(KeyCode::Enter, KeyModifiers::SHIFT, LEGACY), b"\r");
        assert_eq!(enc(KeyCode::Enter, KeyModifiers::ALT, LEGACY), b"\x1b\r");
        assert_eq!(enc(KeyCode::Enter, KeyModifiers::NONE, KITTY), b"\r");
    }

    #[test]
    fn esc_and_control_keys() {
        assert_eq!(enc(KeyCode::Esc, KeyModifiers::NONE, KITTY), b"\x1b[27u");
        assert_eq!(enc(KeyCode::Esc, KeyModifiers::NONE, LEGACY), b"\x1b");
        assert_eq!(
            enc(KeyCode::Char('c'), KeyModifiers::CONTROL, LEGACY),
            b"\x03"
        );
        assert_eq!(
            enc(KeyCode::Char('c'), KeyModifiers::CONTROL, KITTY),
            b"\x1b[99;5u"
        );
        assert_eq!(enc(KeyCode::Char('x'), KeyModifiers::ALT, LEGACY), b"\x1bx");
        assert_eq!(
            enc(KeyCode::Char(','), KeyModifiers::CONTROL, MOK),
            b"\x1b[27;5;44~"
        );
        assert!(encode_key(&key(KeyCode::Char(','), KeyModifiers::CONTROL), LEGACY).is_none());
    }

    #[test]
    fn cursor_and_function_keys() {
        let app = KeyModes {
            app_cursor: true,
            ..LEGACY
        };
        assert_eq!(enc(KeyCode::Up, KeyModifiers::NONE, app), b"\x1bOA");
        assert_eq!(enc(KeyCode::Up, KeyModifiers::NONE, LEGACY), b"\x1b[A");
        assert_eq!(enc(KeyCode::Left, KeyModifiers::CONTROL, app), b"\x1b[1;5D");
        assert_eq!(enc(KeyCode::Delete, KeyModifiers::NONE, LEGACY), b"\x1b[3~");
        assert_eq!(
            enc(KeyCode::PageUp, KeyModifiers::SHIFT, LEGACY),
            b"\x1b[5;2~"
        );
        assert_eq!(enc(KeyCode::F(1), KeyModifiers::NONE, LEGACY), b"\x1bOP");
        assert_eq!(enc(KeyCode::F(5), KeyModifiers::NONE, LEGACY), b"\x1b[15~");
        assert_eq!(
            enc(KeyCode::F(12), KeyModifiers::SHIFT, LEGACY),
            b"\x1b[24;2~"
        );
        assert_eq!(
            enc(KeyCode::BackTab, KeyModifiers::SHIFT, LEGACY),
            b"\x1b[Z"
        );
        assert_eq!(enc(KeyCode::Backspace, KeyModifiers::NONE, LEGACY), b"\x7f");
    }

    #[test]
    fn text_and_releases() {
        assert_eq!(
            enc(KeyCode::Char('漢'), KeyModifiers::NONE, KITTY),
            "漢".as_bytes()
        );
        assert_eq!(enc(KeyCode::Char('A'), KeyModifiers::SHIFT, KITTY), b"A");
        let mut release = key(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(encode_key(&release, LEGACY).is_none());
        assert!(encode_key(&key(KeyCode::CapsLock, KeyModifiers::NONE), LEGACY).is_none());
    }
}
