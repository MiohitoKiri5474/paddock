//! The dashboard (`C-a o`): every session and task row with its details,
//! and a live preview of the selected row's terminal that `I` types into.
//!
//! It reads the task tree through [`App::rows`] and acts on rows through the
//! same dismiss, close, fork and jump as the rest of the app.

use super::rows::{row_ids, Row, RowId};
use super::state::{App, Mode};
use super::watch::Watched;
use crate::input::{encode_key, encode_paste};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The open dashboard: the selected line, and whether keys go to its
/// terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dashboard {
    pub selected: Option<RowId>,
    /// Where the selected line was last seen, for when it goes away.
    pub index: usize,
    pub interactive: bool,
}

impl Dashboard {
    /// The same dashboard with a line of `ids` selected: the selected one
    /// wherever it moved, else (it went away, or none was) the one now at
    /// `index`, else the last; nothing without lines.
    pub fn anchored(&self, ids: &[RowId]) -> Self {
        let found = self
            .selected
            .as_ref()
            .and_then(|s| ids.iter().position(|id| id == s));
        let index = found.unwrap_or(self.index.min(ids.len().saturating_sub(1)));
        Self {
            selected: ids.get(index).cloned(),
            index,
            interactive: self.interactive,
        }
    }
}

impl App {
    /// The dashboard, if open (also while asking before a close from it).
    pub fn dashboard(&self) -> Option<&Dashboard> {
        match &self.mode {
            Mode::Dashboard(d) | Mode::ConfirmClose { back: Some(d), .. } => Some(d),
            _ => None,
        }
    }

    /// The session row of the dashboard's selected line (a task line counts
    /// as its session's), else the first.
    pub fn dashboard_row(&self, d: &Dashboard) -> Option<Row> {
        let rows = self.rows();
        let pane = d.selected.as_ref().map(|s| s.pane.as_str());
        let found = rows.iter().position(|r| Some(r.pane.as_str()) == pane);
        rows.into_iter().nth(found.unwrap_or(0))
    }

    /// Whether the dashboard shows terminal `session` in its preview.
    pub fn shows_in_preview(&self, session: &str) -> bool {
        self.dashboard()
            .and_then(|d| self.dashboard_row(d))
            .is_some_and(|r| !r.closed && r.terminal == session)
    }

    /// `C-a o`: open the dashboard on its first line, or close it.
    pub(super) fn toggle_dashboard(&mut self) {
        if matches!(self.mode, Mode::Dashboard(_)) {
            self.mode = Mode::Normal;
            return;
        }
        self.mode = Mode::Dashboard(Dashboard::default().anchored(&row_ids(&self.rows())));
    }

    /// Keep the dashboard's selection on a line that exists, so the line
    /// highlighted is the one keys act on.
    pub fn anchor_dashboard(&mut self) {
        let ids = row_ids(&self.rows());
        if let Mode::Dashboard(d) | Mode::ConfirmClose { back: Some(d), .. } = &mut self.mode {
            *d = d.anchored(&ids);
        }
    }

    /// Tasks to keep git fresh for: all of the dashboard's, with details,
    /// while it is open; else the sidebar's.
    pub fn watched_tasks(&self) -> Watched {
        if self.dashboard().is_none() {
            return Watched {
                tasks: self.visible_tasks(),
                details: false,
            };
        }
        Watched {
            tasks: self
                .rows()
                .into_iter()
                .flat_map(|r| r.tasks)
                .map(|t| (t.cwd, t.branch))
                .collect(),
            details: true,
        }
    }

    /// A key for the dashboard. Interactive: see [`Self::interactive_key`].
    /// Else `j`/`k` (or arrows) move, `Enter` jumps to the row's terminal,
    /// `D` dismisses the row, `x` closes and `f` forks its terminal, `I`
    /// types into it, `Esc` closes the dashboard. A closed terminal's row can
    /// only be dismissed.
    pub(super) fn dashboard_key(&mut self, d: Dashboard, key: KeyEvent) {
        // Act on the line highlighted, even if rows changed since the draw.
        let d = d.anchored(&row_ids(&self.rows()));
        if d.interactive {
            return self.interactive_key(d, key);
        }
        self.mode = match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.dashboard_move(d, 1),
            KeyCode::Char('k') | KeyCode::Up => self.dashboard_move(d, -1),
            KeyCode::Char('D') => self.dashboard_dismiss(d),
            KeyCode::Esc => Mode::Normal,
            code => self.dashboard_act(d, code),
        };
        self.report(|app| app.relayout());
    }

    /// Interactive: every key but `Esc` goes to the previewed terminal, and
    /// `Esc` only leaves. `Esc` and a key read together arrive as that key
    /// with Alt: no paddock key uses Alt, so any such key leaves too, and
    /// `Esc` never reaches the terminal.
    fn interactive_key(&mut self, d: Dashboard, key: KeyEvent) {
        if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::ALT) {
            self.mode = Mode::Dashboard(Dashboard {
                interactive: false,
                ..d
            });
            return;
        }
        let row = self.dashboard_row(&d).filter(|r| !r.closed);
        self.mode = Mode::Dashboard(d);
        if let Some(t) = row.and_then(|r| self.terminals.get_mut(&r.terminal)) {
            if let Some(bytes) = encode_key(&key, t.key_modes()) {
                let sent = t.input(&bytes);
                self.report(|_| sent);
            }
        }
    }

    /// The line `step` lines away from the selected one; at either end, the
    /// selected one.
    fn dashboard_move(&self, d: Dashboard, step: isize) -> Mode {
        let moved = Dashboard {
            selected: None,
            index: d.index.saturating_add_signed(step),
            ..d
        };
        Mode::Dashboard(moved.anchored(&row_ids(&self.rows())))
    }

    /// Dismiss the selected line, then select the line that took its place,
    /// else the one above.
    fn dashboard_dismiss(&mut self, d: Dashboard) -> Mode {
        if let Some(id) = &d.selected {
            self.report(|app| app.dismiss(id));
        }
        Mode::Dashboard(d.anchored(&row_ids(&self.rows())))
    }

    /// `Enter`, `x`, `f` and `I` on the selected row's terminal, unless it
    /// was closed.
    fn dashboard_act(&mut self, d: Dashboard, code: KeyCode) -> Mode {
        let Some(row) = self.dashboard_row(&d).filter(|r| !r.closed) else {
            return Mode::Dashboard(d);
        };
        match code {
            KeyCode::Enter => {
                self.jump_to(row.tab, &row.terminal);
                Mode::Normal
            }
            KeyCode::Char('x') => {
                self.mode = Mode::Dashboard(d.clone());
                self.report(|app| app.close(&row.terminal, Some(d)));
                // Asking takes the keyboard, then comes back here.
                std::mem::replace(&mut self.mode, Mode::Normal)
            }
            KeyCode::Char('f') => {
                // The fork's tab gets the keyboard.
                self.report(|app| app.fork(&row.terminal));
                Mode::Normal
            }
            KeyCode::Char('I') => Mode::Dashboard(Dashboard {
                interactive: true,
                ..d
            }),
            _ => Mode::Dashboard(d),
        }
    }

    /// Pasted text goes to the previewed terminal while interactive, and is
    /// dropped otherwise.
    pub(super) fn dashboard_paste(&mut self, d: &Dashboard, text: &str) -> Result<()> {
        if !d.interactive {
            return Ok(());
        }
        let Some(row) = self.dashboard_row(d).filter(|r| !r.closed) else {
            return Ok(());
        };
        match self.terminals.get_mut(&row.terminal) {
            Some(t) => {
                let bytes = encode_paste(text, t.emulator().modes().bracketed_paste);
                t.input(&bytes)
            }
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(pane: &str, branch: Option<&str>) -> RowId {
        RowId {
            pane: pane.into(),
            branch: branch.map(str::to_string),
        }
    }

    fn at(selected: Option<RowId>, index: usize) -> Dashboard {
        Dashboard {
            selected,
            index,
            interactive: true,
        }
    }

    #[test]
    fn a_selection_whose_line_went_away_moves_to_the_nearest_line() {
        let ids = [id("%1", None), id("%1", Some("a")), id("%2", None)];
        let gone = id("%9", None);
        let anchored = |d: Dashboard| d.anchored(&ids);
        assert_eq!(
            anchored(at(Some(ids[2].clone()), 0)),
            at(Some(ids[2].clone()), 2),
            "still there: followed where it moved"
        );
        assert_eq!(
            anchored(at(Some(gone.clone()), 1)),
            at(Some(ids[1].clone()), 1),
            "gone: the line now in its place"
        );
        assert_eq!(
            anchored(at(Some(gone.clone()), 7)),
            at(Some(ids[2].clone()), 2),
            "gone from the end: the last line"
        );
        assert_eq!(anchored(at(None, 0)), at(Some(ids[0].clone()), 0));
        assert_eq!(at(Some(gone), 1).anchored(&[]), at(None, 0), "no lines");
    }
}
