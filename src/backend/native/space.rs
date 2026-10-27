//! One space on the native server: its tabs in order, the current one and
//! those current before it, the size its tabs are shown at, and its
//! terminals. Every change to a tab's layout ends with [`Space::lay_out`],
//! which sizes each terminal's PTY to the place it is drawn in.

use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{SpaceId, Structure, TabId, TerminalId};
use anyhow::{anyhow, Result};

pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub root: String,
    /// The size its tabs are shown at.
    pub size: (u16, u16),
    pub tabs: Vec<Tab>,
    /// Index of the current tab.
    pub current: usize,
    /// The tabs current before, the latest last: closing the current tab
    /// goes back to the latest of them, as tmux does.
    previous: Vec<TabId>,
    /// Every terminal of its tabs.
    pub terminals: Vec<HostedTerminal>,
}

impl Space {
    /// A space of one tab holding `terminal`, shown at its size.
    pub fn new(
        id: SpaceId,
        name: String,
        root: String,
        tab: Tab,
        terminal: HostedTerminal,
    ) -> Self {
        Self {
            id,
            name,
            root,
            size: terminal.size,
            tabs: vec![tab],
            current: 0,
            previous: Vec::new(),
            terminals: vec![terminal],
        }
    }

    pub fn structure(&self) -> Structure {
        let tabs = self.tabs.iter().map(|tab| crate::backend::Tab {
            id: tab.id.clone(),
            name: tab.name.clone(),
            auto_name: tab.auto_name,
            splits: tab.splits(self.size),
            active: tab.active().clone(),
            zoomed: tab.zoomed(),
        });
        Structure {
            name: self.name.clone(),
            tabs: tabs.collect(),
            current: self.current,
            floats: Vec::new(),
            float_active: false,
        }
    }

    pub fn tab(&self, tab: &TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == *tab)
    }

    /// The tab holding `terminal`.
    pub fn tab_of(&self, terminal: &TerminalId) -> Option<usize> {
        self.tabs.iter().position(|t| t.contains(terminal))
    }

    pub fn hosted(&self, terminal: &TerminalId) -> Option<&HostedTerminal> {
        self.terminals.iter().find(|t| t.id == *terminal)
    }

    /// The tab holding `terminal`, to change.
    pub fn tab_mut_of(&mut self, terminal: &TerminalId) -> Result<&mut Tab> {
        let i = self.tab_of(terminal);
        let i = i.ok_or_else(|| anyhow!("no terminal {terminal} in {}", self.id))?;
        Ok(&mut self.tabs[i])
    }

    /// Make tab `i` the current one.
    pub fn select(&mut self, i: usize) {
        if i == self.current || i >= self.tabs.len() {
            return;
        }
        let was = self.tabs[self.current].id.clone();
        self.previous.retain(|t| *t != was);
        self.previous.push(was);
        self.current = i;
    }

    /// Swap tab `a` with tab `b`; the current tab stays current.
    pub fn swap(&mut self, a: usize, b: usize) {
        let current = self.tabs[self.current].id.clone();
        self.tabs.swap(a, b);
        self.current = self.tab(&current).unwrap_or(self.current);
    }

    /// Add `tab` at the end, holding `terminal`, and make it current.
    pub fn push_tab(&mut self, tab: Tab, terminal: HostedTerminal) {
        self.tabs.push(tab);
        self.terminals.push(terminal);
        self.select(self.tabs.len() - 1);
    }

    /// Forget `terminal`, whose shell has exited: its tab gives its place
    /// to the others, and goes with its last terminal. True if the space
    /// has no tab left.
    pub fn remove(&mut self, terminal: &TerminalId) -> bool {
        self.terminals.retain(|t| t.id != *terminal);
        let Some(i) = self.tab_of(terminal) else {
            return self.tabs.is_empty();
        };
        match self.tabs[i].remove(terminal, self.size) {
            Ok(true) => {}
            Ok(false) => self.close_tab(i),
            Err(e) => eprintln!("paddock server: {e:#}"),
        }
        self.tabs.is_empty()
    }

    /// Take tab `i` out. If it was current, the latest tab current before
    /// it takes over, else the one before it, else the one after.
    fn close_tab(&mut self, i: usize) {
        let gone = self.tabs.remove(i).id;
        self.previous.retain(|t| *t != gone);
        if self.tabs.is_empty() {
            self.current = 0;
            return;
        }
        if i == self.current {
            let back = self.previous.pop().and_then(|t| self.tab(&t));
            self.current = back.unwrap_or(i.saturating_sub(1));
        } else if i < self.current {
            self.current -= 1;
        }
    }

    /// Size every terminal's PTY to its place in its tab.
    pub fn lay_out(&mut self) {
        let places: Vec<_> = self.tabs.iter().flat_map(|t| t.splits(self.size)).collect();
        for split in places {
            let Some(term) = self.terminals.iter_mut().find(|t| t.id == split.id) else {
                continue;
            };
            if let Err(e) = term.resize((split.rect.width, split.rect.height)) {
                eprintln!("paddock server: {e:#}");
            }
        }
    }

    /// Name each tab that follows what runs in it after its focused
    /// terminal. True if a name changed.
    pub fn follow_names(&mut self) -> bool {
        let mut changed = false;
        for tab in self.tabs.iter_mut().filter(|t| t.auto_name) {
            let active = tab.active();
            let found = self.terminals.iter().find(|t| t.id == *active);
            let Some(name) = found.and_then(HostedTerminal::auto_name) else {
                continue;
            };
            if tab.name != name {
                tab.name = name;
                changed = true;
            }
        }
        changed
    }
}
