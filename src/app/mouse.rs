//! The mouse: clicks focus a pane, open a space or agent, or switch
//! tabs; the
//! wheel and programs' mouse reports go to the pane under the pointer;
//! dragging selects text and releasing copies it. While a float is shown,
//! the mouse reaches only the float; a click outside it does nothing.

use super::state::{App, Mode, TAB_BAR_ROWS};
use crate::input::{route_mouse, MouseAction};
use crate::terminal::emulator::Emulator;
use crate::ui::sidebar;
use crate::ui::tab_bar::{self, Part};
use anyhow::{Context, Result};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::io::Write;
use std::process::{Command, Stdio};

/// Where a held button went when it was pressed. Its drags and its release
/// go the same way, even outside that pane.
pub enum Press {
    /// Reported to the program in this pane.
    Program(String),
    /// Selecting text in this pane; `dragged` once the pointer moved.
    Select { pane: String, dragged: bool },
    /// On the sidebar's border, pressed at screen row `row`; `dragged`
    /// once the pointer left the border's column (until then, a release is
    /// a click on that row).
    Border { row: u16, dragged: bool },
}

impl App {
    /// Returns whether the screen may have changed.
    pub fn mouse(&mut self, m: MouseEvent) -> Result<bool> {
        if self.mode.is_prompt() {
            return Ok(self.prompt_mouse(m));
        }
        match self.mode {
            Mode::Help | Mode::Dashboard(_) => return Ok(false),
            Mode::Normal | Mode::Sidebar(_) => {}
            Mode::Naming { .. }
            | Mode::ConfirmClose { .. }
            | Mode::OpenWorktree { .. }
            | Mode::ConfirmDelete { .. } => unreachable!("the prompt took it"),
        }
        match m.kind {
            MouseEventKind::Down(_) => self.press(m),
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => self.held(m),
            _ => self.hover(m),
        }
    }

    /// The mouse while the prompt popup is open: a press outside it closes
    /// it, as `Esc` would, giving the keyboard back to the panel it was
    /// asked from; nothing else does anything.
    fn prompt_mouse(&mut self, m: MouseEvent) -> bool {
        let inside = self.prompt_area().contains(Position::new(m.column, m.row));
        if inside || !matches!(m.kind, MouseEventKind::Down(_)) {
            return false;
        }
        self.mode = match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Naming { back, .. }
            | Mode::ConfirmClose { back, .. }
            | Mode::OpenWorktree { back, .. }
            | Mode::ConfirmDelete { back, .. } => Mode::back_to(back),
            _ => Mode::Normal,
        };
        true
    }

    fn press(&mut self, m: MouseEvent) -> Result<bool> {
        if let Some(Press::Select { pane, .. }) = self.press.take() {
            if let Some(t) = self.terminals.get_mut(&pane) {
                t.emulator_mut().clear_selection();
            }
        }
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        let hit = if self.float_pane().is_some() {
            // While a float is shown, only presses inside it do anything.
            // Its pane is never focused in tmux: that would make its
            // window the session's current one.
            self.under(m.column, m.row)
        } else {
            if let Some(changed) = self.press_outside_panes(m, left) {
                return Ok(changed);
            }
            let hit = self.under(m.column, m.row);
            if let (true, Some((pane, _))) = (left, &hit) {
                self.focus_pane(pane);
            }
            hit
        };
        let Some((pane, rect)) = hit else {
            return Ok(false);
        };
        let Some(t) = self.terminals.get_mut(&pane) else {
            return Ok(false);
        };
        let (col, row) = (m.column - rect.x, m.row - rect.y);
        match route_mouse(&m, col, row, t.emulator().modes()) {
            MouseAction::Report(bytes) => {
                self.input(&pane, &bytes)?;
                self.press = Some(Press::Program(pane));
            }
            MouseAction::Select => {
                t.emulator_mut().select(row, col);
                self.press = Some(Press::Select {
                    pane,
                    dragged: false,
                });
            }
            MouseAction::Scroll(_) | MouseAction::Ignore => {}
        }
        Ok(!left || self.settle())
    }

    /// A press on the sidebar's border, the tab bar or the sidebar: whether
    /// the screen may have changed, or `None` if it was over the panes.
    fn press_outside_panes(&mut self, m: MouseEvent, left: bool) -> Option<bool> {
        let side = self.sidebar_width();
        if left && side > 0 && m.column == side - 1 {
            // The border: dragging it resizes the sidebar.
            self.press = Some(Press::Border {
                row: m.row,
                dragged: false,
            });
            return Some(false);
        }
        if m.row < TAB_BAR_ROWS && m.column >= side {
            return Some(left && self.press_tab_bar(m.column - side));
        }
        if m.column < side {
            return Some(left && self.press_sidebar(m.row));
        }
        None
    }

    /// A left click at column `x` of the tab bar: a tab, `<`, `>` or `+`.
    /// Returns whether it hit one.
    fn press_tab_bar(&mut self, x: u16) -> bool {
        match tab_bar::part_at(&self.tab_layout(), x) {
            Some(Part::Tab(i)) => self.report(|app| app.select_tab(i)),
            Some(Part::Left(true)) => self.tab_first = self.tab_first.saturating_sub(1),
            Some(Part::Right(true)) => self.tab_first += 1,
            Some(Part::Plus) => self.report(|app| app.new_tab()),
            _ => return false,
        }
        self.settle()
    }

    /// A left click on screen row `y` of the sidebar: opens the space or
    /// agent there. Returns whether it hit one.
    fn press_sidebar(&mut self, y: u16) -> bool {
        let area = Rect::new(0, 0, self.sidebar_width(), self.screen.1);
        let picks = self.sidebar_picks();
        let entries = self.sidebar_spaces(&self.rows());
        let agents = picks.len() - self.spaces.len();
        let Some(pick) = sidebar::pick_at(&entries, agents, area, y).and_then(|i| picks.get(i))
        else {
            return false;
        };
        self.open(pick);
        self.settle()
    }

    /// Focus pane `pane` of the current tab, here at once and in tmux.
    fn focus_pane(&mut self, pane: &str) {
        let Some(window) = self
            .current_space_mut()
            .and_then(|i| i.current_window_mut())
        else {
            return;
        };
        window.active = pane.to_string();
        let id = window.id.clone();
        self.report(|app| app.server.select(&id, Some(pane)));
    }

    /// A drag or release: it goes where its press went, clamped to that pane.
    fn held(&mut self, m: MouseEvent) -> Result<bool> {
        let released = matches!(m.kind, MouseEventKind::Up(_));
        let pane = match &self.press {
            Some(Press::Program(p) | Press::Select { pane: p, .. }) => p.clone(),
            Some(Press::Border { row, dragged }) => {
                let (row, dragged) = (*row, *dragged);
                return Ok(self.border_held(m, row, dragged));
            }
            None => return Ok(false),
        };
        let (Some(rect), true) = (self.rect_of(&pane), self.terminals.contains_key(&pane)) else {
            self.press = None;
            return Ok(false);
        };
        let Some((col, row)) = clamp_into(rect, m.column, m.row) else {
            self.press = None;
            return Ok(false);
        };
        let (mut copied, mut report) = (None, None);
        if let Some(t) = self.terminals.get_mut(&pane) {
            match &mut self.press {
                Some(Press::Program(_)) => {
                    if let MouseAction::Report(bytes) =
                        route_mouse(&m, col, row, t.emulator().modes())
                    {
                        report = Some(bytes);
                    }
                }
                Some(Press::Select { dragged, .. }) => {
                    copied = select_held(t.emulator_mut(), dragged, &m, (row, col));
                }
                Some(Press::Border { .. }) | None => {}
            }
        }
        if let Some(bytes) = report {
            self.input(&pane, &bytes)?;
        }
        if let Some(text) = copied {
            self.report(|_| copy(&text));
        }
        if released {
            self.press = None;
        }
        Ok(true)
    }

    /// The button held on the sidebar's border (pressed on screen row
    /// `row`) moved or let go: dragging resizes the sidebar live and saves
    /// the width on release; a release without a drag is a click on `row`.
    fn border_held(&mut self, m: MouseEvent, row: u16, dragged: bool) -> bool {
        let released = matches!(m.kind, MouseEventKind::Up(_));
        let border = self.sidebar_width().saturating_sub(1);
        // Motion along the border itself resizes nothing.
        let dragged = dragged || m.column != border;
        if m.column != border {
            self.drag_border(m.column);
            self.report(|app| app.relayout());
        }
        if !released {
            self.press = Some(Press::Border { row, dragged });
            return true;
        }
        self.press = None;
        if dragged {
            self.report(|app| app.save());
            return true;
        }
        self.press_sidebar(row)
    }

    /// The wheel, and motion without a button: over the tab bar, the wheel
    /// goes to the previous or next tab; else to the pane under the pointer.
    fn hover(&mut self, m: MouseEvent) -> Result<bool> {
        // While a float is shown, the wheel only reaches it.
        let over_tab_bar = m.row < TAB_BAR_ROWS && m.column >= self.sidebar_width();
        if over_tab_bar && self.float_pane().is_none() {
            let step: isize = match m.kind {
                MouseEventKind::ScrollUp => -1,
                MouseEventKind::ScrollDown => 1,
                _ => return Ok(false),
            };
            let Some(space) = self.current_space() else {
                return Ok(false);
            };
            if let Some(next) = space.structure.current.checked_add_signed(step) {
                self.report(|app| app.select_tab(next));
            }
            return Ok(true);
        }
        let Some((pane, rect)) = self.under(m.column, m.row) else {
            return Ok(false);
        };
        let Some(t) = self.terminals.get_mut(&pane) else {
            return Ok(false);
        };
        match route_mouse(&m, m.column - rect.x, m.row - rect.y, t.emulator().modes()) {
            MouseAction::Report(bytes) => self.input(&pane, &bytes)?,
            MouseAction::Scroll(lines) => t.emulator_mut().scroll(lines),
            MouseAction::Select | MouseAction::Ignore => return Ok(false),
        }
        Ok(true)
    }

    /// The panes the mouse reaches, and where they are drawn: the float
    /// shown alone, else the current tab's.
    fn mouse_rects(&self) -> Vec<(String, Rect)> {
        match self.float_rect() {
            Some(float) => vec![float],
            None => self.pane_rects(),
        }
    }

    /// The pane at a screen position, and where it is drawn.
    fn under(&self, col: u16, row: u16) -> Option<(String, Rect)> {
        self.mouse_rects()
            .into_iter()
            .find(|(_, r)| r.contains(Position::new(col, row)))
    }

    /// Where pane `pane` is drawn, if the mouse reaches it.
    fn rect_of(&self, pane: &str) -> Option<Rect> {
        self.mouse_rects()
            .into_iter()
            .find(|(p, r)| p == pane && !r.is_empty())
            .map(|(_, r)| r)
    }

    /// After a click picked something: the keyboard goes back to the panes,
    /// and sizes and the saved state are updated as after a command.
    fn settle(&mut self) -> bool {
        self.mode = Mode::Normal;
        self.relayout_and_save();
        true
    }
}

/// Screen position `(col, row)` clamped into `rect`, as a position inside
/// it; `None` if `rect` is empty (a pane clipped away).
fn clamp_into(rect: Rect, col: u16, row: u16) -> Option<(u16, u16)> {
    if rect.is_empty() {
        return None;
    }
    let col = col.clamp(rect.x, rect.right() - 1) - rect.x;
    let row = row.clamp(rect.y, rect.bottom() - 1) - rect.y;
    Some((col, row))
}

/// A drag or release of a selection in `emu`, at `(row, col)` of the pane:
/// the selection follows the pointer; on release it ends, and its text is
/// returned if the pointer moved.
fn select_held(
    emu: &mut Emulator,
    dragged: &mut bool,
    m: &MouseEvent,
    (row, col): (u16, u16),
) -> Option<String> {
    if matches!(m.kind, MouseEventKind::Drag(_)) {
        *dragged = true;
        emu.select_to(row, col);
    }
    if !matches!(m.kind, MouseEventKind::Up(_)) {
        return None;
    }
    if *dragged {
        emu.select_to(row, col);
    }
    let text = dragged.then(|| emu.selected_text()).flatten();
    emu.clear_selection();
    text
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_clamped_into_a_pane_and_an_empty_one_has_none() {
        let rect = Rect::new(10, 2, 5, 3);
        assert_eq!(clamp_into(rect, 0, 0), Some((0, 0)));
        assert_eq!(clamp_into(rect, 12, 3), Some((2, 1)));
        assert_eq!(clamp_into(rect, 99, 99), Some((4, 2)));
        assert_eq!(clamp_into(Rect::new(10, 2, 0, 3), 12, 3), None);
        assert_eq!(clamp_into(Rect::new(10, 2, 5, 0), 12, 3), None);
    }
}
