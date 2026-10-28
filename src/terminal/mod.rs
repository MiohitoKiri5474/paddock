//! A terminal as paddock sees it. It holds an emulator fed by the
//! terminal's output; input goes through its space's connection to the
//! backend (see `app::space`).

pub mod emulator;
pub mod key_modes;
pub mod program_status;
pub mod screen_title;
pub mod search;

use emulator::Emulator;
use key_modes::{KeyModeScanner, KeyModes};
use program_status::{Report, StatusScanner};
use screen_title::ScreenTitleFilter;

/// Lines of scrollback paddock keeps per terminal (matches tmux's history-limit).
pub const SCROLLBACK: usize = 50_000;

/// The most output a paused terminal holds back (8 MiB); past it, it goes
/// on by itself.
pub const MAX_HELD: usize = 8 * 1024 * 1024;

/// Why a paused terminal went on by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WentOn {
    /// The held output outgrew [`MAX_HELD`].
    Overflowed,
    /// The terminal was resized.
    Resized,
}

pub struct Terminal {
    emulator: Emulator,
    keys: KeyModeScanner,
    titles: ScreenTitleFilter,
    status: StatusScanner,
    /// Status reports read from the output, until taken.
    reports: Vec<Report>,
    /// Output held back while paused (copy mode), fed when it goes on.
    held: Option<Vec<u8>>,
    /// Why the terminal last went on by itself, until asked.
    went_on: Option<WentOn>,
}

impl Terminal {
    /// An empty terminal of `(cols, rows)`.
    pub fn new((cols, rows): (u16, u16)) -> Self {
        Self {
            emulator: Emulator::new(cols, rows, SCROLLBACK),
            keys: KeyModeScanner::default(),
            titles: ScreenTitleFilter::default(),
            status: StatusScanner::default(),
            reports: Vec::new(),
            held: None,
            went_on: None,
        }
    }

    /// Feed bytes the program wrote, minus screen title escapes the emulator
    /// would draw as text. While paused, they are held back instead, up to
    /// [`MAX_HELD`]: past it the terminal goes on by itself. Status reports
    /// (OSC 7501) are read at once, paused or not: the program's state
    /// does not wait for the view.
    pub fn output(&mut self, bytes: &[u8]) {
        self.reports.extend(self.status.scan(bytes));
        // While paused, the key-mode scanner and the title filter wait for
        // the bytes too: they see them, in order, when the terminal goes
        // on, and nothing reads their state meanwhile, as no key, paste
        // or mouse report reaches a terminal in copy mode.
        if let Some(held) = &mut self.held {
            held.extend_from_slice(bytes);
            if held.len() > MAX_HELD {
                self.went_on = Some(WentOn::Overflowed);
                self.go_on();
            }
            return;
        }
        self.feed(bytes);
    }

    fn feed(&mut self, bytes: &[u8]) {
        let bytes = self.titles.filter(bytes);
        self.keys.feed(&bytes);
        self.emulator.feed(&bytes);
    }

    /// The status reports read since the last call, in order.
    pub fn take_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }

    pub fn key_modes(&self) -> KeyModes {
        KeyModes {
            app_cursor: self.emulator.modes().app_cursor,
            kitty_flags: self.keys.kitty_flags(),
            modify_other_keys: self.keys.modify_other_keys(),
        }
    }

    /// Resize; a paused terminal goes on first, so what it held is drawn
    /// at the size it was written for.
    pub fn resize(&mut self, (cols, rows): (u16, u16)) {
        if self.emulator.size() != (cols, rows) {
            if self.is_paused() {
                self.went_on = Some(WentOn::Resized);
            }
            self.go_on();
            self.emulator.resize(cols, rows);
        }
    }

    /// Hold the program's output back: the view stays as it is.
    pub fn pause(&mut self) {
        if self.held.is_none() {
            self.held = Some(Vec::new());
            self.went_on = None;
        }
    }

    /// Whether output is held back.
    pub fn is_paused(&self) -> bool {
        self.held.is_some()
    }

    /// Feed the output held back, if paused, and stop holding it.
    pub fn go_on(&mut self) {
        if let Some(held) = self.held.take() {
            self.feed(&held);
        }
    }

    /// Why the terminal went on by itself, if it did since this was last
    /// asked.
    pub fn take_went_on(&mut self) -> Option<WentOn> {
        self.went_on.take()
    }

    pub fn emulator(&self) -> &Emulator {
        &self.emulator
    }

    pub fn emulator_mut(&mut self) -> &mut Emulator {
        &mut self.emulator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reports_are_read_even_while_paused() {
        let mut t = Terminal::new((20, 2));
        t.pause();
        t.output(b"\x1b]7501;state=working:app=antigravity\x07");
        let got = t.take_reports();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].state, crate::sessions::ProgramState::Working);
        assert!(t.take_reports().is_empty(), "taken once");
        t.go_on();
        assert!(t.take_reports().is_empty(), "not again when it goes on");
        assert_eq!(t.emulator().row_text(0), "", "and never drawn");
    }

    #[test]
    fn a_paused_terminal_holds_output_back_until_it_goes_on() {
        let mut t = Terminal::new((10, 2));
        t.output(b"one");
        t.pause();
        t.output(b"\r\ntwo");
        assert!(t.is_paused());
        assert_eq!(t.emulator().row_text(1), "");
        t.go_on();
        assert!(!t.is_paused());
        assert_eq!(t.emulator().row_text(1), "two");
        assert_eq!(t.take_went_on(), None);
    }

    #[test]
    fn too_much_held_output_or_a_resize_lets_it_go_on() {
        let mut t = Terminal::new((10, 2));
        t.pause();
        t.output(&vec![b'x'; MAX_HELD + 1]);
        assert!(!t.is_paused());
        assert_eq!(t.take_went_on(), Some(WentOn::Overflowed));
        assert_eq!(t.take_went_on(), None, "told once");
        t.pause();
        t.output(b"\r\nlate");
        t.resize((12, 2));
        assert!(!t.is_paused());
        assert_eq!(t.emulator().row_text(1), "late");
        assert_eq!(t.take_went_on(), Some(WentOn::Resized));
        // A resize while not paused is no reason to tell.
        t.resize((14, 2));
        assert_eq!(t.take_went_on(), None);
    }
}
