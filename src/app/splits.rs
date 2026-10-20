//! Splitting a tab into several terminals, moving focus between them, and
//! keeping each terminal's size and the saved layout in step.

use super::layout::{terminal_size, Direction, Split};
use super::saved::{self, Saved};
use super::state::App;
use anyhow::Result;

impl App {
    /// Split the focused terminal; a new shell takes the second half and focus.
    pub fn split(&mut self, split: Split) -> Result<()> {
        let area = self.area();
        let Some(tab) = self.tabs.current() else {
            return Ok(());
        };
        // Where the new terminal will be, so it starts at its final size.
        let mut probe = tab.content.clone();
        probe.split(split, String::new());
        let rect = probe
            .rects(area)
            .0
            .into_iter()
            .find(|(id, _)| id.is_empty());
        let size = terminal_size(rect.map_or(area, |(_, r)| r));
        let id = self.new_terminal(size)?;
        if let Some(tab) = self.tabs.current_mut() {
            tab.content.split(split, id);
        }
        self.relayout()
    }

    pub fn move_focus(&mut self, dir: Direction) {
        let area = self.area();
        if let Some(tab) = self.tabs.current_mut() {
            tab.content.move_focus(dir, area);
        }
    }

    pub fn equalise(&mut self) -> Result<()> {
        if let Some(tab) = self.tabs.current_mut() {
            tab.content.equalise();
        }
        self.relayout()
    }

    /// Give every terminal, in every tab, the size its layout gives it.
    pub(super) fn relayout(&mut self) -> Result<()> {
        let area = self.area();
        // One terminal that cannot be resized (it just ended) stops no other.
        let mut result = Ok(());
        for tab in self.tabs.iter() {
            for (id, rect) in tab.content.rects(area).0 {
                let (cols, rows) = terminal_size(rect);
                match self.terminals.get_mut(&id) {
                    Some(t) if t.emulator().size() != (cols, rows) => {
                        result = result.and(t.resize(cols, rows));
                    }
                    _ => {}
                }
            }
        }
        result
    }

    /// Write the tabs and their layouts to the state directory.
    pub(super) fn save(&self) -> Result<()> {
        let saved = Saved {
            current: self.tabs.current_index(),
            tabs: self.tabs.iter().map(|t| t.content.clone()).collect(),
        };
        saved::save(&self.state_dir, &saved)
    }
}
