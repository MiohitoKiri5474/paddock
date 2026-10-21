//! Encode outer-terminal input (crossterm events) into the bytes the pane's
//! program expects, given the modes it asked for. The bytes are then sent
//! verbatim with `send-keys -H`, which bypasses tmux's own key encoding.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyModes {
    pub app_cursor: bool,
    pub kitty_flags: u8,
    pub modify_other_keys: u8,
}

const KITTY_DISAMBIGUATE: u8 = 1;

fn mod_param(m: KeyModifiers) -> u8 {
    let mut p = 0;
    if m.contains(KeyModifiers::SHIFT) {
        p |= 1;
    }
    if m.contains(KeyModifiers::ALT) {
        p |= 2;
    }
    if m.contains(KeyModifiers::CONTROL) {
        p |= 4;
    }
    if m.contains(KeyModifiers::SUPER) {
        p |= 8;
    }
    p + 1
}

/// CSI-u (kitty) form: `CSI code ; mods u` (mods omitted when 1).
fn csi_u(code: u32, mods: u8) -> Vec<u8> {
    if mods == 1 {
        format!("\x1b[{code}u").into_bytes()
    } else {
        format!("\x1b[{code};{mods}u").into_bytes()
    }
}

/// xterm modifyOtherKeys form: `CSI 27 ; mods ; code ~`.
fn mok(code: u32, mods: u8) -> Vec<u8> {
    format!("\x1b[27;{mods};{code}~").into_bytes()
}

fn cursor_key(letter: char, mods: u8, app: bool) -> Vec<u8> {
    if mods > 1 {
        format!("\x1b[1;{mods}{letter}").into_bytes()
    } else if app {
        format!("\x1bO{letter}").into_bytes()
    } else {
        format!("\x1b[{letter}").into_bytes()
    }
}

fn tilde_key(n: u8, mods: u8) -> Vec<u8> {
    if mods > 1 {
        format!("\x1b[{n};{mods}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

/// Encode a key for the pane, or None if it should not be sent (key release etc.).
pub fn encode_key(ev: &KeyEvent, km: KeyModes) -> Option<Vec<u8>> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let m = ev.modifiers;
    let mods = mod_param(m);
    let kitty = km.kitty_flags & KITTY_DISAMBIGUATE != 0;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    let shift = m.contains(KeyModifiers::SHIFT);

    // Keys whose legacy encoding cannot carry the modifier.
    let special = |code: u32, legacy: &[u8]| -> Vec<u8> {
        if mods == 1 {
            legacy.to_vec()
        } else if kitty {
            csi_u(code, mods)
        } else if km.modify_other_keys >= 1 {
            mok(code, mods)
        } else if alt && !ctrl && !shift {
            [b"\x1b".as_slice(), legacy].concat()
        } else {
            legacy.to_vec()
        }
    };

    let bytes = match ev.code {
        KeyCode::Enter => special(13, b"\r"),
        KeyCode::Tab => special(9, b"\t"),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => special(127, b"\x7f"),
        KeyCode::Esc => {
            if kitty {
                csi_u(27, mods)
            } else {
                b"\x1b".to_vec()
            }
        }
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
            let l = [b'P', b'Q', b'R', b'S'][n as usize - 1] as char;
            if mods > 1 {
                format!("\x1b[1;{mods}{l}").into_bytes()
            } else {
                format!("\x1bO{l}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => {
            let t = [15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5];
            tilde_key(t, mods)
        }
        KeyCode::Char(c) => encode_char(c, m, km)?,
        _ => return None,
    };
    Some(bytes)
}

fn encode_char(c: char, m: KeyModifiers, km: KeyModes) -> Option<Vec<u8>> {
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    let kitty = km.kitty_flags & KITTY_DISAMBIGUATE != 0;
    // Shift alone is already in the character itself.
    let mods_no_shift = mod_param(m - KeyModifiers::SHIFT);
    if (ctrl || alt) && kitty {
        let base = c.to_ascii_lowercase() as u32;
        return Some(csi_u(base, mod_param(m)));
    }
    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    if ctrl {
        let b = match c {
            'a'..='z' | 'A'..='Z' => (c.to_ascii_lowercase() as u8) & 0x1f,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '/' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ if km.modify_other_keys >= 2 => return Some(mok(c as u32, mods_no_shift)),
            _ => return None,
        };
        out.push(b);
    } else {
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    Some(out)
}

/// Wrap pasted text for the pane: newlines become CR (as a terminal sends them),
/// and the text is bracketed if the program enabled mode 2004.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    // Never let the pasted text close the bracket early.
    let body = body.replace("\x1b[201~", "");
    if bracketed {
        [b"\x1b[200~".as_slice(), body.as_bytes(), b"\x1b[201~"].concat()
    } else {
        body.into_bytes()
    }
}

/// SGR (1006) mouse report. `col`/`row` are 0-based within the pane.
pub fn encode_mouse_sgr(kind: MouseEventKind, col: u16, row: u16, m: KeyModifiers) -> Option<Vec<u8>> {
    use crossterm::event::MouseButton as B;
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
    let btn = |b: B| match b {
        B::Left => 0,
        B::Middle => 1,
        B::Right => 2,
    };
    let (code, press) = match kind {
        MouseEventKind::Down(b) => (btn(b), true),
        MouseEventKind::Up(b) => (btn(b), false),
        MouseEventKind::Drag(b) => (btn(b) + 32, true),
        MouseEventKind::Moved => (35, true),
        MouseEventKind::ScrollUp => (64, true),
        MouseEventKind::ScrollDown => (65, true),
        MouseEventKind::ScrollLeft => (66, true),
        MouseEventKind::ScrollRight => (67, true),
    };
    let fin = if press { 'M' } else { 'm' };
    Some(format!("\x1b[<{};{};{}{}", code + mods, col + 1, row + 1, fin).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    #[test]
    fn shift_enter_per_mode() {
        let ev = k(KeyCode::Enter, KeyModifiers::SHIFT);
        let kitty = KeyModes { kitty_flags: 5, ..Default::default() };
        assert_eq!(encode_key(&ev, kitty).unwrap(), b"\x1b[13;2u");
        let mok2 = KeyModes { modify_other_keys: 2, ..Default::default() };
        assert_eq!(encode_key(&ev, mok2).unwrap(), b"\x1b[27;2;13~");
        assert_eq!(encode_key(&ev, KeyModes::default()).unwrap(), b"\r");
    }

    #[test]
    fn esc_and_ctrl_c() {
        let kitty = KeyModes { kitty_flags: 1, ..Default::default() };
        assert_eq!(encode_key(&k(KeyCode::Esc, KeyModifiers::NONE), kitty).unwrap(), b"\x1b[27u");
        assert_eq!(encode_key(&k(KeyCode::Esc, KeyModifiers::NONE), KeyModes::default()).unwrap(), b"\x1b");
        assert_eq!(encode_key(&k(KeyCode::Char('c'), KeyModifiers::CONTROL), KeyModes::default()).unwrap(), b"\x03");
        assert_eq!(encode_key(&k(KeyCode::Char('c'), KeyModifiers::CONTROL), kitty).unwrap(), b"\x1b[99;5u");
    }

    #[test]
    fn arrows_and_text() {
        let app = KeyModes { app_cursor: true, ..Default::default() };
        assert_eq!(encode_key(&k(KeyCode::Up, KeyModifiers::NONE), app).unwrap(), b"\x1bOA");
        assert_eq!(encode_key(&k(KeyCode::Up, KeyModifiers::SHIFT), app).unwrap(), b"\x1b[1;2A");
        assert_eq!(encode_key(&k(KeyCode::Char('漢'), KeyModifiers::NONE), app).unwrap(), "漢".as_bytes());
    }

    #[test]
    fn paste_and_mouse() {
        assert_eq!(encode_paste("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(
            encode_mouse_sgr(MouseEventKind::ScrollUp, 4, 9, KeyModifiers::NONE).unwrap(),
            b"\x1b[<64;5;10M"
        );
    }
}
