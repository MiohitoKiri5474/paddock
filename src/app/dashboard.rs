//! The dashboard (`C-a o`): every session and task row with its details,
//! and a live preview of the selected row's terminal that `I` types into.
//!
//! It reads the task tree through [`App::rows`] and acts on rows through the
//! same dismiss, close, fork and jump as the rest of the app.

use super::rows::{row_ids, Row, RowId};
use super::state::{App, Mode, Panel};
use super::watch::Watched;
use crate::input::keymap::DashboardAction;
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
            Mode::Dashboard(d)
            | Mode::ConfirmClose { back: Some(d), .. }
            | Mode::Naming {
                back: Some(Panel::Dashboard(d)),
                ..
            } => Some(d),
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

    /// Whether the dashboard shows pane `pane` in its preview.
    pub fn shows_in_preview(&self, pane: &str) -> bool {
        self.dashboard()
            .and_then(|d| self.dashboard_row(d))
            .is_some_and(|r| !r.closed && r.pane == pane)
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
        if let Mode::Dashboard(d)
        | Mode::ConfirmClose { back: Some(d), .. }
        | Mode::Naming {
            back: Some(Panel::Dashboard(d)),
            ..
        } = &mut self.mode
        {
            *d = d.anchored(&ids);
        }
    }

    /// Tasks to keep git fresh for: all of the dashboard's, with details,
    /// while it is open; else the sidebar's. The spaces' directories and
    /// branches are always read.
    pub fn watched_tasks(&self) -> Watched {
        let spaces = self.spaces.iter().map(|i| i.id.clone()).collect();
        if self.dashboard().is_none() {
            return Watched {
                tasks: self.visible_tasks(),
                details: false,
                spaces,
            };
        }
        Watched {
            spaces,
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
    /// Else `[keys.dashboard]`, by default: `j`/`k` (or arrows) move,
    /// `Enter` jumps to the row's terminal, `D` dismisses the row, `x`
    /// closes and `f` forks its terminal, `I` types into it, `Esc` closes
    /// the dashboard. A closed terminal's row can only be dismissed.
    pub(super) fn dashboard_key(&mut self, d: Dashboard, key: KeyEvent) {
        // Act on the line highlighted, even if rows changed since the draw.
        let d = d.anchored(&row_ids(&self.rows()));
        if d.interactive {
            return self.interactive_key(d, key);
        }
        self.mode = match self.keymap.dashboard.action(&key) {
            Some(DashboardAction::Down) => self.dashboard_move(d, 1),
            Some(DashboardAction::Up) => self.dashboard_move(d, -1),
            Some(DashboardAction::Dismiss) => self.dashboard_dismiss(d),
            Some(DashboardAction::Back) => Mode::Normal,
            Some(
                action @ (DashboardAction::Open
                | DashboardAction::Close
                | DashboardAction::Fork
                | DashboardAction::Interact),
            ) => self.dashboard_act(d, action),
            None => Mode::Dashboard(d),
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
        let Some(pane) = row.map(|r| r.pane) else {
            return;
        };
        let bytes = self
            .terminals
            .get(&pane)
            .and_then(|t| encode_key(&key, t.key_modes()));
        if let Some(bytes) = bytes {
            self.report(|app| app.input(&pane, &bytes));
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

    /// Open, close, fork and interact on the selected row's terminal, unless it
    /// was closed.
    fn dashboard_act(&mut self, d: Dashboard, action: DashboardAction) -> Mode {
        let Some(row) = self.dashboard_row(&d).filter(|r| !r.closed) else {
            return Mode::Dashboard(d);
        };
        match action {
            DashboardAction::Open => {
                self.jump_to(&row.pane);
                Mode::Normal
            }
            DashboardAction::Close => {
                self.mode = Mode::Dashboard(d.clone());
                self.report(|app| app.close(&row.pane, Some(d)));
                // Asking takes the keyboard, then comes back here.
                std::mem::replace(&mut self.mode, Mode::Normal)
            }
            DashboardAction::Fork => {
                // The fork's tab gets the keyboard.
                self.report(|app| app.fork(&row.pane));
                Mode::Normal
            }
            DashboardAction::Interact => Mode::Dashboard(Dashboard {
                interactive: true,
                ..d
            }),
            DashboardAction::Down
            | DashboardAction::Up
            | DashboardAction::Dismiss
            | DashboardAction::Back => Mode::Dashboard(d),
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
        let bracketed = self
            .terminals
            .get(&row.pane)
            .is_some_and(|t| t.emulator().modes().bracketed_paste);
        self.input(&row.pane, &encode_paste(text, bracketed))
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
