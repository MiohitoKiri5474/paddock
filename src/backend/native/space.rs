//! One space on the native server: its tabs in order, the current one
//! and those current before it, the size its tabs are shown at, its
//! floats, and its terminals. Every change to a tab's layout ends with
//! [`Space::lay_out`], which sizes each terminal's PTY to the place it is
//! drawn in.

use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{FloatId, SpaceId, Split, Structure, TabId, TerminalId};
use anyhow::{anyhow, bail, Result};
use ratatui::layout::Rect;

/// A float as the server holds it: one terminal running a program, on
/// its key, never a tab. Its terminal keeps the size it was last given,
/// apart from the tabs'.
pub struct HostedFloat {
    pub id: FloatId,
    /// Its key, as [`crate::input::keymap::Key::name`] writes it.
    pub key: String,
    pub terminal: TerminalId,
}

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
    /// Its floats, oldest first.
    pub floats: Vec<HostedFloat>,
    /// Every terminal of its tabs and floats.
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
            floats: Vec::new(),
            terminals: vec![terminal],
        }
    }

    /// A space as `layout.json` brings it back: `tabs` (at least one),
    /// tab `current` current, its terminals `terminals`, shown at `size`.
    pub fn restore(
        id: SpaceId,
        name: String,
        root: String,
        size: (u16, u16),
        tabs: Vec<Tab>,
        current: usize,
        terminals: Vec<HostedTerminal>,
    ) -> Self {
        Self {
            id,
            name,
            root,
            size,
            current: current.min(tabs.len().saturating_sub(1)),
            tabs,
            previous: Vec::new(),
            floats: Vec::new(),
            terminals,
        }
    }

    /// The tabs current before the current one, the latest last.
    pub fn previous(&self) -> &[TabId] {
        &self.previous
    }

    /// The space with `previous` as the tabs current before (as
    /// [`Self::previous`] gave them), those it still has.
    pub fn with_previous(mut self, mut previous: Vec<TabId>) -> Self {
        previous.retain(|t| self.tab(t).is_some());
        self.previous = previous;
        self
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
            floats: self.floats.iter().map(|f| self.float_view(f)).collect(),
            float_active: false,
        }
    }

    /// Float `float` as the backend's interface shows it: its terminal
    /// fills it.
    fn float_view(&self, float: &HostedFloat) -> crate::backend::Float {
        let (width, height) = self.hosted(&float.terminal).map_or(self.size, |t| t.size);
        crate::backend::Float {
            id: float.id.clone(),
            key: float.key.clone(),
            splits: vec![Split {
                id: float.terminal.clone(),
                rect: Rect::new(0, 0, width, height),
            }],
            active: float.terminal.clone(),
        }
    }

    /// An error if the space has a float on `key` already: one per key.
    pub fn check_float_key(&self, key: &str) -> Result<()> {
        if self.floats.iter().any(|f| f.key == key) {
            bail!("{} has a float on {key} already", self.id);
        }
        Ok(())
    }

    /// Add `float`, whose program runs in `terminal`.
    pub fn push_float(&mut self, float: HostedFloat, terminal: HostedTerminal) {
        self.floats.push(float);
        self.terminals.push(terminal);
    }

    /// The float `float`.
    pub fn float(&self, float: &FloatId) -> Option<&HostedFloat> {
        self.floats.iter().find(|f| f.id == *float)
    }

    /// Note that `terminal`'s program has exited and been reaped, before
    /// its last output is read: it is never signalled again, and a float
    /// it ran goes at once, so its key is free for the next. True if a
    /// float went.
    pub fn reaped(&mut self, terminal: &TerminalId) -> bool {
        if let Some(t) = self.hosted_mut(terminal) {
            t.exited = true;
        }
        let before = self.floats.len();
        self.floats.retain(|f| f.terminal != *terminal);
        self.floats.len() != before
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

    pub fn hosted_mut(&mut self, terminal: &TerminalId) -> Option<&mut HostedTerminal> {
        self.terminals.iter_mut().find(|t| t.id == *terminal)
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

    /// Forget `terminal`, whose program has exited: its tab gives its
    /// place to the others, and goes with its last terminal; a float goes
    /// with its terminal. The terminal goes either way, as its program
    /// has. True if the space has no tab left: it ends, its floats too.
    pub fn remove(&mut self, terminal: &TerminalId) -> bool {
        self.floats.retain(|f| f.terminal != *terminal);
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
            let Some(term) = self.hosted_mut(&split.id) else {
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

#[cfg(test)]
mod tests {
    use super::super::pty::{self, Program, Pty};
    use super::super::terminal::Input;
    use super::*;

    /// Terminal `id` running `sleep 30` at `size`, and its PTY, whose
    /// program the test kills.
    fn hosted(id: &str, size: (u16, u16)) -> (HostedTerminal, Pty) {
        let env = [("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let dir = std::env::temp_dir();
        let pty = pty::spawn(
            "/bin/sh",
            Program::Command("sleep 30"),
            &dir,
            &dir,
            size,
            &env,
        )
        .unwrap();
        let id = TerminalId::new(id);
        let input = Input::start(&id, pty.master.try_clone().unwrap());
        (HostedTerminal::new(id, &pty, input, size).unwrap(), pty)
    }

    /// A space of one tab holding `%1`, at 80x24, with a float on `G`
    /// holding `%2`, at 40x10.
    fn with_a_float() -> (Space, Vec<Pty>) {
        let (tab_term, tab_pty) = hosted("%1", (80, 24));
        let tab = Tab::new(TabId::new("@1"), "t".into(), TerminalId::new("%1"));
        let mut space = Space::new(SpaceId::new("$1"), "s".into(), "/".into(), tab, tab_term);
        let (float_term, float_pty) = hosted("%2", (40, 10));
        let float = HostedFloat {
            id: FloatId::new("@2"),
            key: "G".into(),
            terminal: TerminalId::new("%2"),
        };
        space.push_float(float, float_term);
        (space, vec![tab_pty, float_pty])
    }

    fn kill(ptys: Vec<Pty>) {
        for mut pty in ptys {
            let _ = pty.child.kill();
            let _ = pty.child.wait();
        }
    }

    #[test]
    fn a_float_is_never_a_tab_and_shows_its_own_size() {
        let (space, ptys) = with_a_float();
        let s = space.structure();
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.floats.len(), 1);
        assert_eq!(s.floats[0].key, "G");
        assert_eq!(s.floats[0].active, "%2");
        assert_eq!(s.floats[0].splits[0].rect, Rect::new(0, 0, 40, 10));
        assert!(!s.float_active);
        assert!(space.check_float_key("G").is_err(), "one per key");
        assert!(space.check_float_key("H").is_ok());
        kill(ptys);
    }

    #[test]
    fn a_floats_key_is_free_once_its_program_is_reaped_before_its_output_is_read() {
        let (mut space, ptys) = with_a_float();
        let float = TerminalId::new("%2");
        assert!(space.reaped(&float), "the float went");
        assert!(space.check_float_key("G").is_ok(), "its key is free");
        assert!(space.structure().floats.is_empty());
        let term = space.hosted(&float).expect("still read until drained");
        assert!(term.exited, "never signalled again");
        assert!(!space.reaped(&TerminalId::new("%1")), "a tab's is no float");
        assert!(!space.remove(&float), "the tab is left");
        kill(ptys);
    }

    #[test]
    fn a_float_goes_with_its_program_and_the_space_with_its_last_tab() {
        let (mut space, ptys) = with_a_float();
        assert!(!space.remove(&TerminalId::new("%2")), "the tab is left");
        assert!(space.structure().floats.is_empty());
        assert_eq!(space.terminals.len(), 1);

        let (mut space, more) = with_a_float();
        assert!(space.remove(&TerminalId::new("%1")), "no tab left");
        kill(ptys.into_iter().chain(more).collect());
    }
}
