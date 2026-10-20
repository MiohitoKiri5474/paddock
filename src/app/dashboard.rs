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
use crossterm::event::{KeyCode, KeyEvent};

/// The open dashboard: the selected line, and whether keys go to its
/// terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dashboard {
    pub selected: Option<RowId>,
    pub interactive: bool,
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
    pub fn previews(&self, session: &str) -> bool {
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
        match row_ids(&self.rows()).into_iter().next() {
            Some(first) => {
                self.mode = Mode::Dashboard(Dashboard {
                    selected: Some(first),
                    interactive: false,
                });
            }
            None => self.notice = Some("no Claude sessions yet".into()),
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

    /// A key for the dashboard. Interactive: every key but `Esc` goes to the
    /// previewed terminal, and `Esc` only leaves. Else `j`/`k` (or arrows)
    /// move, `Enter` jumps to the row's terminal, `D` dismisses the row, `x`
    /// closes and `f` forks its terminal, `I` types into it, `Esc` closes the
    /// dashboard. A closed terminal's row can only be dismissed.
    pub(super) fn dashboard_key(&mut self, d: Dashboard, key: KeyEvent) {
        let row = self.dashboard_row(&d).filter(|r| !r.closed);
        if d.interactive {
            if key.code == KeyCode::Esc {
                self.mode = Mode::Dashboard(Dashboard {
                    interactive: false,
                    ..d
                });
                return;
            }
            self.mode = Mode::Dashboard(d);
            if let Some(t) = row.and_then(|r| self.terminals.get_mut(&r.terminal)) {
                if let Some(bytes) = encode_key(&key, t.key_modes()) {
                    let sent = t.input(&bytes);
                    self.report(|_| sent);
                }
            }
            return;
        }
        let ids = row_ids(&self.rows());
        let at = ids
            .iter()
            .position(|id| Some(id) == d.selected.as_ref())
            .unwrap_or(0);
        let pick = |i: usize| ids.get(i).cloned();
        let keep = |selected| {
            Mode::Dashboard(Dashboard {
                selected,
                interactive: false,
            })
        };
        self.mode = match (key.code, row) {
            (KeyCode::Char('j') | KeyCode::Down, _) => keep(pick(at + 1).or(d.selected)),
            (KeyCode::Char('k') | KeyCode::Up, _) => {
                keep(at.checked_sub(1).and_then(pick).or(d.selected))
            }
            (KeyCode::Char('D'), _) => {
                if let Some(id) = ids.get(at) {
                    self.report(|app| app.dismiss(id));
                }
                // The line that took its place, else the one above.
                let ids = row_ids(&self.rows());
                match ids.get(at.min(ids.len().saturating_sub(1))) {
                    Some(next) => keep(Some(next.clone())),
                    None => Mode::Normal,
                }
            }
            (KeyCode::Enter, Some(row)) => {
                self.jump_to(row.tab, &row.terminal);
                Mode::Normal
            }
            (KeyCode::Char('x'), Some(row)) => {
                self.mode = Mode::Dashboard(d.clone());
                self.report(|app| app.close(&row.terminal));
                // Asking takes the keyboard, then comes back here.
                match std::mem::replace(&mut self.mode, Mode::Normal) {
                    Mode::ConfirmClose {
                        terminal, state, ..
                    } => Mode::ConfirmClose {
                        terminal,
                        state,
                        back: Some(d),
                    },
                    mode => mode,
                }
            }
            (KeyCode::Char('f'), Some(row)) => {
                // The fork's tab gets the keyboard.
                self.report(|app| app.fork(&row.terminal));
                Mode::Normal
            }
            (KeyCode::Char('I'), Some(_)) => Mode::Dashboard(Dashboard {
                interactive: true,
                ..d
            }),
            (KeyCode::Esc, _) => Mode::Normal,
            _ => Mode::Dashboard(d),
        };
        self.report(|app| app.relayout());
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
