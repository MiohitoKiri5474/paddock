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
use std::sync::mpsc::Sender;

/// How the server names itself to XTVERSION.
const NAME: &str = concat!("paddock ", env!("CARGO_PKG_VERSION"));

pub struct Host {
    pub id: TerminalId,
    /// The pid of its shell.
    pub pid: u32,
    pub tty: String,
    /// The PTY's master side, for its size and foreground.
    pub master: File,
    /// Bytes for the program, in order: input and query replies. A thread
    /// of its own writes them, so nobody waits on a program that does not
    /// read.
    pub input: Sender<Vec<u8>>,
    pub size: (u16, u16),
    emulator: Emulator,
    keys: KeyModeScanner,
    queries: QueryScanner,
}

impl Host {
    pub fn new(
        id: TerminalId,
        pty: &Pty,
        input: Sender<Vec<u8>>,
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
            queries: QueryScanner::default(),
        })
    }

    /// Feed what the program wrote, and send it the answers to the
    /// queries in it.
    pub fn output(&mut self, bytes: &[u8]) {
        self.keys.feed(bytes);
        self.emulator.feed(bytes);
        let mut replies = self.emulator.take_replies();
        for _ in 0..self.queries.feed(bytes) {
            replies.extend_from_slice(format!("\x1bP>|{NAME}\x1b\\").as_bytes());
        }
        if !replies.is_empty() {
            // The writer is gone only once the terminal is.
            let _ = self.input.send(replies);
        }
    }

    pub fn resize(&mut self, size: (u16, u16)) -> Result<()> {
        if size != self.size {
            self.size = size;
            self.emulator.resize(size.0, size.1);
            pty::resize(&self.master, size)?;
        }
        Ok(())
    }

    /// The bytes that rebuild the terminal in a fresh emulator of its
    /// size: the emulator's snapshot, and modifyOtherKeys, which
    /// alacritty does not keep.
    pub fn snapshot(&mut self) -> Vec<u8> {
        let mut bytes = self.emulator.snapshot();
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

/// Finds XTVERSION queries (`CSI > q`, `CSI > 0 q`) in a program's
/// output, across reads.
#[derive(Default)]
struct QueryScanner {
    state: State,
    params: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
}

/// Longer parameters are no query of ours.
const MAX_PARAMS: usize = 8;

impl QueryScanner {
    /// How many XTVERSION queries `bytes` finish.
    fn feed(&mut self, bytes: &[u8]) -> usize {
        let mut found = 0;
        for &b in bytes {
            self.state = match (self.state, b) {
                (_, 0x1b) => State::Escape,
                (State::Escape, b'[') => {
                    self.params.clear();
                    State::Csi
                }
                (State::Csi, 0x40..=0x7e) => {
                    if b == b'q' && matches!(self.params.as_slice(), b">" | b">0") {
                        found += 1;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xtversion_queries_are_found_across_reads_and_nothing_else_is() {
        let mut s = QueryScanner::default();
        assert_eq!(s.feed(b"\x1b[>q text \x1b[>0q"), 2);
        assert_eq!(s.feed(b"\x1b["), 0);
        assert_eq!(s.feed(b">q"), 1);
        assert_eq!(s.feed(b"\x1b[q \x1b[>1q \x1b[31m \x1b[ q"), 0);
    }
}
