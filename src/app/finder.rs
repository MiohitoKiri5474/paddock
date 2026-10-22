//! The finder (`C-a g`): a box with a text field and, under it, every
//! space, tab and agent that fuzzy-matches what is typed, best first.
//! `Enter` shows the one selected: its space and tab, with its pane
//! focused. On an empty query, letters show only agents that are waiting
//! (`b`, blocked), running (`w`, working), idle (`i`) or done (`d`), or
//! everything again (`a`, all).

use super::rows::Row;
use super::state::{App, Mode, Panel};
use crate::fuzzy;
use crate::input::line::LineEdit;
use crate::paths::HomePaths;
use crate::sessions::State;
use crate::ui::finder::{self as ui, FinderLine};
use crate::ui::sidebar::SpaceLine;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

/// What a finder row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A space, by tmux session id (`$N`).
    Space(String),
    /// A tab: its space's id and its window id (`@N`).
    Tab(String, String),
    /// An agent's pane (`%N`).
    Pane(String),
}

/// What kind of thing a finder row is; shown as its marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Space,
    Tab,
    Agent,
}

impl Kind {
    /// The marker before the name, also matched by the query.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Space => "space",
            Kind::Tab => "tab",
            Kind::Agent => "agent",
        }
    }
}

/// One row of the finder: what it shows, what the query is matched
/// against, and what it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderRow {
    pub kind: Kind,
    pub name: String,
    /// Dim, after the name: a space's directory and branch, a tab's
    /// directory, an agent's state and place.
    pub detail: String,
    /// The directory, matched besides the name and the kind.
    pub path: Option<String>,
    /// The branch, matched besides the name and the kind.
    pub branch: Option<String>,
    /// An agent's state, for the state filters.
    pub state: Option<State>,
    pub target: Target,
}

impl FinderRow {
    /// How well `query` matches this row: its best field.
    fn score(&self, query: &str) -> Option<i32> {
        [
            Some(self.name.as_str()),
            Some(self.kind.label()),
            self.path.as_deref(),
            self.branch.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|field| fuzzy::score(query, field))
        .max()
    }
}

/// Which rows the finder lists by state, set by a letter on an empty
/// query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StateFilter {
    /// Every row.
    #[default]
    All,
    /// Only agents in this state.
    Only(State),
}

/// The open finder: the line typed, the state filter and the selected
/// match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Finder {
    pub line: LineEdit,
    /// Which rows show by state.
    pub state: StateFilter,
    /// The index of the selected match.
    pub selected: usize,
}

/// The finder's title in its border while every row shows;
/// [`Finder::title`] adds a state filter after it.
const TITLE: &str = " Finder ";

impl Finder {
    /// The title: [`TITLE`], or ` Finder · waiting ` with a state filter.
    pub fn title(&self) -> String {
        match self.state {
            StateFilter::All => TITLE.to_string(),
            StateFilter::Only(state) => format!("{} · {} ", TITLE.trim_end(), state.label()),
        }
    }

    /// Change the line with `edit`; the selection starts over on the
    /// first match.
    pub fn edited(&mut self, edit: impl FnOnce(&mut LineEdit)) {
        edit(&mut self.line);
        self.selected = 0;
    }

    /// Show the rows `state` lets through; the selection starts over.
    fn filtered(&mut self, state: StateFilter) {
        self.state = state;
        self.selected = 0;
    }

    /// The indices of the rows of `rows` that match, best first; with a
    /// state filter, only agents in that state.
    pub fn matches(&self, rows: &[FinderRow]) -> Vec<usize> {
        let query = self.line.text();
        let mut scored: Vec<(usize, i32)> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| match self.state {
                StateFilter::All => true,
                StateFilter::Only(state) => r.state == Some(state),
            })
            .filter_map(|(i, r)| Some((i, r.score(query)?)))
            .collect();
        // Stable: rows in their own order within a score.
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(i, _)| i).collect()
    }

    /// The same finder with its selection moved `step` matches, staying
    /// at either end of `count` matches.
    fn moved(self, step: isize, count: usize) -> Self {
        let selected = self
            .selected
            .saturating_add_signed(step)
            .min(count.saturating_sub(1));
        Self { selected, ..self }
    }
}

/// The state filter a letter on an empty query sets: waiting (`b`,
/// blocked), running (`w`, working), idle (`i`), done (`d`), or every row
/// (`a`, all).
fn state_filter_for(c: char) -> Option<StateFilter> {
    match c {
        'b' => Some(StateFilter::Only(State::Waiting)),
        'w' => Some(StateFilter::Only(State::Running)),
        'i' => Some(StateFilter::Only(State::Idle)),
        'd' => Some(StateFilter::Only(State::Done)),
        'a' => Some(StateFilter::All),
        _ => None,
    }
}

/// A space's or tab's detail: its directory and branch.
fn place_detail(path: Option<&str>, branch: Option<&str>) -> String {
    [path, branch]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ")
}

/// A tab's directory and branch, taken as a pair from one place: the
/// Claude's in its focused pane (`claude`) when its directory is known,
/// else its space's (the first tab's), which needs no round trip to tmux.
/// A branch is never paired with another place's directory.
fn tab_place(
    claude: (Option<String>, Option<String>),
    space: &SpaceLine,
) -> (Option<String>, Option<String>) {
    match claude {
        (Some(path), branch) => (Some(path), branch),
        (None, _) => (space.path.clone(), space.branch.clone()),
    }
}

/// The branch row `r`'s main agent works on, if known.
fn main_branch(r: &Row) -> Option<String> {
    r.main_branch().map(str::to_string)
}

impl App {
    /// `C-a g`: open the finder on an empty query. A panel with the
    /// keyboard (the sidebar or the dashboard) gets it back on `Esc`.
    pub(super) fn open_finder(&mut self) {
        let back = std::mem::replace(&mut self.mode, Mode::Normal).into_panel();
        self.mode = Mode::Finder {
            finder: Finder::default(),
            back,
        };
    }

    /// The finder's rows as things are now: every space, every tab, then
    /// every agent (but external sessions).
    pub(super) fn finder_rows(&self) -> Vec<FinderRow> {
        let rows = self.rows();
        let home = HomePaths::new();
        let spaces = self.space_lines(&rows);
        let mut out: Vec<FinderRow> = spaces
            .iter()
            .zip(&self.spaces)
            .map(|(line, space)| {
                // Nothing until read.
                let (path, branch) = match self.places.get(&space.id) {
                    Some(place) => {
                        let (root, branch) = place.root_shown(&home);
                        (Some(root), branch)
                    }
                    None => (None, None),
                };
                FinderRow {
                    kind: Kind::Space,
                    name: line.name.clone(),
                    detail: place_detail(path.as_deref(), branch.as_deref()),
                    path,
                    branch,
                    state: None,
                    target: Target::Space(space.id.clone()),
                }
            })
            .collect();
        for (line, space) in spaces.iter().zip(&self.spaces) {
            for w in space.windows() {
                let claude_branch = rows
                    .iter()
                    .find(|r| r.pane == w.active)
                    .and_then(main_branch);
                let claude = (self.shown_cwd(&w.active, &home), claude_branch);
                let (path, branch) = tab_place(claude, line);
                out.push(FinderRow {
                    kind: Kind::Tab,
                    name: format!("{} › {}", space.name(), self.tab_name(w)),
                    detail: place_detail(path.as_deref(), branch.as_deref()),
                    path,
                    branch,
                    state: None,
                    target: Target::Tab(space.id.clone(), w.id.clone()),
                });
            }
        }
        // External sessions have nothing to show: the finder leaves them out.
        let agents = rows.iter().filter(|r| !r.external);
        out.extend(agents.map(|r| self.agent_row(r, &home)));
        out
    }

    /// The directory of the agent session in pane `pane`, home as `~`,
    /// if known.
    fn shown_cwd(&self, pane: &str, home: &HomePaths) -> Option<String> {
        self.session_in(pane)
            .and_then(|s| s.cwd.as_deref())
            .map(|p| home.show(p))
    }

    /// Agent row `r` of the sidebar as a finder row.
    fn agent_row(&self, r: &Row, home: &HomePaths) -> FinderRow {
        let place = self.place_of(r, " › ").unwrap_or_default();
        FinderRow {
            kind: Kind::Agent,
            name: r.label.clone(),
            detail: format!("{} · {place}", r.state.label()),
            path: self.shown_cwd(&r.pane, home),
            branch: main_branch(r),
            state: Some(r.state),
            target: Target::Pane(r.pane.clone()),
        }
    }

    /// A key while the finder is open: `Enter` opens the selected match,
    /// `Esc` closes the finder, `Up`/`Down` (`C-p`/`C-n`) move, a filter
    /// letter on an empty query sets the state filter, and other keys edit
    /// the line, which starts the selection over.
    pub(super) fn finder_key(&mut self, mut finder: Finder, back: Option<Panel>, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = self.finder_rows();
        let count = finder.matches(&rows).len();
        let step = match key.code {
            KeyCode::Esc => {
                self.mode = Mode::back_to(back);
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::back_to(back);
                let target = finder
                    .matches(&rows)
                    .get(finder.selected)
                    .map(|&i| rows[i].target.clone());
                if let Some(target) = target {
                    self.open_target(&target);
                }
                return;
            }
            KeyCode::Up => Some(-1),
            KeyCode::Down => Some(1),
            KeyCode::Char('p') if ctrl => Some(-1),
            KeyCode::Char('n') if ctrl => Some(1),
            _ => None,
        };
        if let Some(step) = step {
            self.mode = Mode::Finder {
                finder: finder.moved(step, count),
                back,
            };
            return;
        }
        let state = match key.code {
            KeyCode::Char(c) if !ctrl && finder.line.text().is_empty() => state_filter_for(c),
            _ => None,
        };
        match state {
            Some(state) => finder.filtered(state),
            None => finder.edited(|line| line.key(key)),
        }
        self.mode = Mode::Finder { finder, back };
    }

    /// Show what `target` stands for: a space's tabs, a tab, or an agent's
    /// pane, with the keyboard back in the terminal. A float shown hides
    /// first, as before any page change.
    fn open_target(&mut self, target: &Target) {
        self.hide_float();
        self.mode = Mode::Normal;
        match target {
            Target::Space(id) => {
                if let Some(i) = self.spaces.iter().position(|s| s.id == *id) {
                    self.select_space(i);
                }
            }
            Target::Tab(id, window) => {
                let Some(i) = self.spaces.iter().position(|s| s.id == *id) else {
                    return;
                };
                self.select_space(i);
                let tab = self.spaces[i]
                    .windows()
                    .iter()
                    .position(|w| w.id == *window);
                if let Some(tab) = tab {
                    self.report(|app| app.select_tab(tab));
                }
            }
            Target::Pane(pane) => self.jump_to(pane),
        }
        self.relayout_and_save();
    }

    /// Where the finder's box is on the screen.
    pub(super) fn finder_area(&self) -> Rect {
        ui::area(Rect::new(0, 0, self.screen.0, self.screen.1))
    }

    /// The finder, ready to draw, while it is open.
    pub(super) fn finder_view(&self) -> Option<ui::View<'_>> {
        let Mode::Finder { finder, .. } = &self.mode else {
            return None;
        };
        let rows = self.finder_rows();
        let lines = finder
            .matches(&rows)
            .into_iter()
            .map(|i| FinderLine {
                kind: rows[i].kind.label(),
                name: rows[i].name.clone(),
                detail: rows[i].detail.clone(),
            })
            .collect();
        Some(ui::View {
            title: finder.title(),
            line: &finder.line,
            lines,
            selected: finder.selected,
            accent: self.tab_colors.current,
        })
    }

    /// The mouse while the finder is open: a press outside the box
    /// closes it as `Esc` would; a click on a row opens it; the wheel
    /// moves the selection. Returns whether the screen may have changed.
    pub(super) fn finder_mouse(&mut self, m: MouseEvent) -> bool {
        let area = self.finder_area();
        let inside = area.contains(Position::new(m.column, m.row));
        let Mode::Finder { finder, back } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return false;
        };
        let rows = self.finder_rows();
        let matches = finder.matches(&rows);
        let step = match m.kind {
            MouseEventKind::Down(_) if !inside => {
                self.mode = Mode::back_to(back);
                return true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = ui::row_at(area, finder.selected, matches.len(), m.row);
                match hit.and_then(|i| matches.get(i)) {
                    Some(&i) => {
                        let target = rows[i].target.clone();
                        self.open_target(&target);
                        return true;
                    }
                    None => 0,
                }
            }
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::ScrollDown => 1,
            _ => 0,
        };
        self.mode = Mode::Finder {
            finder: finder.moved(step, matches.len()),
            back,
        };
        step != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: Kind, name: &str, path: Option<&str>, state: Option<State>) -> FinderRow {
        FinderRow {
            kind,
            name: name.into(),
            detail: String::new(),
            path: path.map(str::to_string),
            branch: None,
            state,
            target: Target::Pane(name.into()),
        }
    }

    fn rows() -> Vec<FinderRow> {
        vec![
            row(Kind::Space, "api-server", Some("~/project/api"), None),
            row(
                Kind::Tab,
                "api-server › claude",
                Some("~/project/api"),
                None,
            ),
            row(Kind::Agent, "claude", None, Some(State::Waiting)),
            row(Kind::Agent, "api", None, Some(State::Idle)),
        ]
    }

    fn typed(text: &str) -> Finder {
        Finder {
            line: LineEdit::new(text),
            ..Finder::default()
        }
    }

    #[test]
    fn an_empty_query_lists_everything_in_order() {
        assert_eq!(Finder::default().matches(&rows()), [0, 1, 2, 3]);
    }

    #[test]
    fn the_best_match_comes_first_and_kind_and_path_count() {
        // `api` starts three names alike: their own order; `cl` scattered
        // in `api-server › claude`'s path loses to its word start.
        assert_eq!(typed("api").matches(&rows()), [0, 1, 3]);
        assert_eq!(typed("cla").matches(&rows()), [1, 2]);
        assert_eq!(typed("agent").matches(&rows()), [2, 3]);
        assert_eq!(typed("project").matches(&rows()), [0, 1]);
        assert!(typed("zzz").matches(&rows()).is_empty());
    }

    #[test]
    fn a_state_filter_keeps_only_agents_in_that_state() {
        let waiting = Finder {
            state: StateFilter::Only(State::Waiting),
            ..Finder::default()
        };
        assert_eq!(waiting.matches(&rows()), [2]);
        assert_eq!(waiting.title(), " Finder · waiting ");
        assert_eq!(Finder::default().title(), " Finder ");
    }

    #[test]
    fn state_letters_map_to_state_filters() {
        let only = |s| Some(StateFilter::Only(s));
        assert_eq!(state_filter_for('b'), only(State::Waiting));
        assert_eq!(state_filter_for('w'), only(State::Running));
        assert_eq!(state_filter_for('i'), only(State::Idle));
        assert_eq!(state_filter_for('d'), only(State::Done));
        assert_eq!(state_filter_for('a'), Some(StateFilter::All));
        assert_eq!(state_filter_for('x'), None);
    }

    #[test]
    fn editing_or_filtering_starts_the_selection_over() {
        let mut f = Finder {
            selected: 3,
            ..Finder::default()
        };
        f.edited(|line| line.insert("x"));
        assert_eq!((f.line.text(), f.selected), ("x", 0));
        f.selected = 2;
        f.filtered(StateFilter::Only(State::Idle));
        assert_eq!((f.state, f.selected), (StateFilter::Only(State::Idle), 0));
    }

    #[test]
    fn a_tab_takes_its_claudes_place_else_its_spaces_and_matches_its_branch() {
        let space = SpaceLine {
            name: "api".into(),
            state: None,
            current: true,
            path: Some("~/project/api".into()),
            branch: Some("feat/login".into()),
            linked: false,
        };
        let none = (None, None);
        assert_eq!(
            tab_place(none, &space),
            (Some("~/project/api".into()), Some("feat/login".into()))
        );
        let claude = (Some("~/wt/fix".into()), Some("fix/css".into()));
        assert_eq!(
            tab_place(claude, &space),
            (Some("~/wt/fix".into()), Some("fix/css".into()))
        );
        // Claude's directory known but no main task yet: never the
        // space's branch beside Claude's directory.
        let no_task = (Some("~/wt/fix".into()), None);
        assert_eq!(tab_place(no_task, &space), (Some("~/wt/fix".into()), None));
        // A branch without a directory is not a place: the space's pair.
        let no_path = (None, Some("fix/css".into()));
        assert_eq!(
            tab_place(no_path, &space),
            (Some("~/project/api".into()), Some("feat/login".into()))
        );
        // A branch query finds the tab.
        let (path, branch) = tab_place((None, None), &space);
        let tab = FinderRow {
            path,
            branch,
            ..row(Kind::Tab, "api › home", None, None)
        };
        assert_eq!(typed("login").matches(&[tab]), [0]);
        assert_eq!(place_detail(Some("~/a"), Some("main")), "~/a  main");
        assert_eq!(place_detail(None, None), "");
    }

    #[test]
    fn moving_stays_at_either_end() {
        let f = Finder::default().moved(-1, 3);
        assert_eq!(f.selected, 0);
        assert_eq!(f.moved(5, 3).selected, 2);
        assert_eq!(Finder::default().moved(1, 0).selected, 0);
    }
}
