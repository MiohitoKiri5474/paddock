//! What the screen shows of the app, ready to draw: the sidebar's spaces and
//! agents, the current space's tabs, and where the current tab's panes go.

use super::rows::{most_urgent, Row};
use super::state::{App, Mode};
use crate::sessions::State;
use crate::tmux::structure::Window;
use crate::ui::sidebar::{tilde, AgentLine, SpaceLine};
use crate::ui::tab_bar::{self, Tab, TabBar};
use ratatui::layout::Rect;

/// What a tab running Claude is called while the user has not named it.
const CLAUDE: &str = "claude";

impl App {
    /// Each space: its name, the most urgent state of its agents, whether
    /// it is current, and its directory and branch once read.
    pub(super) fn sidebar_spaces(&self, rows: &[Row]) -> Vec<SpaceLine> {
        let home = std::env::var("HOME").unwrap_or_default();
        // tmux reports directories with symlinks resolved (`/tmp` is
        // `/private/tmp` on macOS), so home may be written either way.
        let real_home = std::fs::canonicalize(&home)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| home.clone());
        let show = |path: &str| {
            let short = tilde(path, &home);
            if short == path {
                tilde(path, &real_home)
            } else {
                short
            }
        };
        self.spaces
            .iter()
            .enumerate()
            .map(|(i, space)| {
                let place = self.places.get(&space.id);
                SpaceLine {
                    name: space.name().to_string(),
                    state: most_urgent(rows, |r| r.space == space.id),
                    current: i == self.current,
                    path: place.map(|p| show(&p.path)),
                    branch: place.and_then(|p| p.branch.clone()),
                }
            })
            .collect()
    }

    /// The main Claude sessions in open panes, across every space, in row
    /// order.
    pub(super) fn agents<'a>(&self, rows: &'a [Row]) -> Vec<&'a Row> {
        rows.iter().filter(|r| !r.closed).collect()
    }

    /// Each agent: its label, state, and its space and tab.
    pub(super) fn sidebar_agents(&self, rows: &[Row]) -> Vec<AgentLine> {
        self.agents(rows)
            .into_iter()
            .map(|r| AgentLine {
                label: r.label.clone(),
                state: r.state,
                place: self.place_of(r).unwrap_or_default(),
            })
            .collect()
    }

    /// Where row `r` runs, as `space · tab`.
    fn place_of(&self, r: &Row) -> Option<String> {
        let space = self.spaces.iter().find(|i| i.id == r.space)?;
        let w = space.windows().iter().find(|w| w.id == r.window)?;
        Some(format!("{} · {}", space.name(), self.tab_name(w)))
    }

    /// A tab's name as shown: tmux's, except that a tab tmux names (not
    /// renamed by the user) whose focused pane runs Claude is called
    /// `claude`, whatever process name tmux sees.
    fn tab_name(&self, w: &Window) -> String {
        let claude = self.session_in(&w.active).is_some_and(|s| {
            matches!(
                s.state,
                State::Running | State::Waiting | State::Done | State::Idle
            )
        });
        if w.auto_name && claude {
            CLAUDE.to_string()
        } else {
            w.name.clone()
        }
    }

    /// The current space's tabs: their names and most urgent Claude state.
    pub(super) fn tabs(&self, rows: &[Row]) -> Vec<Tab> {
        let Some(space) = self.current_space() else {
            return Vec::new();
        };
        space
            .windows()
            .iter()
            .map(|w| Tab {
                name: self.tab_name(w),
                state: most_urgent(rows, |r| r.window == w.id).unwrap_or(State::Unknown),
                custom: !w.auto_name,
            })
            .collect()
    }

    /// The tab bar as drawn now: the current space's tabs, or the question
    /// before a close (it takes the whole bar, so it always fits), the
    /// rename prompt, and a notice or the armed prefix at the right.
    pub(super) fn tab_bar_view(&self, rows: &[Row]) -> TabBar<'_> {
        let ask = match &self.mode {
            Mode::ConfirmClose { state, target, .. } => Some(format!(
                "Claude is {}: {}? y/n",
                state.label(),
                target.verb()
            )),
            _ => None,
        };
        let armed = self.prefix.armed().then(|| self.prefix_name.clone());
        TabBar {
            tabs: if ask.is_some() {
                Vec::new()
            } else {
                self.tabs(rows)
            },
            current: self.current_space().map_or(0, |s| s.structure.current),
            first: self.tab_first,
            colors: self.tab_colors,
            rename: match &self.mode {
                Mode::Rename { target, text } => Some((target.noun(), text.as_str())),
                _ => None,
            },
            status: ask.or_else(|| self.notice.clone()).or(armed),
        }
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
    /// clipped to the session area.
    pub(super) fn pane_rects(&self) -> Vec<(String, Rect)> {
        let area = self.area();
        let Some(window) = self.current_space().and_then(|i| i.current_window()) else {
            return Vec::new();
        };
        window
            .panes
            .iter()
            .map(|p| {
                let r = Rect {
                    x: area.x.saturating_add(p.rect.x),
                    y: area.y.saturating_add(p.rect.y),
                    ..p.rect
                };
                (p.id.clone(), r.intersection(area))
            })
            .collect()
    }
}
