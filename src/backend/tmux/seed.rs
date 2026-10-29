//! Rebuilding a terminal's screen and history when paddock (re)attaches.
//!
//! A new control client gets no replay of what was on screen, so paddock asks
//! tmux for it: one `display-message` for cursor and modes, then captures of
//! the history, the saved normal screen and the visible screen. All four go on
//! ONE control line, so tmux runs them back to back and orders `%output` with
//! their replies: output before the last reply is already in the captures,
//! output after it is new (ADR 0005).

use super::control::{wait_reply, ControlClient};
use super::protocol::Message;
use anyhow::{bail, Result};
use std::sync::mpsc::Receiver;

const STATE_FORMAT: &str = "#{alternate_on} #{cursor_x} #{cursor_y} #{cursor_flag} \
#{keypad_cursor_flag} #{bracket_paste_flag} #{mouse_standard_flag} #{mouse_button_flag} \
#{mouse_any_flag} #{mouse_sgr_flag} #{scroll_region_upper} #{scroll_region_lower} #{history_size} \
#{pane_key_mode}";
const STATE_FIELDS: usize = 14;
/// How many times [`fetch`] asks for the panes' states before it settles.
const STATE_ASKS: usize = 5;

/// What tmux holds for a pane, as captured.
#[derive(Debug, Default, Clone)]
pub struct Seed {
    pub state: String,
    pub history: Vec<Vec<u8>>,
    pub saved_screen: Vec<Vec<u8>>,
    pub visible: Vec<Vec<u8>>,
}

/// The commands that capture `pane`, for one control line.
fn commands(pane: &str) -> String {
    format!(
        "display-message -p -t {pane} '{STATE_FORMAT}' ; \
         capture-pane -p -e -S - -E -1 -t {pane} ; \
         capture-pane -p -e -a -q -t {pane} ; \
         capture-pane -p -e -t {pane}"
    )
}

/// Ask tmux for everything needed to rebuild each of `panes`, all on one
/// control line (ADR 0006). Pane output that arrives before the replies is
/// discarded; output after them stays queued on `rx`. Notifications that
/// arrive meanwhile go to `held`.
///
/// tmux gives up on a format that takes more than 100 ms to expand and
/// returns what it has, which a loaded machine reaches now and then: a
/// state cut short is asked again, for all panes at once, so the captures
/// still follow each other with no output read in between.
pub fn fetch(
    control: &mut ControlClient,
    rx: &Receiver<Message>,
    panes: &[&str],
    held: &mut Vec<Message>,
) -> Result<Vec<Seed>> {
    let mut asks = 1;
    loop {
        let seeds = fetch_once(control, rx, panes, held)?;
        if asks == STATE_ASKS || seeds.iter().all(|s| state_fields(&s.state).is_some()) {
            return Ok(seeds);
        }
        asks += 1;
    }
}

fn fetch_once(
    control: &mut ControlClient,
    rx: &Receiver<Message>,
    panes: &[&str],
    held: &mut Vec<Message>,
) -> Result<Vec<Seed>> {
    if panes.is_empty() {
        return Ok(Vec::new());
    }
    let line: Vec<String> = panes.iter().map(|p| commands(p)).collect();
    control.send(&line.join(" ; "))?;
    panes
        .iter()
        .map(|_| {
            let state = wait_reply(rx, held)?;
            Ok(Seed {
                state: state
                    .first()
                    .map(|l| String::from_utf8_lossy(l).into_owned())
                    .unwrap_or_default(),
                history: wait_reply(rx, held)?,
                saved_screen: wait_reply(rx, held)?,
                visible: wait_reply(rx, held)?,
            })
        })
        .collect()
}

/// The fields of a pane's state; `None` if tmux cut it short.
fn state_fields(state: &str) -> Option<Vec<&str>> {
    let f: Vec<&str> = state.split_whitespace().collect();
    (f.len() >= STATE_FIELDS).then_some(f)
}

/// Lines of `capture-pane -e` output as a byte stream that redraws them.
fn lines_to_stream(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(line);
        out.extend_from_slice(b"\x1b[0m");
    }
    out
}

/// The bytes that, fed to a fresh emulator of the pane's size, rebuild it.
pub fn stream(seed: &Seed) -> Result<Vec<u8>> {
    let Some(f) = state_fields(&seed.state) else {
        bail!("unexpected pane state from tmux: {:?}", seed.state);
    };
    let flag = |i: usize| f[i] == "1";
    let num = |i: usize| f[i].parse::<u16>().unwrap_or(0);
    let mut s = Vec::new();
    // With no history tmux still returns a line for `-S - -E -1`; ignore it.
    let has_history = num(12) > 0;
    if has_history && !seed.history.is_empty() {
        s.extend(lines_to_stream(&seed.history));
        s.extend_from_slice(b"\r\n");
    }
    if flag(0) {
        // Normal screen first, then switch to the alternate one and draw it.
        s.extend(lines_to_stream(&seed.saved_screen));
        s.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J");
    }
    s.extend(lines_to_stream(&seed.visible));
    let rows = u16::try_from(seed.visible.len()).unwrap_or(u16::MAX);
    let (top, bottom) = (num(10), num(11));
    if top != 0 || bottom + 1 != rows {
        s.extend(format!("\x1b[{};{}r", top + 1, bottom + 1).into_bytes());
    }
    s.extend(format!("\x1b[{};{}H", num(2) + 1, num(1) + 1).into_bytes());
    for (i, mode) in [
        (4, "1"),
        (5, "2004"),
        (6, "1000"),
        (7, "1002"),
        (8, "1003"),
        (9, "1006"),
    ] {
        if flag(i) {
            s.extend(format!("\x1b[?{mode}h").into_bytes());
        }
    }
    if !flag(3) {
        s.extend_from_slice(b"\x1b[?25l");
    }
    // tmux remembers modifyOtherKeys ("Ext 1" / "Ext 2") but not kitty flags.
    if let Some(level) = f[13..].join(" ").strip_prefix("Ext ") {
        s.extend(format!("\x1b[>4;{level}m").into_bytes());
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::emulator::{Colour, Emulator, MouseMode};
    use crate::terminal::key_modes::KeyModeScanner;

    fn lines(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|l| l.as_bytes().to_vec()).collect()
    }

    fn state(alt: u8, x: u16, y: u16, rest: &str) -> String {
        format!("{alt} {x} {y} {rest}")
    }

    #[test]
    fn rebuilds_history_screen_and_cursor() {
        let seed = Seed {
            state: state(0, 2, 1, "1 0 1 0 0 0 0 0 2 2 VT10x"),
            history: lines(&["old 1", "\x1b[31mold 2"]),
            saved_screen: vec![],
            visible: lines(&["$ ls", "$ ", ""]),
        };
        let mut e = Emulator::new(10, 3, 100);
        e.feed(&stream(&seed).unwrap());
        assert_eq!(e.history_len(), 2);
        assert_eq!((e.row_text(0), e.row_text(1)), ("$ ls".into(), "$".into()));
        assert_eq!(e.cursor(), (1, 2));
        assert!(e.modes().bracketed_paste);
        e.scroll(2);
        assert_eq!(e.row_text(0), "old 1");
        assert_eq!(e.cell(1, 0).fg, Colour::Indexed(1));
    }

    #[test]
    fn rebuilds_alternate_screen_and_mouse_and_key_modes() {
        let seed = Seed {
            state: state(1, 0, 0, "0 1 1 1 1 1 1 0 2 0 Ext 2"),
            history: vec![],
            saved_screen: lines(&["shell", "", ""]),
            visible: lines(&["full", "screen", "app"]),
        };
        let bytes = stream(&seed).unwrap();
        let mut e = Emulator::new(10, 3, 100);
        e.feed(&bytes);
        let m = e.modes();
        assert!(m.alt_screen && m.app_cursor && m.sgr_mouse && !m.show_cursor);
        assert_eq!(m.mouse, MouseMode::Motion);
        assert_eq!(e.row_text(1), "screen");
        let mut k = KeyModeScanner::default();
        k.feed(&bytes);
        assert_eq!(k.modify_other_keys(), 2);
        e.feed(b"\x1b[?1049l");
        assert_eq!(e.row_text(0), "shell");
    }

    #[test]
    fn empty_history_is_not_taken_from_the_capture() {
        // With no history, `capture-pane -S - -E -1` still returns the first visible line.
        let seed = Seed {
            state: state(0, 0, 1, "1 0 0 0 0 0 0 0 2 0 VT10x"),
            history: lines(&["first"]),
            saved_screen: vec![],
            visible: lines(&["first", "second", ""]),
        };
        let mut e = Emulator::new(10, 3, 100);
        e.feed(&stream(&seed).unwrap());
        assert_eq!(e.history_len(), 0);
        assert_eq!(e.row_text(0), "first");
    }

    #[test]
    fn rejects_garbled_state() {
        assert!(stream(&Seed {
            state: "1 2".into(),
            ..Seed::default()
        })
        .is_err());
    }
}
