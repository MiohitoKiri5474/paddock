//! The mouse: clicks focus a terminal, jump to a sidebar row or switch tabs;
//! the wheel and programs' mouse reports go to the terminal under the
//! pointer; dragging selects text and releasing copies it.

use super::state::{App, Mode, TAB_BAR_ROWS};
use crate::input::{route_mouse, MouseAction};
use crate::ui::{sidebar, tab_bar};
use anyhow::{Context, Result};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::io::Write;
use std::process::{Command, Stdio};

/// Where a held button went when it was pressed. Its drags and its release
/// go the same way, even outside that terminal.
pub enum Press {
    /// Reported to the program in this terminal.
    Program(String),
    /// Selecting text in this terminal; `dragged` once the pointer moved.
    Select { terminal: String, dragged: bool },
}

impl App {
    /// Returns whether the screen may have changed.
    pub fn mouse(&mut self, m: MouseEvent) -> Result<bool> {
        if matches!(
            self.mode,
            Mode::Help | Mode::Rename(_) | Mode::ConfirmClose { .. } | Mode::Dashboard(_)
        ) {
            return Ok(false);
        }
        match m.kind {
            MouseEventKind::Down(_) => self.press(m),
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => self.held(m),
            _ => self.hover(m),
        }
    }

    fn press(&mut self, m: MouseEvent) -> Result<bool> {
        if let Some(Press::Select { terminal, .. }) = self.press.take() {
            if let Some(t) = self.terminals.get_mut(&terminal) {
                t.emulator_mut().clear_selection();
            }
        }
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        let side = self.sidebar_width();
        if m.row < TAB_BAR_ROWS && m.column >= side {
            let names: Vec<&str> = self.tabs.iter().map(|t| t.name.as_str()).collect();
            let Some(tab) = tab_bar::tab_at(&names, m.column - side).filter(|_| left) else {
                return Ok(false);
            };
            self.tabs.select(tab);
            return Ok(self.settle());
        }
        if m.column < side {
            let area = Rect::new(0, 0, side, self.screen.1);
            let row = {
                let rows = self.rows();
                sidebar::row_at(&rows, area, m.row).and_then(|i| rows.get(i).cloned())
            };
            let Some(row) = row.filter(|r| left && !r.closed) else {
                return Ok(false);
            };
            self.jump_to(row.tab, &row.terminal);
            return Ok(self.settle());
        }
        let Some((id, rect)) = self.under(m.column, m.row) else {
            return Ok(false);
        };
        if left {
            if let Some(tab) = self.tabs.current_mut() {
                tab.content.focus = id.clone();
            }
        }
        let Some(t) = self.terminals.get_mut(&id) else {
            return Ok(false);
        };
        let (col, row) = (m.column - rect.x, m.row - rect.y);
        match route_mouse(&m, col, row, t.emulator().modes()) {
            MouseAction::Report(bytes) => {
                t.input(&bytes)?;
                self.press = Some(Press::Program(id));
            }
            MouseAction::Select => {
                t.emulator_mut().select(row, col);
                self.press = Some(Press::Select {
                    terminal: id,
                    dragged: false,
                });
            }
            MouseAction::Scroll(_) | MouseAction::Ignore => {}
        }
        Ok(!left || self.settle())
    }

    /// A drag or release: it goes where its press went, clamped to that terminal.
    fn held(&mut self, m: MouseEvent) -> Result<bool> {
        let id = match &self.press {
            Some(Press::Program(id) | Press::Select { terminal: id, .. }) => id.clone(),
            None => return Ok(false),
        };
        let released = matches!(m.kind, MouseEventKind::Up(_));
        let rect = self.rect_of(&id);
        let (Some(rect), Some(t)) = (rect, self.terminals.get_mut(&id)) else {
            self.press = None;
            return Ok(false);
        };
        let col = m.column.clamp(rect.x, rect.right() - 1) - rect.x;
        let row = m.row.clamp(rect.y, rect.bottom() - 1) - rect.y;
        match &mut self.press {
            Some(Press::Program(_)) => {
                if let MouseAction::Report(bytes) = route_mouse(&m, col, row, t.emulator().modes())
                {
                    t.input(&bytes)?;
                }
            }
            Some(Press::Select { dragged, .. }) => {
                if matches!(m.kind, MouseEventKind::Drag(_)) {
                    *dragged = true;
                    t.emulator_mut().select_to(row, col);
                }
                if released {
                    if *dragged {
                        t.emulator_mut().select_to(row, col);
                    }
                    let text = dragged.then(|| t.emulator().selected_text()).flatten();
                    t.emulator_mut().clear_selection();
                    if let Some(text) = text {
                        self.report(|_| copy(&text));
                    }
                }
            }
            None => {}
        }
        if released {
            self.press = None;
        }
        Ok(true)
    }

    /// The wheel, and motion without a button: to the terminal under the pointer.
    fn hover(&mut self, m: MouseEvent) -> Result<bool> {
        let Some((id, rect)) = self.under(m.column, m.row) else {
            return Ok(false);
        };
        let Some(t) = self.terminals.get_mut(&id) else {
            return Ok(false);
        };
        match route_mouse(&m, m.column - rect.x, m.row - rect.y, t.emulator().modes()) {
            MouseAction::Report(bytes) => t.input(&bytes)?,
            MouseAction::Scroll(lines) => t.emulator_mut().scroll(lines),
            MouseAction::Select | MouseAction::Ignore => return Ok(false),
        }
        Ok(true)
    }

    /// The current tab's terminal at a screen position, and where it is drawn.
    fn under(&self, col: u16, row: u16) -> Option<(String, Rect)> {
        let (rects, _) = self.tabs.current()?.content.rects(self.area());
        rects
            .into_iter()
            .find(|(_, r)| r.contains(Position::new(col, row)))
    }

    /// Where terminal `id` of the current tab is drawn.
    fn rect_of(&self, id: &str) -> Option<Rect> {
        let (rects, _) = self.tabs.current()?.content.rects(self.area());
        rects
            .into_iter()
            .find(|(t, r)| t == id && !r.is_empty())
            .map(|(_, r)| r)
    }

    /// After a click moved the focus: the keyboard goes back to the
    /// terminals, and the layout is resized and saved as after a command.
    fn settle(&mut self) -> bool {
        self.mode = Mode::Normal;
        self.report(|app| app.relayout());
        self.report(|app| app.save());
        true
    }
}

/// Put `text` on the clipboard with `pbcopy`, or with the program named by
/// `PADDOCK_COPY_COMMAND` (tests use it to keep off the real clipboard).
fn copy(text: &str) -> Result<()> {
    let program = std::env::var_os("PADDOCK_COPY_COMMAND").unwrap_or_else(|| "pbcopy".into());
    let name = program.to_string_lossy().into_owned();
    let mut child = Command::new(&program)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot copy: cannot run {name}"))?;
    let written = child
        .stdin
        .take()
        .context("cannot copy: no stdin")?
        .write_all(text.as_bytes());
    let status = child.wait()?;
    written.with_context(|| format!("cannot copy: writing to {name} failed"))?;
    anyhow::ensure!(status.success(), "cannot copy: {name} failed");
    Ok(())
}
