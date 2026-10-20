//! A terminal: one pane on the hidden server, as paddock sees it. It holds an
//! emulator fed by the pane's output; input goes through the control client
//! of the pane's tmux session (see `app::item`).

pub mod emulator;
pub mod key_modes;
pub mod screen_title;
pub mod seed;

use emulator::Emulator;
use key_modes::{KeyModeScanner, KeyModes};
use screen_title::ScreenTitleFilter;

/// Lines of scrollback paddock keeps per terminal (matches tmux's history-limit).
pub const SCROLLBACK: usize = 50_000;

pub struct Terminal {
    emulator: Emulator,
    keys: KeyModeScanner,
    titles: ScreenTitleFilter,
}

impl Terminal {
    /// An empty terminal of `(cols, rows)`.
    pub fn new((cols, rows): (u16, u16)) -> Self {
        Self {
            emulator: Emulator::new(cols, rows, SCROLLBACK),
            keys: KeyModeScanner::default(),
            titles: ScreenTitleFilter::default(),
        }
    }

    /// Feed bytes the program wrote, minus screen title escapes the emulator
    /// would draw as text.
    pub fn output(&mut self, bytes: &[u8]) {
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

    pub fn resize(&mut self, (cols, rows): (u16, u16)) {
        if self.emulator.size() != (cols, rows) {
            self.emulator.resize(cols, rows);
        }
    }

    pub fn emulator(&self) -> &Emulator {
        &self.emulator
    }

    pub fn emulator_mut(&mut self) -> &mut Emulator {
        &mut self.emulator
    }
}
