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

    /// Tab `i` as the backend's interface shows it.
    pub fn tab_view(&self, i: usize) -> crate::backend::Tab {
        let tab = &self.tabs[i];
        crate::backend::Tab {
            id: tab.id.clone(),
            name: tab.name().to_string(),
            auto_name: tab.auto_name(),
            splits: tab.splits(self.size),
            active: tab.active().clone(),
            zoomed: tab.zoomed(),
        }
    }

    pub fn structure(&self) -> Structure {
        Structure {
            name: self.name.clone(),
            tabs: (0..self.tabs.len()).map(|i| self.tab_view(i)).collect(),
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

    /// The index of the tab holding `terminal`; an error if none does.
    pub fn tab_index(&self, terminal: &TerminalId) -> Result<usize> {
        let i = self.tab_of(terminal);
        i.ok_or_else(|| anyhow!("no terminal {terminal} in {}", self.id))
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
    /// to the others, and goes with its last terminal. The terminal goes
    /// either way, as its shell has. True if the space has no tab left.
    pub fn remove(&mut self, terminal: &TerminalId) -> bool {
        if let Some(i) = self.tab_of(terminal) {
            match self.tabs[i].remove(terminal, self.size) {
                Ok(true) => self.lay_out_tab(i),
                Ok(false) => self.close_tab(i),
                Err(e) => eprintln!("paddock server: {e:#}"),
            }
        }
        self.terminals.retain(|t| t.id != *terminal);
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
        for i in 0..self.tabs.len() {
            self.lay_out_tab(i);
        }
    }

    /// Size the PTYs of tab `i`'s terminals to their places.
    pub fn lay_out_tab(&mut self, i: usize) {
        let Some(tab) = self.tabs.get(i) else {
            return;
        };
        for split in tab.splits(self.size) {
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
        for tab in &mut self.tabs {
            if !tab.auto_name() {
                continue;
            }
            let active = tab.active();
            let found = self.terminals.iter().find(|t| t.id == *active);
            if let Some(name) = found.and_then(HostedTerminal::auto_name) {
                changed |= tab.follow(name);
            }
        }
        changed
    }
}
