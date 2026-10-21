//! Applying outer-terminal input to the app, depending on its mode.

use super::state::{App, Mode, Naming};
use crate::input::line::LineEdit;
use crate::input::{encode_key, encode_paste, Command, KeyAction};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

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
            Mode::Name { target, line } => {
                self.name_key(target, line, key);
                Ok(Flow::Continue)
            }
            // `y` closes; any other key keeps it, and is not passed on.
            Mode::ConfirmClose { target, back, .. } => {
                if key.code == KeyCode::Char('y') {
                    self.report(|app| app.close_now(&target));
                }
                if let Some(d) = back {
                    self.mode = Mode::Dashboard(d);
                }
                Ok(Flow::Continue)
            }
            Mode::Dashboard(d) => {
                let kept = Mode::Dashboard(d.clone());
                self.panel_key(key, kept, |app, key| app.dashboard_key(d, key))
            }
            Mode::Sidebar(selected) => {
                let kept = Mode::Sidebar(selected.clone());
                self.panel_key(key, kept, |app, key| app.sidebar_key(selected, key))
            }
            Mode::Normal => self.normal_key(key),
        }
    }

    /// A key while a panel (the sidebar or the dashboard) has the keyboard:
    /// commands work and keep it in `kept`; other keys go to `on_key`.
    fn panel_key(
        &mut self,
        key: KeyEvent,
        kept: Mode,
        on_key: impl FnOnce(&mut Self, KeyEvent),
    ) -> Result<Flow> {
        match self.prefix.handle(key) {
            KeyAction::Command(c) => {
                self.mode = kept;
                self.command(c)
            }
            KeyAction::Forward(key) => {
                on_key(self, key);
                Ok(Flow::Continue)
            }
            KeyAction::None => {
                self.mode = kept;
                Ok(Flow::Continue)
            }
        }
    }

    /// A key while the focused pane has the keyboard.
    fn normal_key(&mut self, key: KeyEvent) -> Result<Flow> {
        match self.prefix.handle(key) {
            KeyAction::Forward(key) => {
                let Some(pane) = self.current_pane() else {
                    return Ok(Flow::Continue);
                };
                let bytes = self
                    .terminals
                    .get(&pane)
                    .and_then(|t| encode_key(&key, t.key_modes()));
                if let Some(bytes) = bytes {
                    self.input(&pane, &bytes)?;
                }
                Ok(Flow::Continue)
            }
            KeyAction::Command(c) => self.command(c),
            KeyAction::None => Ok(Flow::Continue),
        }
    }

    fn command(&mut self, command: Command) -> Result<Flow> {
        match command {
            Command::NewTab => self.report(|app| app.new_tab()),
            Command::NewSpace => self.start_naming(Naming::NewSpace),
            Command::NextTab => self.report(|app| app.cycle_tab(true)),
            Command::PreviousTab => self.report(|app| app.cycle_tab(false)),
            Command::GoToTab(i) => self.report(|app| app.select_tab(i)),
            Command::RenameTab => self.start_naming(Naming::Tab),
            Command::RenameSpace => self.start_naming(Naming::Space),
            Command::Close => {
                if let Some(pane) = self.current_pane() {
                    self.report(|app| app.close(&pane, None));
                }
            }
            Command::Fork => {
                if let Some(pane) = self.current_pane() {
                    self.report(|app| app.fork(&pane));
                }
            }
            Command::SplitColumns => self.report(|app| app.split(true)),
            Command::SplitRows => self.report(|app| app.split(false)),
            Command::Focus(dir) => self.report(|app| app.move_focus(dir)),
            Command::Equalise => self.report(|app| app.equalise()),
            Command::Help => self.mode = Mode::Help,
            Command::Detach => return Ok(Flow::Quit),
            Command::FocusSidebar => self.focus_sidebar(false),
            Command::FocusSpaces => self.focus_sidebar(true),
            Command::MoveTab(step) => self.report(|app| app.move_tab(step)),
            Command::LastPage => self.last_page(),
            Command::ResizeSidebar(step) => self.resize_sidebar(step),
            Command::ToggleSidebar => {
                self.sidebar_hidden = !self.sidebar_hidden;
                if self.sidebar_hidden && matches!(self.mode, Mode::Sidebar(_)) {
                    self.mode = Mode::Normal;
                }
            }
            Command::NextAttention => self.next_attention(),
            Command::Dashboard => self.toggle_dashboard(),
        }
        self.relayout_and_save();
        Ok(Flow::Continue)
    }

    /// Open the prompt for `target`, holding its current or default name.
    fn start_naming(&mut self, target: Naming) {
        let line = LineEdit::new(&self.name_of(target));
        self.mode = Mode::Name { target, line };
    }

    /// Run a command whose failure the user should see but that is not fatal.
    pub(super) fn report(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) {
        if let Err(e) = f(self) {
            self.notice = Some(format!("{e:#}"));
        }
    }

    /// A key while the prompt asks for a name: `Enter` acts on it, `Esc`
    /// closes the prompt, others edit the line.
    fn name_key(&mut self, target: Naming, mut line: LineEdit, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.report(|app| app.name(target, line.text()));
                self.relayout_and_save();
                return;
            }
            KeyCode::Esc => return,
            _ => {
                line.key(key);
            }
        }
        self.mode = Mode::Name { target, line };
    }

    pub fn paste(&mut self, text: &str) -> Result<()> {
        if let Mode::Dashboard(d) = &self.mode {
            let d = d.clone();
            return self.dashboard_paste(&d, text);
        }
        match &mut self.mode {
            Mode::Name { line, .. } => line.insert(text),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Sidebar(_) | Mode::ConfirmClose { .. } | Mode::Dashboard(_) => {}
            Mode::Normal => {
                let Some(pane) = self.current_pane() else {
                    return Ok(());
                };
                let bracketed = self
                    .terminals
                    .get(&pane)
                    .is_some_and(|t| t.emulator().modes().bracketed_paste);
                self.input(&pane, &encode_paste(text, bracketed))?;
            }
        }
        Ok(())
    }
}
