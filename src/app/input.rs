//! Applying outer-terminal input to the app, depending on its mode.

use super::layout::Split;
use super::state::{App, Mode};
use crate::input::{encode_key, encode_paste, route_mouse, Command, KeyAction, MouseAction};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Position;

/// What the event loop should do next.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

impl App {
    pub fn key(&mut self, key: KeyEvent) -> Result<Flow> {
        self.notice = None;
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            // Any key closes the help, and is not passed on.
            Mode::Help => Ok(Flow::Continue),
            Mode::Rename(text) => {
                self.rename_key(text, key);
                Ok(Flow::Continue)
            }
            Mode::Normal => match self.prefix.handle(key) {
                KeyAction::Forward(key) => {
                    if let Some(t) = self.current_mut() {
                        if let Some(bytes) = encode_key(&key, t.key_modes()) {
                            t.input(&bytes)?;
                        }
                    }
                    Ok(Flow::Continue)
                }
                KeyAction::Command(c) => self.command(c),
                KeyAction::None => Ok(Flow::Continue),
            },
        }
    }

    fn command(&mut self, command: Command) -> Result<Flow> {
        match command {
            Command::NewTab => self.report(|app| app.new_tab()),
            Command::NextTab => self.tabs.next(),
            Command::PreviousTab => self.tabs.previous(),
            Command::GoToTab(i) => {
                self.tabs.select(i);
            }
            Command::RenameTab => {
                let name = self
                    .tabs
                    .current()
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                self.mode = Mode::Rename(name);
            }
            Command::Close => self.report(|app| app.close()),
            Command::SplitColumns => self.report(|app| app.split(Split::Columns)),
            Command::SplitRows => self.report(|app| app.split(Split::Rows)),
            Command::Focus(dir) => self.move_focus(dir),
            Command::Equalise => self.report(|app| app.equalise()),
            Command::Help => self.mode = Mode::Help,
            Command::Detach => return Ok(Flow::Quit),
        }
        // Keep the saved layout current, in case paddock does not get to detach.
        self.report(|app| app.save());
        Ok(Flow::Continue)
    }

    /// Run a command whose failure the user should see but that is not fatal.
    pub(super) fn report(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) {
        if let Err(e) = f(self) {
            self.notice = Some(format!("{e:#}"));
        }
    }

    fn rename_key(&mut self, mut text: String, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => {
                self.report(|app| app.rename_current(&text));
                return;
            }
            KeyCode::Esc => return,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char('u') if ctrl => text.clear(),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => text.push(c),
            _ => {}
        }
        self.mode = Mode::Rename(text);
    }

    pub fn paste(&mut self, text: &str) -> Result<()> {
        match &mut self.mode {
            Mode::Rename(name) => name.extend(text.chars().filter(|c| !c.is_control())),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Normal => {
                if let Some(t) = self.current_mut() {
                    let bytes = encode_paste(text, t.emulator().modes().bracketed_paste);
                    t.input(&bytes)?;
                }
            }
        }
        Ok(())
    }

    /// Mouse events over the focused terminal, in its coordinates. The tab
    /// bar and the other terminals do not take the mouse yet.
    pub fn mouse(&mut self, m: MouseEvent) -> Result<bool> {
        if !matches!(self.mode, Mode::Normal) {
            return Ok(false);
        }
        let area = self.area();
        let Some(rect) = self.tabs.current().and_then(|t| t.content.focus_rect(area)) else {
            return Ok(false);
        };
        if !rect.contains(Position::new(m.column, m.row)) {
            return Ok(false);
        }
        let Some(t) = self.current_mut() else {
            return Ok(false);
        };
        match route_mouse(&m, m.column - rect.x, m.row - rect.y, t.emulator().modes()) {
            MouseAction::Report(bytes) => t.input(&bytes)?,
            MouseAction::Scroll(lines) => t.emulator_mut().scroll(lines),
            MouseAction::Ignore => return Ok(false),
        }
        Ok(true)
    }
}
