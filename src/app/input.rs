//! Applying outer-terminal input to the app, depending on its mode.

use super::sidebar::SidebarTarget;
use super::state::{App, Mode, NameTarget, Panel, Renamed};
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
            Mode::Naming { target, line, back } => {
                self.naming_key(target, line, back, key);
                Ok(Flow::Continue)
            }
            // `y` closes; any other key keeps it, and is not passed on.
            Mode::ConfirmClose { target, back, .. } => {
                if key.code == KeyCode::Char('y') {
                    self.report(|app| app.close_now(&target));
                }
                self.mode = Mode::back_to(back);
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
            Command::NewSpace => self.start_naming(NameTarget::NewSpace),
            Command::NextTab => self.report(|app| app.cycle_tab(true)),
            Command::PreviousTab => self.report(|app| app.cycle_tab(false)),
            Command::GoToTab(i) => self.report(|app| app.select_tab(i)),
            Command::RenameTab => self.start_naming(NameTarget::Rename(Renamed::Tab)),
            Command::RenameSpace => self.start_naming(NameTarget::Rename(Renamed::Space)),
            Command::Close => {
                if let Some(pane) = self.current_pane() {
                    self.report(|app| app.close(&pane));
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
            Command::FocusSidebar => self.focus_sidebar(SidebarTarget::Here),
            Command::FocusSpaces => self.focus_sidebar(SidebarTarget::Spaces),
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
    /// A panel with the keyboard (the sidebar or the dashboard) gets it
    /// back when the prompt closes.
    fn start_naming(&mut self, target: NameTarget) {
        let line = LineEdit::new(&self.name_of(target));
        let back = std::mem::replace(&mut self.mode, Mode::Normal).into_panel();
        self.mode = Mode::Naming { target, line, back };
    }

    /// Run a command whose failure the user should see but that is not fatal.
    pub(super) fn report(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) {
        if let Err(e) = f(self) {
            self.notice = Some(format!("{e:#}"));
        }
    }

    /// A key while the prompt asks for a name: `Enter` acts on it, `Esc`
    /// closes the prompt, others edit the line. Closing gives the keyboard
    /// back to `back`.
    fn naming_key(
        &mut self,
        target: NameTarget,
        mut line: LineEdit,
        back: Option<Panel>,
        key: KeyEvent,
    ) {
        match key.code {
            KeyCode::Enter => {
                self.mode = Mode::back_to(back);
                self.report(|app| app.submit_name(target, line.text()));
                self.relayout_and_save();
            }
            KeyCode::Esc => self.mode = Mode::back_to(back),
            _ => {
                line.key(key);
                self.mode = Mode::Naming { target, line, back };
            }
        }
    }

    pub fn paste(&mut self, text: &str) -> Result<()> {
        if let Mode::Dashboard(d) = &self.mode {
            let d = d.clone();
            return self.dashboard_paste(&d, text);
        }
        match &mut self.mode {
            Mode::Naming { line, .. } => line.insert(text),
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
