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

/// The most output a terminal keeps while its scrollback is still to come
/// (8 MiB, see [`Terminal::await_scrollback`]); past it, the scrollback
/// is not waited for.
pub const MAX_SINCE_SCREENS: usize = 8 * 1024 * 1024;

/// Why a paused terminal went on by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WentOn {
    /// The held output outgrew [`MAX_HELD`].
    Overflowed,
    /// The terminal was resized.
    Resized,
}

/// A fresh emulator of `(cols, rows)` fed `snapshot` (a terminal's
/// snapshot with its scrollback), for [`Terminal::take_scrollback`]. Its
/// own call, so the work can be done off the event loop.
pub fn rebuild((cols, rows): (u16, u16), snapshot: &[u8]) -> Emulator {
    let mut emulator = Emulator::new(cols, rows, SCROLLBACK);
    emulator.feed(snapshot);
    emulator
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
    /// While its scrollback is still to come ([`Self::await_scrollback`]):
    /// what its emulator was fed and how it was resized since, in order,
    /// for the emulator that takes its place.
    since_screens: Option<Vec<Fed>>,
    /// How many bytes `since_screens` holds.
    since_screens_len: usize,
}

/// What a terminal's emulator went through, for [`Terminal::take_scrollback`].
enum Fed {
    Bytes(Vec<u8>),
    Resize(u16, u16),
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
            since_screens: None,
            since_screens_len: 0,
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
        let Some(fed) = &mut self.since_screens else {
            return;
        };
        self.since_screens_len += bytes.len();
        // Past the cap, or once the output alone fills the scrollback
        // (the one to come has scrolled out whole), the scrollback is not
        // waited for: this emulator, which holds all since the screens,
        // stays.
        if self.since_screens_len > MAX_SINCE_SCREENS || self.emulator.history_len() >= SCROLLBACK {
            self.since_screens = None;
            self.since_screens_len = 0;
        } else if let Some(Fed::Bytes(last)) = fed.last_mut() {
            last.extend_from_slice(&bytes);
        } else {
            fed.push(Fed::Bytes(bytes));
        }
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
            if let Some(fed) = &mut self.since_screens {
                fed.push(Fed::Resize(cols, rows));
            }
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

    /// Its emulator holds the screens alone, fed it just now: its
    /// scrollback comes later ([`Self::take_scrollback`]). Until then,
    /// what the emulator goes through is kept, up to
    /// [`MAX_SINCE_SCREENS`]. It starts with the emulator's size, which
    /// the snapshot's may not be (one under the smallest paddock gives).
    pub fn await_scrollback(&mut self) {
        let (cols, rows) = self.emulator.size();
        self.since_screens = Some(vec![Fed::Resize(cols, rows)]);
        self.since_screens_len = 0;
    }

    /// Take `emulator` in place of its own: one rebuilt by [`rebuild`]
    /// from the snapshot of the same moment as the screens this terminal
    /// was rebuilt from, but with the scrollback. What this terminal's
    /// emulator was fed and how it was resized since (see
    /// [`Self::await_scrollback`]) goes through it again, so it shows the
    /// same, scroll offset and selection too, over the whole scrollback.
    /// Ignored if the terminal no longer waits for it (more output came
    /// meanwhile than is kept).
    pub fn take_scrollback(&mut self, mut emulator: Emulator) {
        let Some(since_screens) = self.since_screens.take() else {
            return;
        };
        self.since_screens_len = 0;
        for fed in since_screens {
            match fed {
                Fed::Bytes(bytes) => emulator.feed(&bytes),
                Fed::Resize(cols, rows) => emulator.resize(cols, rows),
            }
        }
        let (cols, rows) = self.emulator.size();
        if emulator.size() != (cols, rows) {
            emulator.resize(cols, rows);
        }
        emulator.keep_view(&self.emulator);
        self.emulator = emulator;
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

    /// A terminal of `size` with lines 0 to `lines - 1`, then `then`.
    fn emulator_with(size: (u16, u16), lines: usize, then: &[u8]) -> Emulator {
        let mut e = Emulator::new(size.0, size.1, SCROLLBACK);
        for i in 0..lines {
            e.feed(format!("\x1b[3{}mline {i}\x1b[0m\r\n", i % 8).as_bytes());
        }
        e.feed(then);
        e
    }

    /// A terminal rebuilt from `e`'s screens, as a client attaches: then
    /// the scrollback comes, rebuilt from its whole snapshot.
    fn attached(e: &mut Emulator) -> (Terminal, Emulator) {
        let mut t = Terminal::new(e.size());
        t.output(&e.screens(None));
        t.await_scrollback();
        assert_eq!(t.emulator().history_len(), 0, "the screens alone first");
        (t, rebuild(e.size(), &e.snapshot(None)))
    }

    #[test]
    fn the_scrollback_after_the_screens_rebuilds_the_terminal_whole() {
        for then in [&b"tail\x1b[2;3H"[..], b"\x1b[?1049h\x1b[Hfull\r\nscreen"] {
            let mut e = emulator_with((12, 4), 30, then);
            let (mut t, rebuilt) = attached(&mut e);
            t.take_scrollback(rebuilt);
            assert_eq!(t.emulator().history_len(), e.history_len());
            assert_eq!(t.emulator_mut().snapshot(None), e.snapshot(None));
            // What comes next continues it the same.
            t.output(b"\x1b[?1049lnext\r\n");
            e.feed(b"\x1b[?1049lnext\r\n");
            assert_eq!(t.emulator_mut().snapshot(None), e.snapshot(None));
        }
    }

    #[test]
    fn output_and_resizes_before_the_scrollback_go_through_it_again() {
        let mut e = emulator_with((12, 4), 30, b"tail");
        let (mut t, rebuilt) = attached(&mut e);
        let mut both = |f: &dyn Fn(&mut Terminal, &mut Emulator)| f(&mut t, &mut e);
        both(&|t, e| {
            t.output(b"\r\nmore one\r\nmore two\r\n");
            e.feed(b"\r\nmore one\r\nmore two\r\n");
        });
        both(&|t, e| {
            t.resize((9, 5));
            e.resize(9, 5);
        });
        both(&|t, e| {
            t.output(b"after\x1b]0;title\x07 resize");
            e.feed(b"after resize");
        });
        assert!(t.emulator().history_len() < e.history_len());
        t.take_scrollback(rebuilt);
        assert_eq!(t.emulator().history_len(), e.history_len());
        assert_eq!(t.emulator_mut().snapshot(None), e.snapshot(None));
        // Kept only until then.
        t.output(b"\r\nlast");
        e.feed(b"\r\nlast");
        assert_eq!(t.emulator_mut().snapshot(None), e.snapshot(None));
        assert!(t.since_screens.is_none());
    }

    #[test]
    fn past_the_cap_the_scrollback_is_not_waited_for() {
        let mut e = emulator_with((12, 4), 30, b"tail");
        let (mut t, rebuilt) = attached(&mut e);
        // Output that redraws in place: it never fills the history.
        let redraw = b"\x1b[Hsame screen, again".repeat(MAX_SINCE_SCREENS / 20 + 1);
        t.output(&redraw);
        e.feed(&redraw);
        assert!(t.since_screens.is_none(), "no longer kept");
        let live = t.emulator_mut().snapshot(None);
        t.take_scrollback(rebuilt);
        assert_eq!(
            t.emulator_mut().snapshot(None),
            live,
            "the live state stays"
        );
        assert_eq!(t.emulator().history_len(), 0);
        assert_eq!(t.emulator().row_text(0), e.row_text(0));
    }

    #[test]
    fn once_the_output_alone_fills_the_history_the_scrollback_is_not_waited_for() {
        let mut e = emulator_with((12, 4), 30, b"tail");
        let (mut t, rebuilt) = attached(&mut e);
        for i in 0..SCROLLBACK + 4 {
            let line = format!("{i}\r\n");
            t.output(line.as_bytes());
            e.feed(line.as_bytes());
        }
        assert!(t.since_screens.is_none());
        t.take_scrollback(rebuilt);
        assert_eq!(
            t.emulator_mut().snapshot(None),
            e.snapshot(None),
            "nothing lost"
        );
    }

    #[test]
    fn the_scrollback_takes_the_size_the_terminal_has_since() {
        let mut e = emulator_with((12, 4), 30, b"tail");
        let (mut t, rebuilt) = attached(&mut e);
        t.resize((8, 6));
        t.take_scrollback(rebuilt);
        e.resize(8, 6);
        assert_eq!(t.emulator().size(), (8, 6));
        assert_eq!(t.emulator().history_len(), e.history_len());
        assert_eq!(t.emulator_mut().snapshot(None), e.snapshot(None));
    }

    #[test]
    fn the_scrollback_keeps_the_view_and_the_selection() {
        let mut e = emulator_with((12, 4), 30, b"tail");
        let (mut t, rebuilt) = attached(&mut e);
        t.emulator_mut().select(1, 0);
        t.emulator_mut().select_to(2, 3);
        let selected = t.emulator().selected_text();
        t.take_scrollback(rebuilt);
        assert_eq!(t.emulator().scroll_offset(), 0);
        assert_eq!(t.emulator().selected_text(), selected);
        // Once there, the view scrolls back into it.
        t.emulator_mut().scroll(i32::MAX);
        assert_eq!(t.emulator().scroll_offset(), e.history_len());
        assert_eq!(t.emulator().row_text(0), "line 0");
    }

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
