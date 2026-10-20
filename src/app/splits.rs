//! Splitting a tab into several panes and moving focus between them, through
//! tmux, which keeps the layout (ADR 0006); and keeping each item's size and
//! the saved state in step.

use super::item::clamp;
use super::saved::{self, Saved};
use super::state::App;
use crate::input::Direction;
use anyhow::Result;

impl App {
    /// Split the focused pane: side by side if `columns`, else top and
    /// bottom. A new shell takes the second half and the focus.
    pub fn split(&mut self, columns: bool) -> Result<()> {
        let (Some(pane), Some(id)) = (
            self.current_pane(),
            self.current_item().map(|i| i.id.clone()),
        ) else {
            return Ok(());
        };
        let dir = self.new_dir();
        self.server.split(&pane, columns, &dir)?;
        self.refresh(&id)
    }

    /// Focus the pane next to the focused one in `dir`, if there is one.
    pub fn move_focus(&mut self, dir: Direction) -> Result<()> {
        let (Some(pane), Some(id)) = (
            self.current_pane(),
            self.current_item().map(|i| i.id.clone()),
        ) else {
            return Ok(());
        };
        let flag = match dir {
            Direction::Left => "-L",
            Direction::Down => "-D",
            Direction::Up => "-U",
            Direction::Right => "-R",
        };
        self.server.select_towards(&pane, flag)?;
        self.refresh(&id)
    }

    /// Give the focused pane and the panes beside it the same size.
    pub fn equalise(&mut self) -> Result<()> {
        match self.current_pane() {
            Some(pane) => self.server.spread(&pane),
            None => Ok(()),
        }
    }

    /// Show every item at the size of the session area; tmux resizes its
    /// windows and reports the new layout.
    pub(super) fn relayout(&mut self) -> Result<()> {
        let area = self.area();
        let size = clamp((area.width, area.height));
        // One item that cannot be resized (it just ended) stops no other.
        let mut result = Ok(());
        for item in &mut self.items {
            result = result.and(item.resize(size));
        }
        result
    }

    /// Write the current item to the state directory.
    pub(super) fn save(&self) -> Result<()> {
        let saved = Saved {
            item: self.current_item().map(|i| i.id.clone()),
        };
        saved::save(&self.state_dir, &saved)
    }
}
