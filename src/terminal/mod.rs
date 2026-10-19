//! A terminal: one shell on the hidden server, as paddock sees it. It owns a
//! control client for its tmux session and an emulator fed by its output.

pub mod emulator;
pub mod key_modes;
pub mod seed;

use crate::tmux::{wait_reply, ControlClient, Message, Server};
use anyhow::Result;
use emulator::Emulator;
use key_modes::{KeyModeScanner, KeyModes};
use std::sync::mpsc::{Receiver, Sender};

/// Lines of scrollback paddock keeps per terminal (matches tmux's history-limit).
pub const SCROLLBACK: usize = 50_000;

pub struct Terminal {
    pane: String,
    emulator: Emulator,
    keys: KeyModeScanner,
    control: ControlClient,
}

impl Terminal {
    /// Attach to an existing tmux session at the given size and rebuild its
    /// screen and history. Its output then arrives on `tx` as [`Message`]s.
    pub fn attach<T: Send + 'static>(
        server: &Server,
        session: &str,
        (cols, rows): (u16, u16),
        channel: (Sender<T>, &Receiver<T>),
        wrap: fn(Message) -> T,
        unwrap: fn(T) -> Option<Message>,
    ) -> Result<Self> {
        let (tx, rx) = channel;
        let pane = server.pane_id(session)?;
        let mut control = ControlClient::attach(server, session, tx, wrap)?;
        control.resize(cols, rows)?;
        wait_reply(rx, unwrap)?;
        let seed = seed::fetch(&mut control, rx, unwrap, &pane)?;
        let mut terminal = Self {
            pane,
            emulator: Emulator::new(cols, rows, SCROLLBACK),
            keys: KeyModeScanner::default(),
            control,
        };
        terminal.output(&seed::stream(&seed)?);
        Ok(terminal)
    }

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
