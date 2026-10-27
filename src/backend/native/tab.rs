//! One tab on the native server: its [`Layout`], which terminal has the
//! focus, whether it is zoomed, and the order its terminals last had the
//! focus in. What changes the layout gives the zoom up first, as tmux
//! does; only focusing the zoomed terminal itself keeps it.

use super::layout::{Found, Layout};
use crate::backend::{Split, TabId, TerminalId};
use crate::input::Direction;
use anyhow::Result;
use ratatui::layout::Rect;

pub struct Tab {
    pub id: TabId,
    pub name: String,
    /// Named after what runs in it: the user has not renamed it.
    pub auto_name: bool,
    layout: Layout,
    active: TerminalId,
    zoomed: bool,
    /// Its terminals, the one focused last at the end.
    focused: Vec<TerminalId>,
}

impl Tab {
    /// A tab of one terminal.
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
    /// their places, hidden.
    pub fn splits(&self, size: (u16, u16)) -> Vec<Split> {
        let area = area(size);
        let placed = self.layout.place(area);
        placed
            .into_iter()
            .map(|(id, rect)| {
                let rect = if self.zoomed && id == self.active {
                    area
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

    /// Split `target` for `new`, which takes the focus.
    pub fn split(
        &mut self,
        target: &TerminalId,
        new: TerminalId,
        columns: bool,
        size: (u16, u16),
    ) -> Result<()> {
        self.layout
            .split(target, new.clone(), columns, area(size))?;
        self.zoomed = false;
        self.focus(&new);
        Ok(())
    }

    /// Take `terminal` out; the terminal focused before it takes the
    /// focus. False if it was the last one: the tab goes with it.
    pub fn remove(&mut self, terminal: &TerminalId, size: (u16, u16)) -> Result<bool> {
        if !self.layout.remove(terminal, area(size))? {
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
        let next = self.layout.neighbour(terminal, dir, area(size), rank);
        self.zoomed = false;
        if let Some(next) = next {
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
        self.layout.resize(terminal, dir, cells, area(size))
    }

    /// Size the cell ending in `terminal` (see [`Layout::resize_to`]).
    pub fn resize_to(
        &mut self,
        terminal: &TerminalId,
        columns: bool,
        cells: u16,
        size: (u16, u16),
    ) -> Result<()> {
        self.zoomed = false;
        self.layout.resize_to(terminal, columns, cells, area(size))
    }

    /// Even out `terminal` and its neighbours (see [`Layout::spread`]).
    pub fn spread(&mut self, terminal: &TerminalId, size: (u16, u16)) -> Result<()> {
        self.zoomed = false;
        self.layout.spread(terminal, area(size))
    }

    /// The border at `(x, y)` of the tab at `size`: none while zoomed,
    /// as none is drawn.
    pub fn border_at(&self, x: u16, y: u16, size: (u16, u16)) -> Option<Found> {
        if self.zoomed {
            return None;
        }
        self.layout.border_at(area(size), x, y)
    }
}

/// The tab's area at `(cols, rows)`.
fn area((cols, rows): (u16, u16)) -> Rect {
    Rect::new(0, 0, cols, rows)
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
        tab.split(&t(0), t(1), true, SIZE).unwrap();
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
            |tab| tab.split(&t(1), t(2), false, SIZE).unwrap(),
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
    }

    #[test]
    fn closing_the_focused_terminal_focuses_the_one_focused_before_it() {
        let mut tab = split();
        tab.split(&t(1), t(2), false, SIZE).unwrap();
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
        tab.split(&t(1), t(2), false, SIZE).unwrap();
        tab.focus_towards(&t(2), Direction::Up, SIZE);
        assert_eq!(tab.active(), &t(1));
        tab.focus_towards(&t(1), Direction::Left, SIZE);
        assert_eq!(tab.active(), &t(0));
        // Back right: to %1, focused after %2.
        tab.focus_towards(&t(0), Direction::Right, SIZE);
        assert_eq!(tab.active(), &t(1));
    }
}
