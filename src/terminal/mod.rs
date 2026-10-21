//! A terminal: one pane on the hidden server, as paddock sees it. It holds an
//! emulator fed by the pane's output; input goes through the control client
//! of the pane's tmux session (see `app::space`).

pub mod emulator;
pub mod key_modes;
pub mod screen_title;
pub mod search;
pub mod seed;

use emulator::Emulator;
use key_modes::{KeyModeScanner, KeyModes};
use screen_title::ScreenTitleFilter;

/// Lines of scrollback paddock keeps per terminal (matches tmux's history-limit).
pub const SCROLLBACK: usize = 50_000;

/// The most output a paused terminal holds back (8 MiB); past it, it goes
/// on by itself.
pub const MAX_HELD: usize = 8 * 1024 * 1024;

pub struct Terminal {
    emulator: Emulator,
    keys: KeyModeScanner,
    titles: ScreenTitleFilter,
    /// Output held back while paused (copy mode), fed when it goes on.
    held: Option<Vec<u8>>,
    /// The held output outgrew [`MAX_HELD`], so the terminal went on by
    /// itself.
    overflowed: bool,
}

impl Terminal {
    /// An empty terminal of `(cols, rows)`.
    pub fn new((cols, rows): (u16, u16)) -> Self {
        Self {
            emulator: Emulator::new(cols, rows, SCROLLBACK),
            keys: KeyModeScanner::default(),
            titles: ScreenTitleFilter::default(),
            held: None,
            overflowed: false,
        }
    }

    /// Feed bytes the program wrote, minus screen title escapes the emulator
    /// would draw as text. While paused, they are held back instead, up to
    /// [`MAX_HELD`]: past it the terminal goes on by itself.
    pub fn output(&mut self, bytes: &[u8]) {
        if let Some(held) = &mut self.held {
            held.extend_from_slice(bytes);
            if held.len() > MAX_HELD {
                self.overflowed = true;
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
            self.go_on();
            self.emulator.resize(cols, rows);
        }
    }

    /// Hold the program's output back: the view stays as it is.
    pub fn pause(&mut self) {
        if self.held.is_none() {
            self.held = Some(Vec::new());
            self.overflowed = false;
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

    /// Whether the terminal went on by itself because it held too much,
    /// since this was last asked.
    pub fn take_overflowed(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
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
        assert!(!t.take_overflowed());
    }

    #[test]
    fn too_much_held_output_or_a_resize_lets_it_go_on() {
        let mut t = Terminal::new((10, 2));
        t.pause();
        t.output(&vec![b'x'; MAX_HELD + 1]);
        assert!(!t.is_paused());
        assert!(t.take_overflowed());
        assert!(!t.take_overflowed(), "told once");
        t.pause();
        t.output(b"\r\nlate");
        t.resize((12, 2));
        assert!(!t.is_paused());
        assert_eq!(t.emulator().row_text(1), "late");
    }
}
