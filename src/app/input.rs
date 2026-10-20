//! Applying outer-terminal input to the app, depending on its mode.

use super::layout::Split;
use super::state::{row_ids, App, Mode, RowId};
use crate::input::{encode_key, encode_paste, Command, KeyAction};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
            // `y` closes; any other key keeps the terminal, and is not passed on.
            Mode::ConfirmClose { terminal, back, .. } => {
                if key.code == KeyCode::Char('y') {
                    self.report(|app| app.server.kill_session(&terminal));
                }
                if let Some(d) = back {
                    self.mode = Mode::Dashboard(d);
                }
                Ok(Flow::Continue)
            }
            Mode::Dashboard(d) => match self.prefix.handle(key) {
                KeyAction::Command(c) => {
                    self.mode = Mode::Dashboard(d);
                    self.command(c)
                }
                KeyAction::Forward(key) => {
                    self.dashboard_key(d, key);
                    Ok(Flow::Continue)
                }
                KeyAction::None => {
                    self.mode = Mode::Dashboard(d);
                    Ok(Flow::Continue)
                }
            },
            Mode::Sidebar(selected) => match self.prefix.handle(key) {
                KeyAction::Command(c) => {
                    self.mode = Mode::Sidebar(selected);
                    self.command(c)
                }
                KeyAction::Forward(key) => {
                    self.sidebar_key(selected, key);
                    Ok(Flow::Continue)
                }
                KeyAction::None => {
                    self.mode = Mode::Sidebar(selected);
                    Ok(Flow::Continue)
                }
            },
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
            Command::Close => {
                if let Some(id) = self.focused() {
                    self.report(|app| app.close(&id));
                }
            }
            Command::Fork => {
                if let Some(id) = self.focused() {
                    self.report(|app| app.fork(&id));
                }
            }
            Command::SplitColumns => self.report(|app| app.split(Split::Columns)),
            Command::SplitRows => self.report(|app| app.split(Split::Rows)),
            Command::Focus(dir) => self.move_focus(dir),
            Command::Equalise => self.report(|app| app.equalise()),
            Command::Help => self.mode = Mode::Help,
            Command::Detach => return Ok(Flow::Quit),
            Command::FocusSidebar => match self.rows().first() {
                Some(row) => {
                    self.sidebar_hidden = false;
                    self.mode = Mode::Sidebar(Some(RowId {
                        pane: row.pane.clone(),
                        branch: None,
                    }));
                }
                None => self.notice = Some("no Claude sessions yet".into()),
            },
            Command::ToggleSidebar => {
                self.sidebar_hidden = !self.sidebar_hidden;
                if self.sidebar_hidden && matches!(self.mode, Mode::Sidebar(_)) {
                    self.mode = Mode::Normal;
                }
            }
            Command::NextAttention => self.next_attention(),
            Command::Dashboard => self.toggle_dashboard(),
        }
        // The sidebar may have come or gone.
        self.report(|app| app.relayout());
        // Keep the saved layout current, in case paddock does not get to detach.
        self.report(|app| app.save());
        Ok(Flow::Continue)
    }

    /// The focused terminal's tmux session id.
    fn focused(&self) -> Option<String> {
        self.tabs.current().map(|t| t.content.focus.clone())
    }

    /// `j`/`k` (or arrows) move over session and task rows, `Enter` jumps to
    /// the row's terminal, `x` closes it, `f` forks its session, `D`
    /// dismisses the row, `Esc` gives the keyboard back to the terminal. A
    /// closed terminal's row can only be dismissed.
    fn sidebar_key(&mut self, selected: Option<RowId>, key: KeyEvent) {
        let rows = self.rows();
        let ids = row_ids(&rows);
        let at = ids
            .iter()
            .position(|id| Some(id) == selected.as_ref())
            .unwrap_or(0);
        let row = ids
            .get(at)
            .and_then(|id| rows.iter().find(|r| r.pane == id.pane))
            .filter(|r| !r.closed);
        match (key.code, row) {
            (KeyCode::Char('x'), Some(row)) => {
                // Asking takes the keyboard; otherwise the sidebar keeps it.
                self.mode = Mode::Sidebar(selected);
                self.report(|app| app.close(&row.terminal));
                return self.report(|app| app.relayout());
            }
            (KeyCode::Char('f'), Some(row)) => {
                // The fork's tab gets the keyboard.
                self.report(|app| app.fork(&row.terminal));
                return self.report(|app| app.relayout());
            }
            _ => {}
        }
        if let (KeyCode::Char('D'), Some(id)) = (key.code, ids.get(at)) {
            self.report(|app| app.dismiss(id));
            // The line that took its place, else the one above.
            let ids = row_ids(&self.rows());
            self.mode = match ids.get(at.min(ids.len().saturating_sub(1))) {
                Some(next) => Mode::Sidebar(Some(next.clone())),
                None => Mode::Normal,
            };
            return self.report(|app| app.relayout());
        }
        let pick = |i: usize| ids.get(i).cloned();
        self.mode = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Mode::Sidebar(pick(at + 1).or(selected)),
            KeyCode::Char('k') | KeyCode::Up => {
                Mode::Sidebar(at.checked_sub(1).and_then(pick).or(selected))
            }
            KeyCode::Enter => match row {
                Some(row) => {
                    self.jump_to(row.tab, &row.terminal);
                    Mode::Normal
                }
                None => Mode::Sidebar(selected),
            },
            KeyCode::Esc => Mode::Normal,
            _ => Mode::Sidebar(selected),
        };
        self.report(|app| app.relayout());
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
        if let Mode::Dashboard(d) = &self.mode {
            let d = d.clone();
            return self.dashboard_paste(&d, text);
        }
        match &mut self.mode {
            Mode::Rename(name) => name.extend(text.chars().filter(|c| !c.is_control())),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Sidebar(_) | Mode::ConfirmClose { .. } | Mode::Dashboard(_) => {}
            Mode::Normal => {
                if let Some(t) = self.current_mut() {
                    let bytes = encode_paste(text, t.emulator().modes().bracketed_paste);
                    t.input(&bytes)?;
                }
            }
        }
        Ok(())
    }
}
