//! Sidebar items and their tabs, kept in tmux (ADR 0006): an item is a tmux
//! session, a tab one of its windows. paddock asks tmux to change them and
//! reads them back when tmux reports a change.

use super::dashboard::Dashboard;
use super::item::clamp;
use super::state::{is_session_id, next_name, App, CloseTarget, Mode, Renaming};
use crate::sessions::State;
use crate::terminal::Terminal;
use crate::tmux::server::quote;
use anyhow::{bail, Result};
use std::path::PathBuf;

/// New spaces are named `space-N` until the user renames them.
const ITEM_STEM: &str = "space-";

impl App {
    /// Start a shell in a new item at the end of the sidebar and make it
    /// current.
    pub fn new_item(&mut self) -> Result<()> {
        let sessions = self.server.sessions();
        let name = next_name(ITEM_STEM, sessions.iter().map(|s| s.name.as_str()));
        let dir = self.new_dir();
        let area = self.area();
        let session = self
            .server
            .new_session(&name, &dir, clamp((area.width, area.height)))?;
        if let Err(e) = self.attach(&session.id) {
            // Best effort: never leave a shell running out of sight.
            let _ = self.server.kill_session(&session.id);
            return Err(e);
        }
        self.current = self.items.len() - 1;
        Ok(())
    }

    /// Start a shell in a new tab of the current item and make it current.
    pub fn new_tab(&mut self) -> Result<()> {
        let Some(id) = self.current_item().map(|i| i.id.clone()) else {
            return Ok(());
        };
        let dir = self.new_dir();
        self.open_tab(&id, dir).map(drop)
    }

    /// Start a shell in `dir` in a new tab of item `id`, current in it.
    /// Returns its pane id.
    fn open_tab(&mut self, id: &str, dir: PathBuf) -> Result<String> {
        let pane = self.server.new_window(id, &dir)?;
        self.refresh(id)?;
        Ok(pane)
    }

    /// Read item `id`'s windows and panes again from tmux: new panes get a
    /// terminal (they start empty, so there is nothing to seed), ended ones
    /// lose theirs, and every terminal takes its pane's size.
    pub(super) fn refresh(&mut self, id: &str) -> Result<()> {
        let Some(i) = self.items.iter().position(|it| it.id == id) else {
            return Ok(());
        };
        let structure = self.server.structure(id)?;
        let mut kept = Vec::new();
        for pane in structure.windows.iter().flat_map(|w| &w.panes) {
            let size = clamp((pane.rect.width, pane.rect.height));
            self.terminals
                .entry(pane.id.clone())
                .or_insert_with(|| Terminal::new(size))
                .resize(size);
            kept.push(pane.id.as_str());
        }
        let gone: Vec<String> = self.items[i]
            .panes()
            .filter(|p| !kept.contains(p))
            .map(str::to_string)
            .collect();
        for pane in gone {
            self.terminals.remove(&pane);
        }
        self.items[i].structure = structure;
        Ok(())
    }

    /// Make tab `index` (0-based) of the current item current; missing tabs
    /// are ignored.
    pub fn select_tab(&mut self, index: usize) -> Result<()> {
        let Some(item) = self.current_item_mut() else {
            return Ok(());
        };
        let Some(window) = item.windows().get(index).map(|w| w.id.clone()) else {
            return Ok(());
        };
        // Show it at once; tmux confirms with a notification.
        item.structure.current = index;
        self.server.select(&window, None)
    }

    /// The next (`forward`) or previous tab of the current item, wrapping.
    pub fn cycle_tab(&mut self, forward: bool) -> Result<()> {
        let Some(item) = self.current_item() else {
            return Ok(());
        };
        let n = item.windows().len();
        if n == 0 {
            return Ok(());
        }
        let at = item.structure.current;
        let next = if forward { at + 1 } else { at + n - 1 } % n;
        self.select_tab(next)
    }

    /// Make item `index` current; its tabs show in the tab bar.
    pub fn select_item(&mut self, index: usize) {
        if index < self.items.len() {
            self.current = index;
        }
    }

    /// Bring pane `pane` to the screen: its item and tab become current and
    /// it gets the focus.
    pub fn jump_to(&mut self, pane: &str) {
        let Some(i) = self.item_of(pane) else {
            return;
        };
        self.current = i;
        let item = &mut self.items[i];
        let Some(w) = item.window_of(pane) else {
            return;
        };
        item.structure.current = w;
        let window = &mut item.structure.windows[w];
        window.active = pane.to_string();
        let id = window.id.clone();
        self.report(|app| app.server.select(&id, Some(pane)));
    }

    /// Rename the current tab or item in tmux, so the name survives
    /// reattaching. tmux may adjust a session name (it replaces `.` and `:`).
    pub fn rename(&mut self, target: Renaming, name: &str) -> Result<()> {
        let name = name.trim();
        let Some(item) = self.current_item() else {
            return Ok(());
        };
        if name.is_empty() {
            return Ok(());
        }
        let id = item.id.clone();
        match target {
            Renaming::Item => self.server.rename_session(&id, name)?,
            Renaming::Tab => match item.current_window() {
                Some(w) => self.server.rename_window(&w.id, name)?,
                None => return Ok(()),
            },
        }
        self.refresh(&id)
    }

    /// The current name of what `target` renames.
    pub fn name_of(&self, target: Renaming) -> String {
        let item = self.current_item();
        match target {
            Renaming::Item => item.map(|i| i.name().to_string()),
            Renaming::Tab => item
                .and_then(|i| i.current_window())
                .map(|w| w.name.clone()),
        }
        .unwrap_or_default()
    }

    /// End pane `pane`, first asking if Claude there is running or waiting.
    /// tmux closes its tab with its last pane, and its item with its last
    /// tab, and reports each. Asking from the dashboard `back` returns to it
    /// after.
    pub fn close(&mut self, pane: &str, back: Option<Dashboard>) -> Result<()> {
        match self.session_in(pane).map(|s| s.state) {
            Some(state @ (State::Running | State::Waiting)) => {
                self.mode = Mode::ConfirmClose {
                    target: CloseTarget::Pane(pane.to_string()),
                    state,
                    back,
                };
                Ok(())
            }
            _ => self.server.kill_pane(pane),
        }
    }

    /// End item `id` with all its tabs, first asking if Claude is running
    /// or waiting in any of its panes.
    pub fn close_item(&mut self, id: &str) -> Result<()> {
        let busy = self
            .rows()
            .into_iter()
            .filter(|r| r.item == id)
            .map(|r| r.state)
            .find(|s| matches!(s, State::Running | State::Waiting));
        match busy {
            Some(state) => {
                self.mode = Mode::ConfirmClose {
                    target: CloseTarget::Item(id.to_string()),
                    state,
                    back: None,
                };
                Ok(())
            }
            None => self.server.kill_session(id),
        }
    }

    /// Close what a close question was about, once the user said yes.
    pub(super) fn close_now(&mut self, target: &CloseTarget) -> Result<()> {
        match target {
            CloseTarget::Pane(pane) => self.server.kill_pane(pane),
            CloseTarget::Item(id) => self.server.kill_session(id),
        }
    }

    /// Continue a copy of the Claude session in pane `pane` in a new tab of
    /// the same item, in the session's directory: `claude --resume <id>
    /// --fork-session`, typed into the new shell so the tab stays when
    /// Claude exits.
    pub fn fork(&mut self, pane: &str) -> Result<()> {
        let Some((session_id, cwd)) = self
            .session_in(pane)
            .and_then(|s| Some((s.session_id.clone()?, s.cwd.clone())))
        else {
            bail!("no Claude session here");
        };
        // The id comes from a hook payload and is typed into a shell, where a
        // control character would end the line even inside quotes.
        if !is_session_id(&session_id) {
            bail!("unexpected Claude session id");
        }
        let Some(i) = self.item_of(pane) else {
            bail!("that terminal is closed");
        };
        let dir = cwd
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| self.dir_of(pane))
            .unwrap_or_else(|| self.start_dir.clone());
        let id = self.items[i].id.clone();
        let new = self.open_tab(&id, dir)?;
        self.current = i;
        let command = format!("claude --resume {} --fork-session\r", quote(&session_id));
        self.input(&new, command.as_bytes())
    }

    /// Forget an item whose tmux session ended. Returns whether any item is
    /// left.
    pub fn item_ended(&mut self, id: &str) -> bool {
        let Some(i) = self.items.iter().position(|it| it.id == id) else {
            return !self.items.is_empty();
        };
        let item = self.items.remove(i);
        for pane in item.panes() {
            self.terminals.remove(pane);
        }
        item.detach();
        if i < self.current || self.current >= self.items.len() {
            self.current = self.current.saturating_sub(1);
        }
        self.report(|app| app.save());
        !self.items.is_empty()
    }
}
