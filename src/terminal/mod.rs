//! A terminal: one shell on the hidden server, as paddock sees it. It owns a
//! control client for its tmux session and an emulator fed by its output.

pub mod emulator;
pub mod key_modes;
pub mod seed;

use crate::tmux::{wait_reply, ControlClient, Message, Server};
use anyhow::Result;
use emulator::Emulator;
use key_modes::{KeyModeScanner, KeyModes};

/// Lines of scrollback paddock keeps per terminal (matches tmux's history-limit).
pub const SCROLLBACK: usize = 50_000;

pub struct Terminal {
    session: String,
    pane: String,
    emulator: Emulator,
    keys: KeyModeScanner,
    control: ControlClient,
}

impl Terminal {
    /// Attach to the tmux session `session` (its `$N` id) at the given size and
    /// rebuild its screen and history. Afterwards every control message for it,
    /// in order, goes to `deliver`.
    pub fn attach(
        server: &Server,
        session: &str,
        (cols, rows): (u16, u16),
        deliver: impl FnMut(Message) -> bool + Send + 'static,
    ) -> Result<Self> {
        let pane = server.display(session, "#{pane_id}")?;
        let (mut control, rx) = ControlClient::attach(server, session)?;
        control.resize(cols, rows)?;
        wait_reply(&rx)?;
        let seed = seed::fetch(&mut control, &rx, &pane)?;
        control.forward(&rx, deliver);
        let mut terminal = Self {
            session: session.to_string(),
            pane,
            emulator: Emulator::new(cols, rows, SCROLLBACK),
            keys: KeyModeScanner::default(),
            control,
        };
        terminal.output(&seed::stream(&seed)?);
        Ok(terminal)
    }

    /// The tmux session id (`$N`) this terminal runs in.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The tmux pane id (`%N`) of its shell; hooks name terminals by it.
    pub fn pane(&self) -> &str {
        &self.pane
    }

    /// Feed bytes the program wrote.
    pub fn output(&mut self, bytes: &[u8]) {
        self.keys.feed(bytes);
        self.emulator.feed(bytes);
    }

    /// Send bytes to the program, returning the view to the live screen.
    pub fn input(&mut self, bytes: &[u8]) -> Result<()> {
        self.emulator.scroll_to_bottom();
        self.control.send_bytes(&self.pane, bytes)
    }

    pub fn key_modes(&self) -> KeyModes {
        KeyModes {
            app_cursor: self.emulator.modes().app_cursor,
            kitty_flags: self.keys.kitty_flags(),
            modify_other_keys: self.keys.modify_other_keys(),
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.emulator.resize(cols, rows);
        self.control.resize(cols, rows)
    }

    pub fn emulator(&self) -> &Emulator {
        &self.emulator
    }

    pub fn emulator_mut(&mut self) -> &mut Emulator {
        &mut self.emulator
    }

    /// Leave the terminal running on the hidden server.
    pub fn detach(self) {
        self.control.detach();
    }
}
