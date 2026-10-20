//! What the screen shows of the app, ready to draw: the sidebar items, the
//! current item's tabs, and where the current tab's panes go.

use super::rows::{most_urgent, Row};
use super::state::App;
use crate::sessions::State;
use crate::ui::sidebar::SidebarItem;
use crate::ui::tab_bar::Tab;
use ratatui::layout::Rect;

/// What a tab running Claude is called while the user has not named it.
const CLAUDE: &str = "claude";

impl App {
    /// Each sidebar item: its name, the most urgent Claude state in it, its
    /// number of tabs, and whether it is current.
    pub(super) fn sidebar_items(&self, rows: &[Row]) -> Vec<SidebarItem> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| SidebarItem {
                name: item.name().to_string(),
                state: most_urgent(rows, |r| r.item == item.id),
                tabs: item.windows().len(),
                current: i == self.current,
            })
            .collect()
    }

    /// The current item's tabs: their names and most urgent Claude state.
    /// A tab tmux names (not renamed by the user) whose focused pane runs
    /// Claude is called `claude`, whatever process name tmux sees.
    pub(super) fn tabs(&self, rows: &[Row]) -> Vec<Tab> {
        let Some(item) = self.current_item() else {
            return Vec::new();
        };
        item.windows()
            .iter()
            .map(|w| {
                let claude = self.session_in(&w.active).is_some_and(|s| {
                    matches!(
                        s.state,
                        State::Running | State::Waiting | State::Done | State::Idle
                    )
                });
                let name = if w.auto_name && claude {
                    CLAUDE.to_string()
                } else {
                    w.name.clone()
                };
                Tab {
                    name,
                    state: most_urgent(rows, |r| r.window == w.id).unwrap_or(State::Unknown),
                }
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
