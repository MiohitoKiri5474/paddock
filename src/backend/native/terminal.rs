//! One terminal as the native server holds it: its shell's PTY, and an
//! emulator fed with everything the shell writes. The emulator answers the
//! program's terminal queries (DA and `CSI ? u` through alacritty,
//! XTVERSION here), and makes the snapshot a client attaches with.

use super::pty::{self, Pty};
use super::queue::Queue;
use crate::backend::TerminalId;
use crate::terminal::emulator::Emulator;
use crate::terminal::key_modes::KeyModeScanner;
use crate::terminal::SCROLLBACK;
use anyhow::Result;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// How the server names itself to XTVERSION.
const NAME: &str = concat!("paddock ", env!("CARGO_PKG_VERSION"));
/// The most input that may wait for a program: an input that would take
/// what waits past it is refused whole.
pub const INPUT_MAX: usize = 64 << 20;

/// Bytes for a program, in order (input and query replies), written by a
/// thread of its own: queuing never waits, so neither the state lock nor
/// the connection the input came on (with its resizes and detach) waits
/// on a program that does not read. What is queued is never dropped.
pub struct Input {
    queue: Queue<Vec<u8>>,
    /// Input is being refused: from the first refusal until the program
    /// has taken all that waited (an episode).
    refusing: AtomicBool,
}

/// What became of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Queued,
    /// Refused, as it would take what waits past [`INPUT_MAX`]; `first`
    /// of an episode.
    Refused {
        first: bool,
    },
    /// Its writer has ended (the PTY is gone): nothing is written.
    Ended,
}

impl Input {
    /// Start the thread writing to `master`, terminal `id`'s PTY.
    pub fn start(id: &TerminalId, mut master: File) -> Self {
        let id = id.clone();
        let (queue, _) = Queue::start(Vec::len, move |bytes: Vec<u8>| {
            let written = master.write_all(&bytes);
            if let Err(e) = &written {
                eprintln!("paddock server: cannot write {id}'s input: {e}");
            }
            written.is_ok()
        });
        Self {
            queue,
            refusing: AtomicBool::new(false),
        }
    }

    /// Queue one whole input (a key, a paste), unless it would take what
    /// waits for the program past [`INPUT_MAX`].
    pub fn send(&self, bytes: Vec<u8>) -> Sent {
        if self.queue.ended() {
            return Sent::Ended;
        }
        if self.fits(bytes.len()) {
            if self.queue.push(bytes).is_some() {
                return Sent::Queued;
            }
            return Sent::Ended;
        }
        self.refused()
    }

    /// Refuse an input too big to ever queue, as [`Self::send`] would.
    pub fn refuse(&self) -> Sent {
        if self.queue.ended() {
            return Sent::Ended;
        }
        let _ = self.fits(0);
        self.refused()
    }

    /// Queue a reply to the program's query, unless it would take what
    /// waits past [`INPUT_MAX`]: then it is dropped, quietly (the notice
    /// is the user's input's).
    pub fn reply(&self, bytes: Vec<u8>) {
        let waiting = self.queue.waiting();
        if waiting + bytes.len() <= INPUT_MAX {
            let _ = self.queue.push(bytes);
        }
    }

    /// Whether `len` more bytes fit under [`INPUT_MAX`]. Nothing waiting
    /// means the program has taken all that waited: the episode ends.
    fn fits(&self, len: usize) -> bool {
        let waiting = self.queue.waiting();
        if waiting == 0 {
            self.refusing.store(false, Ordering::Relaxed);
        }
        waiting + len <= INPUT_MAX
    }

    /// A refusal: the episode's first, or not.
    fn refused(&self) -> Sent {
        let first = !self.refusing.swap(true, Ordering::Relaxed);
        Sent::Refused { first }
    }
}

pub struct HostedTerminal {
    pub id: TerminalId,
    /// The pid of its shell.
    pub pid: u32,
    pub tty: String,
    /// The PTY's master side, for its size and foreground.
    pub master: File,
    /// Bytes for the program: input and query replies.
    pub input: Input,
    pub size: (u16, u16),
    /// Its program has exited and been reaped: its pid may be another
    /// process's now, so it is never signalled again.
    pub exited: bool,
    emulator: Emulator,
    keys: KeyModeScanner,
    scanner: Scanner,
}

impl HostedTerminal {
    pub fn new(id: TerminalId, pty: &Pty, input: Input, size: (u16, u16)) -> Result<Self> {
        Ok(Self {
            id,
            pid: pty.child.id(),
            tty: pty.tty.clone(),
            master: pty.master.try_clone()?,
            input,
            size,
            exited: false,
            emulator: Emulator::answering(size.0, size.1, SCROLLBACK),
            keys: KeyModeScanner::default(),
            scanner: Scanner::default(),
        })
    }

    /// Hang up its program and its process group, as closing its
    /// terminal does; nothing once the program has been reaped.
    pub fn hang_up(&self) {
        let Ok(pid) = i32::try_from(self.pid) else {
            return;
        };
        if self.exited || pid <= 1 {
            return;
        }
        // SAFETY: kill has no memory-safety preconditions; `pid` > 1 leads
        // its own session (setsid), so `-pid` is its group only.
        unsafe {
            libc::kill(-pid, libc::SIGHUP);
            libc::kill(pid, libc::SIGHUP);
        }
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
            self.input.reply(replies);
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

    /// The name of a tab that follows what runs in this terminal, as
    /// paddock's tmux config names one: a shell is named after its
    /// directory, anything else after its command. `None` while it cannot
    /// say (the shell not started yet).
    pub fn auto_name(&self) -> Option<String> {
        let foreground = self.foreground();
        let command = pty::name(foreground)?;
        // Forked from the server, not yet the shell.
        if command == "paddock" && u32::try_from(foreground).is_ok_and(|p| p == self.pid) {
            return None;
        }
        if !is_shell(&command) {
            return Some(command);
        }
        pty::cwd(foreground).map(|dir| dir_name(&dir))
    }
}

/// Whether `command` is a shell, as paddock's tmux config tells one
/// (`^-?(ba|z|fi|da|k|tc|c)?sh$`): `ssh` and `mosh` are not.
fn is_shell(command: &str) -> bool {
    let name = command.strip_prefix('-').unwrap_or(command);
    let Some(kind) = name.strip_suffix("sh") else {
        return false;
    };
    matches!(kind, "" | "ba" | "z" | "fi" | "da" | "k" | "tc" | "c")
}

/// A directory's last part (`/` for the root), as a tab is named after it.
pub fn dir_name(dir: &str) -> String {
    let path = std::path::Path::new(dir);
    path.file_name()
        .map_or_else(|| dir.to_string(), |n| n.to_string_lossy().into_owned())
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
    use std::os::fd::{FromRawFd, OwnedFd};

    /// Input to a program that reads nothing: a pipe nobody reads, and its
    /// reading end, kept open.
    fn unread_input() -> (Input, OwnedFd) {
        let mut fds = [0; 2];
        // SAFETY: pipe writes two descriptors into the array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: pipe succeeded, so both are open descriptors that nothing
        // else owns.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        (Input::start(&TerminalId::new("%1"), write), read)
    }

    #[test]
    fn an_input_that_would_pass_the_cap_is_refused_whole_and_told_once() {
        let (input, _read) = unread_input();
        // Far more than the pipe holds: it waits, whole.
        assert_eq!(input.send(vec![b'x'; INPUT_MAX - 10]), Sent::Queued);
        assert_eq!(input.send(vec![b'k'; 5]), Sent::Queued, "under the cap");
        // A reply that fits goes in, and uses up no notice.
        input.reply(vec![b'r'; 100]);
        assert_eq!(
            input.send(vec![b'p'; 1 << 20]),
            Sent::Refused { first: true }
        );
        assert_eq!(
            input.send(vec![b'p'; 1 << 20]),
            Sent::Refused { first: false }
        );
    }

    #[test]
    fn a_reply_past_the_cap_is_dropped_quietly() {
        let (input, _read) = unread_input();
        assert_eq!(input.send(vec![b'x'; INPUT_MAX]), Sent::Queued);
        let waiting = input.queue.waiting();
        input.reply(vec![b'r'; 1 << 20]);
        assert!(input.queue.waiting() <= waiting, "not queued");
        // The user's input still gets the episode's notice.
        assert_eq!(
            input.send(vec![b'k'; 1 << 20]),
            Sent::Refused { first: true }
        );
    }

    #[test]
    fn input_to_a_program_whose_pty_is_gone_ends() {
        let (input, read) = unread_input();
        drop(read);
        let start = std::time::Instant::now();
        while input.send(b"x".to_vec()) != Sent::Ended {
            assert!(start.elapsed().as_secs() < 5, "never ended");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(input.refuse(), Sent::Ended);
    }

    #[test]
    fn shells_are_named_after_their_directory_and_other_programs_after_themselves() {
        for shell in [
            "zsh", "-zsh", "bash", "sh", "fish", "dash", "ksh", "tcsh", "csh",
        ] {
            assert!(is_shell(shell), "{shell}");
        }
        for other in ["ssh", "mosh", "vim", "nush", "-", ""] {
            assert!(!is_shell(other), "{other}");
        }
        assert_eq!(dir_name("/Users/me/src/paddock"), "paddock");
        assert_eq!(dir_name("/"), "/");
    }

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
