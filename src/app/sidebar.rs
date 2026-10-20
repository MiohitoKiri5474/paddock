//! The sidebar with the keyboard (`C-a s`, `C-a w`) or the mouse: moving
//! over its spaces and agents, and acting on the one picked.

use super::space::MIN_SIZE;
use super::state::{App, Mode};
use crossterm::event::{KeyCode, KeyEvent};

/// The narrowest the sidebar gets.
pub const MIN_SIDEBAR: u16 = 16;
/// Columns `C-a {` and `C-a }` take away or add.
pub const SIDEBAR_STEP: i16 = 2;

/// The sidebar's width for a wish of `wanted` columns on a screen
/// `screen_cols` wide: at least [`MIN_SIDEBAR`], at most half the screen,
/// and always leaving the panes their minimum width.
pub fn clamp_width(wanted: u16, screen_cols: u16) -> u16 {
    let max = (screen_cols / 2).min(screen_cols.saturating_sub(MIN_SIZE.0));
    wanted.max(MIN_SIDEBAR).min(max)
}

/// The width the sidebar starts at: the one saved last time, else the
/// configured one.
pub fn starting_width(saved: Option<u16>, configured: u16) -> u16 {
    saved.unwrap_or(configured)
}

/// What a sidebar line stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// A space, by tmux session id (`$N`).
    Space(String),
    /// An agent, by the pane it runs in (`%N`).
    Agent(String),
}

/// The sidebar's keyboard selection: what it is on, and the index of its
/// line, so that if that space or agent goes, the selection stays on the
/// line that took its place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub pick: Option<Pick>,
    pub index: usize,
}

impl Selection {
    /// The selection on `picks` as they are now.
    pub fn anchored(&self, picks: &[Pick]) -> Self {
        let found = self
            .pick
            .as_ref()
            .and_then(|p| picks.iter().position(|q| q == p));
        let index = found.unwrap_or(self.index.min(picks.len().saturating_sub(1)));
        Self {
            pick: picks.get(index).cloned(),
            index,
        }
    }

    /// The selection `step` lines away, staying at either end.
    fn moved(&self, picks: &[Pick], step: isize) -> Self {
        let index = self
            .index
            .saturating_add_signed(step)
            .min(picks.len().saturating_sub(1));
        Self {
            pick: picks.get(index).cloned(),
            index,
        }
    }
}

impl App {
    /// Make the sidebar `step` columns wider (narrower if negative), within
    /// [`clamp_width`], showing it if hidden.
    pub(super) fn resize_sidebar(&mut self, step: i16) {
        let now = clamp_width(self.sidebar_cols, self.screen.0);
        self.sidebar_cols = clamp_width(now.saturating_add_signed(step), self.screen.0);
        self.sidebar_resized = Some(self.sidebar_cols);
        self.sidebar_hidden = false;
    }

    /// The sidebar's border dragged to screen column `col`.
    pub(super) fn drag_border(&mut self, col: u16) {
        self.sidebar_cols = clamp_width(col.saturating_add(1), self.screen.0);
        self.sidebar_resized = Some(self.sidebar_cols);
    }

    /// The sidebar's picks in line order: the spaces, then the agents.
    pub(super) fn sidebar_picks(&self) -> Vec<Pick> {
        let rows = self.rows();
        let spaces = self.spaces.iter().map(|i| Pick::Space(i.id.clone()));
        let agents = self
            .agents(&rows)
            .into_iter()
            .map(|r| Pick::Agent(r.pane.clone()));
        spaces.chain(agents).collect()
    }

    /// Give the sidebar the keyboard: on the current space if `on_space`,
    /// else on the first agent (the one that needs you most), if any.
    pub(super) fn focus_sidebar(&mut self, on_space: bool) {
        self.sidebar_hidden = false;
        let picks = self.sidebar_picks();
        let first_agent = picks.iter().position(|p| matches!(p, Pick::Agent(_)));
        let index = match first_agent {
            Some(i) if !on_space => i,
            _ => self.current,
        };
        self.mode = Mode::Sidebar(Selection {
            pick: picks.get(index).cloned(),
            index,
        });
    }

    /// The sidebar selection, if the sidebar has the keyboard.
    pub(super) fn sidebar_selection(&self) -> Option<usize> {
        match &self.mode {
            Mode::Sidebar(s) => Some(s.anchored(&self.sidebar_picks()).index),
            _ => None,
        }
    }

    /// `j`/`k` (or arrows) move over the spaces and agents; `Enter` shows a
    /// space's tabs or jumps to an agent's pane, giving the keyboard back;
    /// `x` closes the space or the agent's pane; `f` forks the agent (or
    /// the space's agent that needs you most); `Esc` gives the keyboard back.
    pub(super) fn sidebar_key(&mut self, s: Selection, key: KeyEvent) {
        let picks = self.sidebar_picks();
        let s = s.anchored(&picks);
        self.mode = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Mode::Sidebar(s.moved(&picks, 1)),
            KeyCode::Char('k') | KeyCode::Up => Mode::Sidebar(s.moved(&picks, -1)),
            KeyCode::Enter => {
                if let Some(p) = &s.pick {
                    self.open(p);
                }
                Mode::Normal
            }
            KeyCode::Char('x') => {
                // Asking takes the keyboard; otherwise the sidebar keeps it.
                self.mode = Mode::Sidebar(s.clone());
                match &s.pick {
                    Some(Pick::Space(id)) => self.report(|app| app.close_space(id)),
                    Some(Pick::Agent(pane)) => self.report(|app| app.close(pane, None)),
                    None => {}
                }
                return;
            }
            KeyCode::Char('f') => match self.agent_of(s.pick.as_ref()) {
                // The fork's tab gets the keyboard.
                Some(pane) => {
                    self.report(|app| app.fork(&pane));
                    Mode::Normal
                }
                None => {
                    self.notice = Some("no Claude session here".into());
                    Mode::Sidebar(s)
                }
            },
            KeyCode::Esc => Mode::Normal,
            _ => Mode::Sidebar(s),
        };
        self.report(|app| app.relayout());
        self.report(|app| app.save());
    }

    /// Show what `pick` stands for: a space's tabs, or an agent's pane.
    pub(super) fn open(&mut self, pick: &Pick) {
        match pick {
            Pick::Space(id) => {
                if let Some(i) = self.spaces.iter().position(|it| it.id == *id) {
                    self.select_space(i);
                }
            }
            Pick::Agent(pane) => self.jump_to(pane),
        }
    }

    /// The agent's pane `pick` stands for; for a space, the pane of its
    /// agent that needs you most.
    fn agent_of(&self, pick: Option<&Pick>) -> Option<String> {
        match pick? {
            Pick::Agent(pane) => Some(pane.clone()),
            Pick::Space(id) => self
                .rows()
                .into_iter()
                .find(|r| r.space == *id)
                .map(|r| r.pane),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picks() -> Vec<Pick> {
        vec![
            Pick::Space("$1".into()),
            Pick::Space("$2".into()),
            Pick::Agent("%3".into()),
        ]
    }

    #[test]
    fn the_selection_follows_its_pick_and_falls_back_to_its_line() {
        let s = Selection {
            pick: Some(Pick::Agent("%3".into())),
            index: 0,
        };
        assert_eq!(s.anchored(&picks()).index, 2, "found where it moved");
        let gone = Selection {
            pick: Some(Pick::Space("$9".into())),
            index: 1,
        };
        let a = gone.anchored(&picks());
        assert_eq!(
            a.pick,
            Some(Pick::Space("$2".into())),
            "the line's new pick"
        );
        let past_the_end = Selection {
            pick: None,
            index: 7,
        };
        assert_eq!(past_the_end.anchored(&picks()).index, 2);
        assert_eq!(past_the_end.anchored(&[]).pick, None);
    }

    #[test]
    fn the_sidebar_width_stays_between_sixteen_and_half_the_screen() {
        assert_eq!(clamp_width(32, 100), 32);
        assert_eq!(clamp_width(4, 100), MIN_SIDEBAR);
        assert_eq!(clamp_width(80, 100), 50);
        // A narrow screen: half of it, the panes keep their minimum.
        assert_eq!(clamp_width(32, 20), 10);
        assert_eq!(clamp_width(32, 3), 1);
        assert_eq!(clamp_width(32, 0), 0);
    }

    #[test]
    fn a_saved_width_wins_over_the_configured_one() {
        assert_eq!(starting_width(Some(40), 32), 40);
        assert_eq!(starting_width(None, 28), 28);
    }

    #[test]
    fn moving_stays_at_either_end() {
        let s = Selection::default().anchored(&picks());
        assert_eq!(s.moved(&picks(), -1).index, 0);
        let last = s.moved(&picks(), 5);
        assert_eq!(last.pick, Some(Pick::Agent("%3".into())));
        assert_eq!(s.moved(&picks(), 1).index, 1);
    }
}
