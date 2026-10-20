//! What the screen shows of the app, ready to draw: the sidebar's spaces and
//! agents, the current space's tabs, and where the current tab's panes go.

use super::rows::{most_urgent, Row};
use super::state::App;
use crate::sessions::State;
use crate::tmux::structure::Window;
use crate::ui::sidebar::{AgentLine, SpaceLine};
use crate::ui::tab_bar::Tab;
use ratatui::layout::Rect;

/// What a tab running Claude is called while the user has not named it.
const CLAUDE: &str = "claude";

impl App {
    /// Each space: its name, the most urgent state of its agents, and
    /// whether it is current.
    pub(super) fn sidebar_spaces(&self, rows: &[Row]) -> Vec<SpaceLine> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| SpaceLine {
                name: item.name().to_string(),
                state: most_urgent(rows, |r| r.item == item.id),
                current: i == self.current,
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

    /// Where row `r` runs, as `space · N:tab`.
    fn place_of(&self, r: &Row) -> Option<String> {
        let item = self.items.iter().find(|i| i.id == r.item)?;
        let (n, w) = item
            .windows()
            .iter()
            .enumerate()
            .find(|(_, w)| w.id == r.window)?;
        Some(format!("{} · {}:{}", item.name(), n + 1, self.tab_name(w)))
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
        let Some(item) = self.current_item() else {
            return Vec::new();
        };
        item.windows()
            .iter()
            .map(|w| Tab {
                name: self.tab_name(w),
                state: most_urgent(rows, |r| r.window == w.id).unwrap_or(State::Unknown),
            })
            .collect()
    }

    /// The current tab's panes and where they are drawn on the screen,
    /// clipped to the session area.
    pub(super) fn pane_rects(&self) -> Vec<(String, Rect)> {
        let area = self.area();
        let Some(window) = self.current_item().and_then(|i| i.current_window()) else {
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
