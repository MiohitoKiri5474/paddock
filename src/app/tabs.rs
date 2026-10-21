//! Sidebar spaces and their tabs, kept in tmux (ADR 0006): a space is a tmux
//! session, a tab one of its windows. paddock asks tmux to change them and
//! reads them back when tmux reports a change.

use super::dashboard::Dashboard;
use super::space::clamp;
use super::state::{is_session_id, next_name, App, CloseTarget, Mode, Naming};
use crate::sessions::State;
use crate::terminal::Terminal;
use crate::tmux::server::quote;
use anyhow::{bail, Result};
use std::path::PathBuf;

/// New spaces are named `space-N` until the user renames them.
const SPACE_STEM: &str = "space-";

impl App {
    /// The name a new space gets unless the user picks one: the first free
    /// `space-N`.
    pub fn new_space_name(&self) -> String {
        let sessions = self.server.sessions();
        next_name(SPACE_STEM, sessions.iter().map(|s| s.name.as_str()))
    }

    /// Start a shell in a new space named [`Self::new_space_name`].
    pub fn new_space(&mut self) -> Result<()> {
        let name = self.new_space_name();
        self.new_space_named(&name)
    }

    /// Start a shell in a new space named `name` at the end of the sidebar
    /// and make it current.
    pub fn new_space_named(&mut self, name: &str) -> Result<()> {
        let dir = self.new_dir();
        let area = self.area();
        let session = self
            .server
            .new_session(name, &dir, clamp((area.width, area.height)))?;
        if let Err(e) = self.attach(&session.id) {
            // Best effort: never leave a shell running out of sight.
            let _ = self.server.kill_session(&session.id);
            return Err(e);
        }
        self.current = self.spaces.len() - 1;
        Ok(())
    }

    /// Start a shell in a new tab of the current space and make it current.
    pub fn new_tab(&mut self) -> Result<()> {
        let Some(id) = self.current_space().map(|i| i.id.clone()) else {
            return Ok(());
        };
        let dir = self.new_dir();
        self.open_tab(&id, dir).map(drop)
    }

    /// Start a shell in `dir` in a new tab of space `id`, current in it.
    /// Returns its pane id.
    fn open_tab(&mut self, id: &str, dir: PathBuf) -> Result<String> {
        let pane = self.server.new_window(id, &dir)?;
        self.refresh(id)?;
        Ok(pane)
    }

    /// [`Self::refresh`] after tmux reported a change, showing a failure as
    /// a notice unless the space has just ended (its exit follows and takes
    /// the space away).
    pub(super) fn refresh_reported(&mut self, id: &str) {
        if let Err(e) = self.refresh(id) {
            let alive = self.server.sessions().iter().any(|s| s.id == id);
            if let Some(message) = refresh_failure(&e, alive) {
                self.notice = Some(message);
            }
        }
    }

    /// Read space `id`'s windows and panes again from tmux: new panes get a
    /// terminal (they start empty, so there is nothing to seed), ended ones
    /// lose theirs, and every terminal takes its pane's size.
    pub(super) fn refresh(&mut self, id: &str) -> Result<()> {
        let Some(i) = self.spaces.iter().position(|it| it.id == id) else {
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
        let gone: Vec<String> = self.spaces[i]
            .panes()
            .filter(|p| !kept.contains(p))
            .map(str::to_string)
            .collect();
        for pane in gone {
            self.terminals.remove(&pane);
        }
        self.spaces[i].structure = structure;
        Ok(())
    }

    /// Make tab `index` (0-based) of the current space current; missing tabs
    /// are ignored.
    pub fn select_tab(&mut self, index: usize) -> Result<()> {
        let Some(space) = self.current_space_mut() else {
            return Ok(());
        };
        let Some(window) = space.windows().get(index).map(|w| w.id.clone()) else {
            return Ok(());
        };
        // Show it at once; tmux confirms with a notification.
        space.structure.current = index;
        self.server.select(&window, None)
    }

    /// The next (`forward`) or previous tab of the current space, wrapping.
    pub fn cycle_tab(&mut self, forward: bool) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        let n = space.windows().len();
        if n == 0 {
            return Ok(());
        }
        let at = space.structure.current;
        let next = if forward { at + 1 } else { at + n - 1 } % n;
        self.select_tab(next)
    }

    /// Move the current tab `step` places (`-1` left, `1` right) within its
    /// space, swapping it with its neighbour in tmux; at either end nothing
    /// moves.
    pub fn move_tab(&mut self, step: isize) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        let at = space.structure.current;
        let Some(to) = at
            .checked_add_signed(step)
            .filter(|&i| i < space.windows().len())
        else {
            return Ok(());
        };
        let (from, to_id) = (
            space.windows()[at].id.clone(),
            space.windows()[to].id.clone(),
        );
        let id = space.id.clone();
        self.server.swap_windows(&from, &to_id)?;
        self.refresh(&id)
    }

    /// Make space `index` current; its tabs show in the tab bar.
    pub fn select_space(&mut self, index: usize) {
        if index < self.spaces.len() {
            self.current = index;
        }
    }

    /// Bring pane `pane` to the screen: its space and tab become current and
    /// it gets the focus.
    pub fn jump_to(&mut self, pane: &str) {
        let Some(i) = self.space_of(pane) else {
            return;
        };
        self.current = i;
        let space = &mut self.spaces[i];
        let Some(w) = space.window_of(pane) else {
            return;
        };
        space.structure.current = w;
        let window = &mut space.structure.windows[w];
        window.active = pane.to_string();
        let id = window.id.clone();
        self.report(|app| app.server.select(&id, Some(pane)));
    }

    /// Act on a name typed into the prompt for `target`: rename the current
    /// tab or space, or start a new space (a blank name keeps the default).
    pub fn name(&mut self, target: Naming, name: &str) -> Result<()> {
        let name = name.trim();
        match target {
            Naming::NewSpace if name.is_empty() => self.new_space(),
            Naming::NewSpace => self.new_space_named(name),
            Naming::Tab | Naming::Space => self.rename(target, name),
        }
    }

    /// Rename the current tab or space in tmux, so the name survives
    /// reattaching. tmux may adjust a session name (it replaces `.` and `:`).
    fn rename(&mut self, target: Naming, name: &str) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        if name.is_empty() {
            return Ok(());
        }
        let id = space.id.clone();
        match target {
            Naming::Space => self.server.rename_session(&id, name)?,
            Naming::Tab => match space.current_window() {
                Some(w) => self.server.rename_window(&w.id, name)?,
                None => return Ok(()),
            },
            Naming::NewSpace => return Ok(()),
        }
        self.refresh(&id)
    }

    /// What the prompt for `target` starts with: the current name, or the
    /// default for a new space.
    pub fn name_of(&self, target: Naming) -> String {
        let space = self.current_space();
        match target {
            Naming::Space => space.map(|i| i.name().to_string()),
            Naming::Tab => space
                .and_then(|i| i.current_window())
                .map(|w| w.name.clone()),
            Naming::NewSpace => Some(self.new_space_name()),
        }
        .unwrap_or_default()
    }

    /// End pane `pane`, first asking if Claude there is running or waiting.
    /// tmux closes its tab with its last pane, and its space with its last
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

    /// End space `id` with all its tabs, first asking if Claude is running
    /// or waiting in any of its panes.
    pub fn close_space(&mut self, id: &str) -> Result<()> {
        let busy = self
            .rows()
            .into_iter()
            .filter(|r| r.space == id)
            .map(|r| r.state)
            .find(|s| matches!(s, State::Running | State::Waiting));
        match busy {
            Some(state) => {
                self.mode = Mode::ConfirmClose {
                    target: CloseTarget::Space(id.to_string()),
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
            CloseTarget::Space(id) => self.server.kill_session(id),
        }
    }

    /// Continue a copy of the Claude session in pane `pane` in a new tab of
    /// the same space, in the session's directory: `claude --resume <id>
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
        let Some(i) = self.space_of(pane) else {
            bail!("that terminal is closed");
        };
        let dir = cwd
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| self.dir_of(pane))
            .unwrap_or_else(|| self.start_dir.clone());
        let id = self.spaces[i].id.clone();
        let new = self.open_tab(&id, dir)?;
        self.current = i;
        let command = format!("claude --resume {} --fork-session\r", quote(&session_id));
        self.input(&new, command.as_bytes())
    }

    /// Forget a space whose tmux session ended. Returns whether any space is
    /// left.
    pub fn space_ended(&mut self, id: &str) -> bool {
        let Some(i) = self.spaces.iter().position(|it| it.id == id) else {
            return !self.spaces.is_empty();
        };
        let space = self.spaces.remove(i);
        for pane in space.panes() {
            self.terminals.remove(pane);
        }
        space.detach();
        if i < self.current || self.current >= self.spaces.len() {
            self.current = self.current.saturating_sub(1);
        }
        self.report(|app| app.save());
        !self.spaces.is_empty()
    }
}

/// What to tell the user when reading space structure failed with `e`:
/// nothing if the space is gone (`alive` false), as its exit is on the way.
fn refresh_failure(e: &anyhow::Error, alive: bool) -> Option<String> {
    alive.then(|| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_refresh_is_reported_unless_its_space_just_ended() {
        let e = anyhow::anyhow!("tmux list-panes: no server");
        assert_eq!(
            refresh_failure(&e, true).as_deref(),
            Some("tmux list-panes: no server")
        );
        // Its exit is on the way and takes the space away.
        assert_eq!(refresh_failure(&e, false), None);
    }
}
