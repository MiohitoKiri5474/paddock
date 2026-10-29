//! What the screen shows of the app, ready to draw: the sidebar's spaces and
//! agents, the current space's tabs, and where the current tab's panes go.

use super::rows::{most_urgent, Row};
use super::space::Space;
use super::state::{App, CloseTarget, Mode};
use super::worktrees::SpaceEntry;
use crate::backend::{self, TerminalId};
use crate::paths::HomePaths;
use crate::sessions::State;
use crate::ui::prompt::{self, Body, Prompt};
use crate::ui::sidebar::{AgentLine, Entry, SpaceLine};
use crate::ui::tab_bar::{self, Tab, TabBar};
use ratatui::layout::Rect;
use std::borrow::Cow;

impl App {
    /// SPACES as drawn: each space (its name, the most urgent state of its
    /// agents, whether it is current, and its directory and branch once
    /// read) in sidebar order, worktree spaces under their repository, with
    /// a header for a repository whose main checkout has no space.
    pub(super) fn sidebar_spaces(&self, rows: &[Row]) -> Vec<Entry> {
        let lines = self.space_lines(rows);
        self.space_entries()
            .into_iter()
            .map(|entry| match entry {
                SpaceEntry::RepoHeader(name) => Entry::RepoHeader(name),
                SpaceEntry::Space { index, linked } => Entry::Space(SpaceLine {
                    linked,
                    ..lines[index].clone()
                }),
            })
            .collect()
    }

    /// Each space as the sidebar draws it, in the spaces' own order (not
    /// grouped): its name, the most urgent state of its agents, whether it
    /// is current, and its directory and branch once read.
    pub(super) fn space_lines(&self, rows: &[Row]) -> Vec<SpaceLine> {
        let home = HomePaths::new();
        self.spaces
            .iter()
            .enumerate()
            .map(|(i, space)| {
                let place = self.places.get(&space.id);
                // The PR read for this directory and branch (the PR
                // thread is fed the same key, see `App::prs_wanted`).
                let pr = place.and_then(|p| self.task_prs.get(&p.pr_key()?)).cloned();
                SpaceLine {
                    name: space.name().to_string(),
                    state: most_urgent(rows, |r| r.space == space.id),
                    current: i == self.current,
                    path: place.map(|p| home.show(&p.path)),
                    branch: place.and_then(|p| p.branch.clone()),
                    status: place.and_then(|p| p.status),
                    pr,
                    linked: false,
                }
            })
            .collect()
    }

    /// Each agent (every row: a main agent session that has not ended, in
    /// any space): its label, state, its space (none for an external
    /// session), and whether it is in the focused terminal.
    pub(super) fn sidebar_agents(&self, rows: &[Row]) -> Vec<AgentLine> {
        rows.iter()
            .map(|r| AgentLine {
                label: r.label.clone(),
                state: r.state,
                space: if r.external {
                    None
                } else {
                    self.row_space(r).map(|s| s.name().to_string())
                },
                current: self.is_current_row(&r.pane),
                external: r.external,
            })
            .collect()
    }

    /// The space row `r` runs in.
    fn row_space(&self, r: &Row) -> Option<&Space> {
        self.spaces.iter().find(|i| i.id == r.space)
    }

    /// Where row `r` runs: its space and tab, as `space › tab`.
    pub(super) fn place_of(&self, r: &Row) -> Option<String> {
        let space = self.row_space(r)?;
        let tab = space.tabs().iter().find(|t| t.id == r.tab)?;
        Some(format!("{} › {}", space.name(), self.tab_name(tab)))
    }

    /// A tab's name as shown: the backend's, except that a tab the backend
    /// names (not renamed by the user) whose focused terminal runs an agent
    /// is called after the agent's kind (`claude`), whatever process name
    /// the backend sees.
    pub(super) fn tab_name(&self, tab: &backend::Tab) -> String {
        let agent = self.session_in(tab.active.as_str()).filter(|s| {
            matches!(
                s.state,
                State::Running | State::Waiting | State::Done | State::Idle
            )
        });
        match agent {
            Some(s) if tab.auto_name => s.kind.tag().to_string(),
            _ => tab.name.clone(),
        }
    }

    /// The current space's tabs: their names and most urgent agent state.
    pub(super) fn tabs(&self, rows: &[Row]) -> Vec<Tab> {
        let Some(space) = self.current_space() else {
            return Vec::new();
        };
        space
            .tabs()
            .iter()
            .map(|t| Tab {
                name: self.tab_name(t),
                state: most_urgent(rows, |r| r.tab == t.id).unwrap_or(State::Unknown),
                custom: !t.auto_name,
                zoomed: t.zoomed,
            })
            .collect()
    }

    /// The tab bar as drawn now: the current space's tabs, and a notice or
    /// the armed prefix at the right.
    pub(super) fn tab_bar_view(&self, rows: &[Row]) -> TabBar<'_> {
        let armed = self.prefix.armed().then_some(self.prefix_name.as_str());
        TabBar {
            tabs: self.tabs(rows),
            current: self.current_space().map_or(0, |s| s.structure.current),
            first: self.tab_first,
            colors: self.tab_colors,
            status: self.notice.as_deref().or(armed).map(Cow::Borrowed),
        }
    }

    /// The prompt popup, while it is open: asking for a name, asking
    /// before a close or a delete, or listing worktrees to open.
    pub(super) fn prompt(&self) -> Option<Prompt<'_>> {
        let (title, body) = match &self.mode {
            Mode::Naming { target, line, .. } => (target.title(), Body::Input(line)),
            Mode::ConfirmClose { target, state, .. } => (
                self.close_title(target),
                Body::Message(format!("an agent is {}: close? y/n", state.label())),
            ),
            Mode::OpenWorktree(list) => (
                " Open worktree ",
                Body::List {
                    items: list.candidates.iter().map(|c| c.line.clone()).collect(),
                    selected: list.selected,
                },
            ),
            Mode::ConfirmDelete(q) => (" Delete worktree ", Body::Message(q.text())),
            _ => return None,
        };
        Some(Prompt {
            title,
            body,
            accent: self.tab_colors.current,
        })
    }

    /// The title of the question before closing `target`: a terminal alone
    /// in its tab closes the tab, and alone in its space's only tab, the
    /// space.
    fn close_title(&self, target: &CloseTarget) -> &'static str {
        let CloseTarget::Terminal(terminal) = target else {
            // A space, or a worktree space.
            return " Close space ";
        };
        let found = self.spaces.iter().find_map(|s| {
            let tab = s.tabs().iter().find(|t| t.contains(terminal))?;
            Some((s.tabs().len(), tab.splits.len()))
        });
        match found {
            Some((1, 1)) => " Close space ",
            Some((_, 1)) | None => " Close tab ",
            Some(_) => " Close pane ",
        }
    }

    /// Where the prompt popup is on the screen: drawn there, and clicks
    /// are judged against it.
    pub(super) fn prompt_area(&self) -> Rect {
        let screen = Rect::new(0, 0, self.screen.0, self.screen.1);
        self.prompt()
            .map_or_else(|| prompt::area(screen), |p| p.area(screen))
    }

    /// The tab bar's parts as drawn now, for clicks.
    pub(super) fn tab_layout(&self) -> tab_bar::Layout {
        tab_bar::layout(&self.tabs(&self.rows()), self.area().width, self.tab_first)
    }

    /// Scroll the tab bar so the current tab is in sight, when the current
    /// space or tab changed (scrolling with `<` and `>` stays put otherwise).
    pub(super) fn settle_tabs(&mut self) {
        let Some(space) = self.current_space() else {
            return;
        };
        let now = (space.id.clone(), space.structure.current);
        if self.tab_shown.as_ref() == Some(&now) {
            return;
        }
        if self.tab_shown.as_ref().is_none_or(|(id, _)| *id != now.0) {
            self.tab_first = 0;
        }
        let tabs = self.tabs(&self.rows());
        self.tab_first = tab_bar::scroll(&tabs, self.area().width, self.tab_first, now.1);
        self.tab_shown = Some(now);
    }

    /// The current tab's panes and where they are drawn on the screen,
    /// clipped to the session area: only the focused one while it is
    /// zoomed, as the others are hidden (the backend keeps their old places).
    pub(super) fn pane_rects(&self) -> Vec<(TerminalId, Rect)> {
        let area = self.area();
        let Some(tab) = self.current_space().and_then(|i| i.current_tab()) else {
            return Vec::new();
        };
        tab.shown_splits()
            .map(|s| {
                let r = Rect {
                    x: area.x.saturating_add(s.rect.x),
                    y: area.y.saturating_add(s.rect.y),
                    ..s.rect
                };
                (s.id.clone(), r.intersection(area))
            })
            .collect()
    }
}
