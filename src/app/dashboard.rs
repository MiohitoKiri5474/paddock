//! The dashboard (`C-a o`): every session with its task rows and their
//! details, and a live preview of the selected session's terminal that `I`
//! types into. Only sessions are selected; task rows go with theirs. `/`
//! types a filter: only blocks whose session name, branch or state
//! fuzzy-match it stay in the list.
//!
//! It reads the task tree through [`App::rows`] and acts on the selected
//! session through the same dismiss, close, fork and jump as the rest of
//! the app.

use super::prs::PrUpdate;
use super::rows::{pane_only, row_ids, Row, RowId, EXTERNAL_TYPE};
use super::state::{App, Mode, Panel};
use super::watch::Watched;
use super::Event;
use crate::fuzzy;
use crate::github;
use crate::input::keymap::DashboardAction;
use crate::input::line::LineEdit;
use crate::input::{encode_key, encode_paste};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::Sender;

/// Tells the event loop that `p`'s gh is done ([`Event::PrOpened`]) when
/// dropped: after the outcome is set, or while unwinding from a panic on
/// the open's thread, so `p` never stays blocked.
struct Opened {
    events: Sender<Event>,
    number: u64,
    result: Option<Result<(), String>>,
}

impl Opened {
    /// Done, with how it went: sent as it drops.
    fn finish(mut self, result: Result<(), String>) {
        self.result = Some(result);
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let result = self
            .result
            .take()
            .unwrap_or_else(|| Err("gh's thread failed".to_string()));
        let _ = self.events.send(Event::PrOpened(self.number, result));
    }
}

/// The dashboard's key line while its filter is typed: its keys are
/// fixed, so they are not read from the keymap.
const FILTER_KEYS: &str = "Enter keep  Esc clear";

/// The dashboard's filter: the text typed, and whether keys type into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardFilter {
    /// Only blocks matching it are listed.
    pub line: LineEdit,
    /// Keys type into the line.
    pub typing: bool,
}

impl DashboardFilter {
    /// Whether its line shows: while typing it, or while it holds text.
    pub fn shown(&self) -> bool {
        self.typing || !self.line.text().is_empty()
    }
}

/// The open dashboard: the selected line, whether keys go to its
/// terminal, and the filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dashboard {
    pub selected: Option<RowId>,
    /// Where the selected line was last seen, for when it goes away.
    pub index: usize,
    pub interactive: bool,
    pub filter: DashboardFilter,
}

impl Dashboard {
    /// The rows of `rows` whose session name, branch or state fuzzy-match
    /// the filter; all of them without one.
    pub fn filtered(&self, rows: Vec<Row>) -> Vec<Row> {
        let query = self.filter.line.text();
        if query.is_empty() {
            return rows;
        }
        rows.into_iter()
            .filter(|r| {
                let branches = r.tasks.iter().map(|t| t.branch.as_str());
                [r.label.as_str(), r.state.label()]
                    .into_iter()
                    .chain(branches)
                    .any(|f| fuzzy::score(query, f).is_some())
            })
            .collect()
    }

    /// The same dashboard with a line of `ids` selected: the selected one
    /// wherever it moved, else (it went away, or none was) the one now at
    /// `index`, else the last; nothing without lines. When the selected one
    /// went away (its session ended), keys stop going to a terminal, so they
    /// never reach the session that took its place.
    pub fn anchored(&self, ids: &[RowId]) -> Self {
        let found = self
            .selected
            .as_ref()
            .and_then(|s| ids.iter().position(|id| id == s));
        let gone = self.selected.is_some() && found.is_none();
        let index = found.unwrap_or(self.index.min(ids.len().saturating_sub(1)));
        Self {
            selected: ids.get(index).cloned(),
            index,
            interactive: self.interactive && !gone,
            ..self.clone()
        }
    }
}

impl App {
    /// The dashboard, if open (also while asking before a close from it).
    pub fn dashboard(&self) -> Option<&Dashboard> {
        match &self.mode {
            Mode::Dashboard(d) => Some(d),
            mode => match mode.back_panel() {
                Some(Panel::Dashboard(d)) => Some(d),
                _ => None,
            },
        }
    }

    /// The dashboard's key line: the filter's fixed keys while it is
    /// typed, else `[keys.dashboard]`.
    pub fn dashboard_keys(&self, d: &Dashboard) -> String {
        if d.filter.typing {
            FILTER_KEYS.to_string()
        } else {
            self.keymap.dashboard.hints().join("  ")
        }
    }

    /// The rows the dashboard lists: those matching its filter.
    pub fn dashboard_rows(&self, d: &Dashboard) -> Vec<Row> {
        d.filtered(self.rows())
    }

    /// The dashboard's selected session, else the first.
    pub fn dashboard_row(&self, d: &Dashboard) -> Option<Row> {
        let rows = self.dashboard_rows(d);
        let pane = d.selected.as_ref().map(|s| s.pane.as_str());
        let found = rows.iter().position(|r| Some(r.pane.as_str()) == pane);
        rows.into_iter().nth(found.unwrap_or(0))
    }

    /// Whether the dashboard shows pane `pane` in its preview.
    pub fn shows_in_preview(&self, pane: &str) -> bool {
        self.dashboard()
            .and_then(|d| self.dashboard_row(d))
            .is_some_and(|r| r.pane == pane)
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
        let Some(d) = self.dashboard() else {
            return;
        };
        let ids = row_ids(&self.dashboard_rows(d));
        let d = match &mut self.mode {
            Mode::Dashboard(d) => Some(d),
            mode => match mode.back_panel_mut() {
                Some(Panel::Dashboard(d)) => Some(d),
                _ => None,
            },
        };
        if let Some(d) = d {
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

    /// A key for the dashboard. Interactive: see [`Self::interactive_key`];
    /// typing the filter: see [`Self::dashboard_filter_key`]. Else
    /// `[keys.dashboard]`, by default: `j`/`k` (or arrows) move, `Enter`
    /// jumps to the selected session's terminal, `D` dismisses the
    /// session, `x` closes and `f` forks its terminal, `I` types into it,
    /// `/` types the filter, `Esc` closes the dashboard.
    pub(super) fn dashboard_key(&mut self, d: Dashboard, key: KeyEvent) {
        // Act on the line highlighted, even if rows changed since the draw.
        let d = d.anchored(&row_ids(&self.dashboard_rows(&d)));
        if d.interactive {
            return self.interactive_key(d, key);
        }
        if d.filter.typing {
            return self.dashboard_filter_key(d, key);
        }
        self.mode = match self.keymap.dashboard.action(&key) {
            Some(DashboardAction::Down) => self.dashboard_move(d, 1),
            Some(DashboardAction::Up) => self.dashboard_move(d, -1),
            Some(DashboardAction::Dismiss) => self.dashboard_dismiss(d),
            Some(DashboardAction::Back) => Mode::Normal,
            Some(DashboardAction::OpenPr) => {
                self.open_pr(&d);
                Mode::Dashboard(d)
            }
            Some(DashboardAction::Filter) => Mode::Dashboard(Dashboard {
                filter: DashboardFilter {
                    typing: true,
                    ..d.filter.clone()
                },
                ..d
            }),
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

    /// Typing the filter: `Enter` keeps it and returns to the list, `Esc`
    /// clears it and leaves the filter line, other keys edit it. The
    /// selection stays on a listed line.
    fn dashboard_filter_key(&mut self, mut d: Dashboard, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => d.filter.typing = false,
            KeyCode::Esc => d.filter = DashboardFilter::default(),
            _ => d.filter.line.key(key),
        }
        let d = d.anchored(&row_ids(&self.dashboard_rows(&d)));
        self.mode = Mode::Dashboard(d);
    }

    /// Interactive: every key but `Esc` goes to the previewed terminal, and
    /// `Esc` only leaves. `Esc` and a key read together arrive as that key
    /// with Alt: no key of the dashboard uses Alt (direct chords are off
    /// in it), so any such key leaves too, and `Esc` never reaches the
    /// terminal.
    fn interactive_key(&mut self, d: Dashboard, key: KeyEvent) {
        if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::ALT) {
            self.mode = Mode::Dashboard(Dashboard {
                interactive: false,
                ..d
            });
            return;
        }
        let row = self.dashboard_row(&d);
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
        Mode::Dashboard(moved.anchored(&row_ids(&self.dashboard_rows(&moved))))
    }

    /// Dismiss the selected session, then select the session that took its
    /// place, else the one above.
    fn dashboard_dismiss(&mut self, d: Dashboard) -> Mode {
        if let Some(id) = &d.selected {
            self.report(|app| app.dismiss(id));
        }
        Mode::Dashboard(d.anchored(&row_ids(&self.dashboard_rows(&d))))
    }

    /// Keep the pull requests just read (see [`super::prs`]); a task
    /// without one loses the one it had.
    pub(super) fn store_prs(&mut self, read: PrUpdate) {
        for (task, pr) in read {
            match pr {
                Some(pr) => self.task_prs.insert(task, pr),
                None => self.task_prs.remove(&task),
            };
        }
    }

    /// `p`: open the selected session's pull request in the browser
    /// (`gh pr view --web` in its directory): its main agent's branch's,
    /// else the first subagent branch's with one. Without one, a notice.
    /// An external session's too: it only opens a page. `gh` failing comes
    /// back as a notice. While one open runs, another `p` does nothing.
    fn open_pr(&mut self, d: &Dashboard) {
        let Some(row) = self.dashboard_row(d) else {
            return;
        };
        let main_first = row
            .tasks
            .iter()
            .filter(|t| t.subagent.is_none())
            .chain(row.tasks.iter().filter(|t| t.subagent.is_some()));
        let found = main_first
            .clone()
            .find_map(|t| Some((t, self.task_prs.get(&(t.cwd.clone(), t.branch.clone()))?)));
        let Some((task, pr)) = found else {
            self.notice = Some(match main_first.map(|t| &t.branch).next() {
                Some(branch) => format!("no pull request for {branch}"),
                None => "no pull request: no branch".to_string(),
            });
            return;
        };
        // One open at a time: a press while one runs is ignored.
        if self.pr_opening {
            return;
        }
        self.pr_opening = true;
        let opened = Opened {
            events: self.events.clone(),
            number: pr.number,
            result: None,
        };
        github::open_web(&task.cwd, pr.number, move |done| opened.finish(done));
    }

    /// `p`'s gh is done (see [`Opened`]): another `p` may open; a failure
    /// is a notice.
    pub(super) fn pr_opened(&mut self, number: u64, result: Result<(), String>) {
        self.pr_opening = false;
        if let Err(e) = result {
            self.notice = Some(format!("cannot open PR #{number}: {e}"));
        }
    }

    /// Open, close, fork and interact on the selected session's terminal.
    fn dashboard_act(&mut self, d: Dashboard, action: DashboardAction) -> Mode {
        let Some(row) = self.dashboard_row(&d) else {
            return Mode::Dashboard(d);
        };
        // An external session has no terminal in paddock: the core paths
        // refuse it with a notice, and the dashboard stays.
        if row.external {
            match action {
                DashboardAction::Open => self.jump_to(&row.pane),
                DashboardAction::Close => self.report(|app| app.close(&row.pane)),
                DashboardAction::Fork => self.report(|app| app.fork(&row.pane)),
                DashboardAction::Interact => {
                    self.report(|_| pane_only(&row.pane, EXTERNAL_TYPE));
                }
                _ => {}
            }
            return Mode::Dashboard(d);
        }
        match action {
            DashboardAction::Open => {
                self.jump_to(&row.pane);
                Mode::Normal
            }
            DashboardAction::Close => {
                self.mode = Mode::Dashboard(d);
                self.report(|app| app.close(&row.pane));
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
            | DashboardAction::Filter
            | DashboardAction::OpenPr
            | DashboardAction::Back => Mode::Dashboard(d),
        }
    }

    /// Pasted text goes to the previewed terminal while interactive, into
    /// the filter while typing it, and is dropped otherwise.
    pub(super) fn dashboard_paste(&mut self, d: &Dashboard, text: &str) -> Result<()> {
        if d.filter.typing {
            if let Mode::Dashboard(d) = &mut self.mode {
                d.filter.line.insert(text);
            }
            self.anchor_dashboard();
            return Ok(());
        }
        if !d.interactive {
            return Ok(());
        }
        let Some(row) = self.dashboard_row(d) else {
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
    use crate::app::rows::TaskRow;
    use crate::sessions::State;

    fn id(pane: &str) -> RowId {
        RowId { pane: pane.into() }
    }

    fn at(selected: Option<RowId>, index: usize) -> Dashboard {
        Dashboard {
            selected,
            index,
            interactive: true,
            ..Dashboard::default()
        }
    }

    #[test]
    fn the_filter_keeps_blocks_matching_a_name_branch_or_state() {
        let row = |label: &str, state: State, branch: &str| Row {
            pane: format!("%{label}"),
            space: "$1".into(),
            window: "@1".into(),
            label: label.into(),
            kind: crate::sessions::AgentKind::Claude,
            state,
            tasks: vec![TaskRow {
                branch: branch.into(),
                cwd: "/r".into(),
                status: None,
                subagent: None,
            }],
            external: false,
        };
        let rows = || {
            vec![
                row("api", State::Waiting, "feat/login"),
                row("web", State::Idle, "fix/css"),
            ]
        };
        let labels = |filter: &str| -> Vec<String> {
            let d = Dashboard {
                filter: DashboardFilter {
                    line: LineEdit::new(filter),
                    typing: false,
                },
                ..Dashboard::default()
            };
            d.filtered(rows()).into_iter().map(|r| r.label).collect()
        };
        assert_eq!(labels(""), ["api", "web"]);
        assert_eq!(labels("web"), ["web"]);
        assert_eq!(labels("login"), ["api"], "by branch");
        assert_eq!(labels("idle"), ["web"], "by state");
        assert_eq!(labels("zzz"), Vec::<String>::new());
        let typing = DashboardFilter {
            typing: true,
            ..DashboardFilter::default()
        };
        assert!(typing.shown());
        assert!(!DashboardFilter::default().shown());
        let kept = DashboardFilter {
            line: LineEdit::new("a"),
            typing: false,
        };
        assert!(kept.shown());
    }

    #[test]
    fn an_open_says_it_is_done_even_when_its_thread_panics() {
        let (tx, rx) = std::sync::mpsc::channel();
        let opened = |number| Opened {
            events: tx.clone(),
            number,
            result: None,
        };
        opened(1).finish(Ok(()));
        assert!(matches!(rx.try_recv(), Ok(Event::PrOpened(1, Ok(())))));
        let guard = opened(2);
        let panicked = std::thread::spawn(move || {
            let _guard = guard;
            panic!("gh's thread");
        })
        .join();
        assert!(panicked.is_err());
        assert!(matches!(rx.try_recv(), Ok(Event::PrOpened(2, Err(_)))));
    }

    #[test]
    fn a_selection_whose_line_went_away_moves_to_the_nearest_line() {
        let ids = [id("%1"), id("%2"), id("%3")];
        let gone = id("%9");
        let anchored = |d: Dashboard| d.anchored(&ids);
        assert_eq!(
            anchored(at(Some(ids[2].clone()), 0)),
            at(Some(ids[2].clone()), 2),
            "still there: followed where it moved"
        );
        // Keys stop going to a terminal: the one typed into went away.
        let typing_stops = |d: Dashboard| Dashboard {
            interactive: false,
            ..d
        };
        assert_eq!(
            anchored(at(Some(gone.clone()), 1)),
            typing_stops(at(Some(ids[1].clone()), 1)),
            "gone: the line now in its place"
        );
        assert_eq!(
            anchored(at(Some(gone.clone()), 7)),
            typing_stops(at(Some(ids[2].clone()), 2)),
            "gone from the end: the last line"
        );
        assert_eq!(anchored(at(None, 0)), at(Some(ids[0].clone()), 0));
        assert_eq!(
            at(Some(gone), 1).anchored(&[]),
            typing_stops(at(None, 0)),
            "no lines"
        );
    }
}
