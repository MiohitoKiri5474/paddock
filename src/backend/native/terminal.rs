//! One terminal as the native server holds it: its shell's PTY, and an
//! emulator fed with everything the shell writes. The emulator answers the
//! program's terminal queries (DA and `CSI ? u` through alacritty,
//! XTVERSION here), and makes the snapshot a client attaches with.

use super::pty::{self, Pty};
use crate::backend::TerminalId;
use crate::terminal::emulator::Emulator;
use crate::terminal::key_modes::KeyModeScanner;
use crate::terminal::SCROLLBACK;
use anyhow::Result;
use std::fs::File;
use std::sync::mpsc::SyncSender;

/// How the server names itself to XTVERSION.
const NAME: &str = concat!("paddock ", env!("CARGO_PKG_VERSION"));

pub struct HostedTerminal {
    pub id: TerminalId,
    /// The pid of its shell.
    pub pid: u32,
    pub tty: String,
    /// The PTY's master side, for its size and foreground.
    pub master: File,
    /// Bytes for the program, in order: input and query replies. A thread
    /// of its own writes them, so the state lock never waits on a program
    /// that does not read; the queue is bounded, so input waits instead
    /// (its client's connection does).
    pub input: SyncSender<Vec<u8>>,
    pub size: (u16, u16),
    emulator: Emulator,
    keys: KeyModeScanner,
    scanner: Scanner,
}

impl HostedTerminal {
    pub fn new(
        id: TerminalId,
        pty: &Pty,
        input: SyncSender<Vec<u8>>,
        size: (u16, u16),
    ) -> Result<Self> {
        Ok(Self {
            id,
            pid: pty.child.id(),
            tty: pty.tty.clone(),
            master: pty.master.try_clone()?,
            input,
            size,
            emulator: Emulator::answering(size.0, size.1, SCROLLBACK),
            keys: KeyModeScanner::default(),
            scanner: Scanner::default(),
        })
    }

    /// Feed what the program wrote, and send it the answers to the
    /// queries in it.
    pub fn output(&mut self, bytes: &[u8]) {
        self.keys.feed(bytes);
        self.emulator.feed(bytes);
        let mut replies = self.emulator.take_replies();
        for _ in 0..self.scanner.feed(bytes, self.size.1) {
            replies.extend_from_slice(format!("\x1bP>|{NAME}\x1b\\").as_bytes());
        }
        if !replies.is_empty() {
            // Called under the state lock, so never wait: replies to a
            // program that does not read its input are dropped (and the
            // writer is gone only once the terminal is).
            let _ = self.input.try_send(replies);
        }
    }

    pub fn resize(&mut self, size: (u16, u16)) -> Result<()> {
        if size != self.size {
            self.size = size;
            self.emulator.resize(size.0, size.1);
            // As alacritty does on a resize.
            self.scanner.region = None;
            pty::resize(&self.master, size)?;
        }
        Ok(())
    }

    /// The bytes that rebuild the terminal in a fresh emulator of its
    /// size: the emulator's snapshot with the scroll region, and
    /// modifyOtherKeys, which alacritty does not keep.
    pub fn snapshot(&mut self) -> Vec<u8> {
        let mut bytes = self.emulator.snapshot(self.scanner.region);
        let level = self.keys.modify_other_keys();
        if level != 0 {
            bytes.extend_from_slice(format!("\x1b[>4;{level}m").as_bytes());
        }
        bytes
    }

    /// Lines of scrollback above the screen.
    pub fn scrollback(&self) -> usize {
        self.emulator.history_len()
    }

    /// The foreground program's pid, else the shell's.
    fn foreground(&self) -> i32 {
        pty::foreground(&self.master).unwrap_or(i32::try_from(self.pid).unwrap_or(0))
    }

    /// The foreground program's directory, as tmux's `pane_current_path`.
    pub fn dir(&self) -> Option<String> {
        pty::cwd(self.foreground()).or_else(|| pty::cwd(i32::try_from(self.pid).ok()?))
    }

    /// The foreground program's name, as tmux's `pane_current_command`.
    pub fn command(&self) -> Option<String> {
        pty::name(self.foreground())
    }
}

/// What alacritty does not tell of a program's output, read across
/// reads: XTVERSION queries (`CSI > q`, `CSI > 0 q`), and the scroll
/// region (DECSTBM, `CSI top ; bottom r`), which alacritty keeps private.
#[derive(Default)]
struct Scanner {
    state: State,
    params: Vec<u8>,
    /// The scroll region's first and last rows, from 1, as DECSTBM
    /// writes them; `None` for the whole screen.
    region: Option<(u16, u16)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
}

/// Longer parameters are no sequence of ours.
const MAX_PARAMS: usize = 16;

impl Scanner {
    /// Read `bytes`, written to a screen of `rows`; how many XTVERSION
    /// queries they finish.
    fn feed(&mut self, bytes: &[u8], rows: u16) -> usize {
        let mut found = 0;
        for &b in bytes {
            self.state = match (self.state, b) {
                (_, 0x1b) => State::Escape,
                (State::Escape, b'[') => {
                    self.params.clear();
                    State::Csi
                }
                // RIS: a full reset.
                (State::Escape, b'c') => {
                    self.region = None;
                    State::Ground
                }
                // CAN and SUB cancel a sequence; other C0 controls inside
                // one are carried out and leave it going on, as in vte.
                (State::Escape | State::Csi, 0x18 | 0x1a) => State::Ground,
                (state @ (State::Escape | State::Csi), 0x00..=0x1f) => state,
                (State::Csi, 0x40..=0x7e) => {
                    if b == b'q' && matches!(self.params.as_slice(), b">" | b">0") {
                        found += 1;
                    }
                    if b == b'r' {
                        self.scroll_region(rows);
                    }
                    State::Ground
                }
                (State::Csi, _) if self.params.len() < MAX_PARAMS => {
                    self.params.push(b);
                    State::Csi
                }
                _ => State::Ground,
            };
        }
        found
    }

    /// DECSTBM with the parameters read, as alacritty takes it: a missing
    /// or 0 top is the first row, a missing or 0 bottom the last, and a
    /// top not above the bottom changes nothing.
    fn scroll_region(&mut self, rows: u16) {
        // `CSI ? … r` and the like are other sequences.
        if !self.params.iter().all(|&b| b.is_ascii_digit() || b == b';') {
            return;
        }
        let text = String::from_utf8_lossy(&self.params);
        let mut nums = text.split(';').map(|n| n.parse::<u16>().unwrap_or(0));
        let top = nums.next().filter(|&n| n != 0).unwrap_or(1);
        let bottom = nums.next().filter(|&n| n != 0).unwrap_or(rows);
        if top >= bottom {
            return;
        }
        // alacritty clamps after that check, so `CSI 30;40 r` on 24 rows
        // leaves it an empty region, and `CSI 24;30 r` one of the last
        // row. No DECSTBM rebuilds either (top must be above bottom):
        // those are left as the whole screen.
        let (top, bottom) = (top.min(rows), bottom.min(rows));
        let whole = top >= bottom || (top == 1 && bottom == rows);
        self.region = (!whole).then_some((top, bottom));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xtversion_queries_are_found_across_reads_and_nothing_else_is() {
        let mut s = Scanner::default();
        assert_eq!(s.feed(b"\x1b[>q text \x1b[>0q", 24), 2);
        assert_eq!(s.feed(b"\x1b[", 24), 0);
        assert_eq!(s.feed(b">q", 24), 1);
        assert_eq!(s.feed(b"\x1b[q \x1b[>1q \x1b[31m \x1b[ q", 24), 0);
    }

    #[test]
    fn the_scroll_region_is_followed_across_reads_until_reset() {
        let mut s = Scanner::default();
        s.feed(b"\x1b[2;2", 24);
        s.feed(b"0r", 24);
        assert_eq!(s.region, Some((2, 20)));
        // Other sequences, and a region of no rows, change nothing.
        s.feed(b"\x1b[?5r \x1b[9;3r \x1b[>1r", 24);
        assert_eq!(s.region, Some((2, 20)));
        s.feed(b"\x1b[3;30r", 24);
        assert_eq!(s.region, Some((3, 24)), "clamped to the screen");
        s.feed(b"\x1b[;0r", 24);
        assert_eq!(s.region, None, "the whole screen");
        s.feed(b"\x1b[5;9r\x1bc", 24);
        assert_eq!(s.region, None, "RIS resets it");
        // A control inside the sequence is carried out, and it goes on;
        // CAN cancels it.
        s.feed(b"\x1b[2;\r\n5r", 24);
        assert_eq!(s.region, Some((2, 5)));
        s.feed(b"\x1b[3;\x187r", 24);
        assert_eq!(s.region, Some((2, 5)));
        // Regions past the screen's end that no DECSTBM can rebuild.
        s.feed(b"\x1b[30;40r", 24);
        assert_eq!(s.region, None);
        s.feed(b"\x1b[2;5r\x1b[24;30r", 24);
        assert_eq!(s.region, None);
    }
}
