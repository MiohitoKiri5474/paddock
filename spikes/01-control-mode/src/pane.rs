//! A pane as paddock sees it: emulator + key-mode tracking, seeded from tmux
//! when (re)attaching.

use crate::emu::{capture_to_stream, Emulator, KeyModeScanner};
use crate::keys::KeyModes;
use crate::tmux::{Control, Event};
use anyhow::{bail, Result};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub struct Pane {
    pub id: String,
    pub emu: Box<dyn Emulator>,
    pub scanner: KeyModeScanner,
}

/// Formats queried on attach to rebuild modes tmux knows about.
const STATE_FMT: &str = "#{alternate_on} #{cursor_x} #{cursor_y} #{cursor_flag} #{keypad_cursor_flag} \
#{bracket_paste_flag} #{mouse_standard_flag} #{mouse_button_flag} #{mouse_any_flag} #{mouse_sgr_flag} \
#{scroll_region_upper} #{scroll_region_lower} #{pane_key_mode}";

impl Pane {
    pub fn new(id: &str, emu: Box<dyn Emulator>) -> Self {
        Self { id: id.to_string(), emu, scanner: KeyModeScanner::default() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.scanner.feed(bytes);
        self.emu.feed(bytes);
    }

    pub fn key_modes(&self) -> KeyModes {
        KeyModes {
            app_cursor: self.emu.modes().app_cursor,
            kitty_flags: self.scanner.kitty_flags(),
            modify_other_keys: self.scanner.modify_other_keys,
        }
    }

    /// Rebuild the pane from tmux: history, (saved) normal screen, alternate
    /// screen, cursor and modes.
    pub fn apply_seed(&mut self, seed: &Seed) -> Result<()> {
        let st = String::from_utf8_lossy(seed.state.first().map(Vec::as_slice).unwrap_or(b"")).to_string();
        let f: Vec<&str> = st.split_whitespace().collect();
        if f.len() < 13 {
            bail!("unexpected state line: {st:?}");
        }
        let flag = |i: usize| f[i] == "1";
        let alt = flag(0);
        let join = |ls: &[Vec<u8>]| -> Vec<u8> {
            let mut v = ls.join(&b'\n');
            v.push(b'\n');
            v
        };
        let (_, rows) = self.emu.size();
        let mut s = Vec::new();
        if !seed.history.is_empty() {
            s.extend(capture_to_stream(&join(&seed.history)));
            s.extend_from_slice(b"\r\n");
        }
        if alt {
            // Normal screen first, then switch and draw the alternate one.
            s.extend(capture_to_stream(&join(&seed.saved)));
            s.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J");
        }
        s.extend(capture_to_stream(&join(&seed.visible)));
        let (top, bot) = (f[10].parse::<u16>().unwrap_or(0), f[11].parse::<u16>().unwrap_or(rows - 1));
        if top != 0 || bot != rows.saturating_sub(1) {
            s.extend(format!("\x1b[{};{}r", top + 1, bot + 1).into_bytes());
        }
        let (cx, cy) = (f[1].parse::<u16>().unwrap_or(0), f[2].parse::<u16>().unwrap_or(0));
        s.extend(format!("\x1b[{};{}H", cy + 1, cx + 1).into_bytes());
        for (i, mode) in [(4, "1"), (5, "2004"), (6, "1000"), (7, "1002"), (8, "1003"), (9, "1006")] {
            if flag(i) {
                s.extend(format!("\x1b[?{mode}h").into_bytes());
            }
        }
        if !flag(3) {
            s.extend_from_slice(b"\x1b[?25l");
        }
        // tmux knows modifyOtherKeys ("Ext 1"/"Ext 2") but not kitty flags.
        let key_mode = f[12..].join(" ");
        if let Some(n) = key_mode.strip_prefix("Ext ") {
            s.extend(format!("\x1b[>4;{n}m").into_bytes());
        }
        self.feed(&s);
        Ok(())
    }
}

/// Everything needed to rebuild a pane, fetched in one atomic batch.
pub struct Seed {
    state: Vec<Vec<u8>>,
    history: Vec<Vec<u8>>,
    saved: Vec<Vec<u8>>,
    visible: Vec<Vec<u8>>,
}

/// Send all queries on ONE control-mode line: tmux runs them back to back with
/// no pane reads in between, and `%output` is queued in order with the replies.
/// So output seen before the last reply is already in the captures (dropped by
/// `wait_reply`), and output after it is new and must be fed normally.
pub fn fetch_seed<T>(ctl: &mut Control, rx: &Receiver<T>, unwrap: fn(T) -> Option<Event>, id: &str) -> Result<Seed> {
    ctl.send(&format!(
        "display -p -t {id} '{STATE_FMT}' ; capture-pane -p -e -S - -E -1 -t {id} ; \
         capture-pane -p -e -a -q -t {id} ; capture-pane -p -e -t {id}"
    ))?;
    Ok(Seed {
        state: wait_reply(rx, unwrap, id)?,
        history: wait_reply(rx, unwrap, id)?,
        saved: wait_reply(rx, unwrap, id)?,
        visible: wait_reply(rx, unwrap, id)?,
    })
}

pub fn wait_reply<T>(rx: &Receiver<T>, unwrap: fn(T) -> Option<Event>, _pane: &str) -> Result<Vec<Vec<u8>>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let t = rx.recv_timeout(left)?;
        match unwrap(t) {
            Some(Event::Reply { ok: true, lines }) => return Ok(lines),
            Some(Event::Reply { ok: false, lines }) => {
                bail!("tmux error: {}", String::from_utf8_lossy(&lines.concat()))
            }
            Some(Event::Exit) => bail!("control client exited"),
            _ => {}
        }
    }
}
