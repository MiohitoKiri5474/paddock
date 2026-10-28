//! One tab on the native server: its [`Layout`], its name, which terminal
//! has the focus, whether it is zoomed, and the order its terminals last
//! had the focus in. What changes the layout gives the zoom up first, as
//! tmux does; only focusing the zoomed terminal itself keeps it.

use super::axis::Axis;
use super::layout::{BorderHit, Layout};
use crate::backend::{Split, TabId, TerminalId};
use crate::input::Direction;
use anyhow::Result;
use ratatui::layout::Rect;

#[derive(Clone)]
pub struct Tab {
    pub id: TabId,
    name: String,
    /// Named after what runs in it: the user has not renamed it.
    auto_name: bool,
    layout: Layout,
    active: TerminalId,
    zoomed: bool,
    /// Its terminals, the one focused last at the end.
    focused: Vec<TerminalId>,
}

impl Tab {
    /// A tab of one terminal, named `name` until what runs in it names it.
    pub fn new(id: TabId, name: String, terminal: TerminalId) -> Self {
        Self {
            id,
            name,
            auto_name: true,
            layout: Layout::new(terminal.clone()),
            active: terminal.clone(),
            zoomed: false,
            focused: vec![terminal],
        }
    }

    /// A tab as `layout.json` brings it back: `layout`, focusing
    /// `active` (one of its terminals), zoomed if `zoomed` and it has
    /// more than one.
    pub fn restore(
        id: TabId,
        name: String,
        auto_name: bool,
        layout: Layout,
        active: TerminalId,
        zoomed: bool,
    ) -> Self {
        let mut focused = layout.terminals();
        focused.retain(|t| *t != active);
        focused.push(active.clone());
        Self {
            id,
            name,
            auto_name,
            zoomed: zoomed && focused.len() > 1,
            layout,
            active,
            focused,
        }
    }

    /// Its tree of splits.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The tab follows what runs in it: the user has not renamed it.
    pub fn auto_name(&self) -> bool {
        self.auto_name
    }

    /// Name the tab `name` for good: it no longer follows what runs in it.
    pub fn rename(&mut self, name: String) {
        self.name = name;
        self.auto_name = false;
    }

    /// Name the tab after what runs in it, `name`, unless the user has
    /// renamed it; true if its name changed.
    pub fn follow(&mut self, name: String) -> bool {
        if !self.auto_name || self.name == name {
            return false;
        }
        self.name = name;
        true
    }

    pub fn active(&self) -> &TerminalId {
        &self.active
    }

    pub fn zoomed(&self) -> bool {
        self.zoomed
    }

    pub fn contains(&self, terminal: &TerminalId) -> bool {
        self.layout.contains(terminal)
    }

    pub fn terminals(&self) -> Vec<TerminalId> {
        self.layout.terminals()
    }

    /// Every terminal's place in a tab of `size`, in the tree's order:
    /// while zoomed, the focused one fills the tab and the others keep
    /// their places, hidden. Never a place of no cell (see [`Layout`]).
    pub fn splits(&self, size: (u16, u16)) -> Vec<Split> {
        let full = Rect::new(0, 0, size.0.max(1), size.1.max(1));
        let placed = self.layout.place(size);
        placed
            .into_iter()
            .map(|(id, rect)| {
                let rect = if self.zoomed && id == self.active {
                    full
                } else {
                    rect
                };
                Split { id, rect }
            })
            .collect()
    }

    /// Focus `terminal`: focusing another than the zoomed one unzooms.
    pub fn focus(&mut self, terminal: &TerminalId) {
        if !self.contains(terminal) {
            return;
        }
        if self.active != *terminal {
            self.zoomed = false;
        }
        self.active = terminal.clone();
        self.focused.retain(|t| t != terminal);
        self.focused.push(terminal.clone());
    }

    /// Zoom `terminal` (focusing it), or give the layout back if the tab
    /// is zoomed. A tab of one terminal does not zoom.
    pub fn zoom(&mut self, terminal: &TerminalId) {
        if self.zoomed {
            self.zoomed = false;
        } else if self.focused.len() > 1 && self.contains(terminal) {
            self.focus(terminal);
            self.zoomed = true;
        }
    }

    /// Split `target` along `axis` for `new`, which takes the focus.
    pub fn split(
        &mut self,
        target: &TerminalId,
        new: TerminalId,
        axis: Axis,
        size: (u16, u16),
    ) -> Result<()> {
        self.layout.split(target, new.clone(), axis, size)?;
        self.zoomed = false;
        self.focus(&new);
        Ok(())
    }

    /// Take `terminal` out; the terminal focused before it takes the
    /// focus. False if it was the last one: the tab goes with it.
    pub fn remove(&mut self, terminal: &TerminalId, size: (u16, u16)) -> Result<bool> {
        if !self.layout.remove(terminal, size)? {
            return Ok(false);
        }
        self.zoomed = false;
        self.focused.retain(|t| t != terminal);
        if self.active == *terminal {
            if let Some(last) = self.focused.last() {
                self.active = last.clone();
            }
        }
        Ok(true)
    }

    /// Focus the terminal next to `terminal` in `dir`, if any.
    pub fn focus_towards(&mut self, terminal: &TerminalId, dir: Direction, size: (u16, u16)) {
        let rank = |t: &TerminalId| {
            let at = self.focused.iter().position(|f| f == t);
            at.map_or(0, |i| i as u64 + 1)
        };
        // None there: the zoom stays, as tmux puts it back.
        if let Some(next) = self.layout.neighbour(terminal, dir, size, rank) {
            self.zoomed = false;
            self.focus(&next);
        }
    }

    /// Move `terminal`'s border (see [`Layout::resize`]).
    pub fn resize(
        &mut self,
        terminal: &TerminalId,
        dir: Direction,
        cells: u16,
        size: (u16, u16),
    ) -> Result<()> {
        self.zoomed = false;
        self.layout.resize(terminal, dir, cells, size)
    }

    /// Size the cell ending in `terminal` (see [`Layout::resize_to`]).
    pub fn resize_to(
        &mut self,
        terminal: &TerminalId,
        axis: Axis,
        len: u16,
        size: (u16, u16),
    ) -> Result<()> {
        self.zoomed = false;
        self.layout.resize_to(terminal, axis, len, size)
    }

    /// Even out `terminal` and its neighbours (see [`Layout::spread`]).
    pub fn spread(&mut self, terminal: &TerminalId, size: (u16, u16)) -> Result<()> {
        self.zoomed = false;
        self.layout.spread(terminal, size)
    }

    /// The border at `(x, y)` of the tab at `size`: none while zoomed,
    /// as none is drawn.
    pub fn border_at(&self, x: u16, y: u16, size: (u16, u16)) -> Option<BorderHit> {
        if self.zoomed {
            return None;
        }
        self.layout.border_at(size, x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u16, u16) = (68, 29);

    fn t(n: u32) -> TerminalId {
        TerminalId::new(format!("%{n}"))
    }

    fn sizes(tab: &Tab) -> Vec<String> {
        let splits = tab.splits(SIZE);
        let size = |s: &Split| format!("{}x{}", s.rect.width, s.rect.height);
        splits.iter().map(size).collect()
    }

    /// `%0 | %1`, the new one focused.
    fn split() -> Tab {
        let mut tab = Tab::new(TabId::new("@1"), "home".into(), t(0));
        tab.split(&t(0), t(1), Axis::Columns, SIZE).unwrap();
        tab
    }

    #[test]
    fn a_new_split_takes_the_focus() {
        let tab = split();
        assert_eq!(tab.active(), &t(1));
        assert_eq!(tab.terminals(), [t(0), t(1)]);
    }

    #[test]
    fn a_zoomed_terminal_fills_the_tab_and_the_others_keep_their_places() {
        let mut tab = split();
        tab.zoom(&t(1));
        assert!(tab.zoomed());
        assert_eq!(sizes(&tab), ["34x29", "68x29"]);
        tab.zoom(&t(1));
        assert!(!tab.zoomed());
        assert_eq!(sizes(&tab), ["34x29", "33x29"]);
        // Zooming another terminal focuses it.
        tab.zoom(&t(0));
        assert_eq!(tab.active(), &t(0));
        assert_eq!(sizes(&tab), ["68x29", "33x29"]);
    }

    #[test]
    fn a_zoomed_terminal_in_a_tab_of_no_size_still_has_a_cell() {
        let mut tab = split();
        tab.zoom(&t(1));
        let splits = tab.splits((0, 0));
        assert!(splits
            .iter()
            .all(|s| s.rect.width >= 1 && s.rect.height >= 1));
    }

    #[test]
    fn a_renamed_tab_stops_following_what_runs_in_it() {
        let mut tab = split();
        assert!(tab.follow("vim".into()));
        assert!(!tab.follow("vim".into()), "the same name: no change");
        tab.rename("logs".into());
        assert!(!tab.follow("zsh".into()));
        assert_eq!((tab.name(), tab.auto_name()), ("logs", false));
    }

    #[test]
    fn a_tab_of_one_terminal_does_not_zoom() {
        let mut tab = Tab::new(TabId::new("@1"), "home".into(), t(0));
        tab.zoom(&t(0));
        assert!(!tab.zoomed());
    }

    #[test]
    fn focusing_the_zoomed_terminal_keeps_the_zoom_and_anything_else_gives_it_up() {
        let mut tab = split();
        tab.zoom(&t(1));
        tab.focus(&t(1));
        assert!(tab.zoomed(), "the zoomed one itself");
        tab.focus(&t(0));
        assert!(!tab.zoomed());
        let changes: [fn(&mut Tab); 5] = [
            |tab| tab.focus_towards(&t(1), Direction::Left, SIZE),
            |tab| tab.resize(&t(1), Direction::Left, 2, SIZE).unwrap(),
            |tab| tab.spread(&t(1), SIZE).unwrap(),
            |tab| tab.split(&t(1), t(2), Axis::Rows, SIZE).unwrap(),
            |tab| assert!(tab.remove(&t(0), SIZE).unwrap()),
        ];
        for change in changes {
            let mut tab = split();
            tab.zoom(&t(1));
            change(&mut tab);
            assert!(!tab.zoomed());
        }
        let mut tab = split();
        tab.zoom(&t(1));
        assert_eq!(tab.border_at(34, 3, SIZE), None, "no border shows");
        // No terminal that way: the zoom stays.
        tab.focus_towards(&t(1), Direction::Up, SIZE);
        assert!(tab.zoomed());
    }

    #[test]
    fn closing_the_focused_terminal_focuses_the_one_focused_before_it() {
        let mut tab = split();
        tab.split(&t(1), t(2), Axis::Rows, SIZE).unwrap();
        tab.focus(&t(0));
        tab.focus(&t(2));
        assert!(tab.remove(&t(2), SIZE).unwrap());
        assert_eq!(tab.active(), &t(0));
        assert!(tab.remove(&t(0), SIZE).unwrap());
        assert_eq!(tab.active(), &t(1));
        assert!(!tab.remove(&t(1), SIZE).unwrap(), "the last one");
    }

    #[test]
    fn focus_moves_by_direction_to_the_one_focused_last() {
        let mut tab = split();
        tab.split(&t(1), t(2), Axis::Rows, SIZE).unwrap();
        tab.focus_towards(&t(2), Direction::Up, SIZE);
        assert_eq!(tab.active(), &t(1));
        tab.focus_towards(&t(1), Direction::Left, SIZE);
        assert_eq!(tab.active(), &t(0));
        // Back right: to %1, focused after %2.
        tab.focus_towards(&t(0), Direction::Right, SIZE);
        assert_eq!(tab.active(), &t(1));
    }
}
