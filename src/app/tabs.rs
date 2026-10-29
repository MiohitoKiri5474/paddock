//! Sidebar spaces and their tabs, kept in the backend (ADR 0006). paddock
//! asks the backend to change them and reads them back when it reports a
//! change.

use super::rows::{EXTERNAL_CLOSE, EXTERNAL_FORK, EXTERNAL_OPEN};
use super::space::clamp;
use super::state::{next_name, App, CloseTarget, Mode, NameTarget, Renamed};
use super::watch;
use crate::backend::{OtherVersion, SpaceId, TerminalId};
use crate::paths::HomePaths;
use crate::sessions::{is_session_id, AgentKind, State};
use crate::shell::quote;
use crate::terminal::Terminal;
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// New spaces are named `space-N` until the user renames them.
const SPACE_STEM: &str = "space-";

/// What acting on a row says when its terminal closed meanwhile.
const TERMINAL_CLOSED: &str = "that terminal is closed";

/// How long paddock waits for an upgraded server to answer (ADR 0009).
const REATTACH_WAIT: Duration = Duration::from_secs(10);

impl App {
    /// The name a new space gets unless the user picks one: the first free
    /// `space-N`.
    pub fn new_space_name(&self) -> String {
        let spaces = self.backend.spaces();
        next_name(SPACE_STEM, spaces.iter().map(|s| s.name.as_str()))
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
        self.new_space_in(name, dir)
    }

    /// Start a shell in `dir` in a new space named `name` at the end of
    /// the sidebar and make it current.
    pub fn new_space_in(&mut self, name: &str, dir: PathBuf) -> Result<()> {
        let area = self.area();
        let space = self
            .backend
            .new_space(name, &dir, clamp((area.width, area.height)))?;
        if let Err(e) = self.attach(&space) {
            // Best effort: never leave a shell running out of sight.
            let _ = self.backend.close_space(&space);
            return Err(e);
        }
        self.current = self.spaces.len() - 1;
        Ok(())
    }

    /// Attach every space again once the server has handed its terminals
    /// to a new binary (ADR 0009): the same spaces in the same order, the
    /// same one current, each with a fresh connection and screens rebuilt
    /// from the new server's snapshots (which keeps each space's current
    /// tab and focus). An error if the new server cannot be talked to (it
    /// speaks another protocol version, or failed).
    pub(super) fn reattach(&mut self) -> Result<()> {
        self.upgrading = None;
        // What the old connections still report is left out, until their
        // end: their spaces are attached again below. One that ended in
        // the upgrade reports nothing more.
        let lost = std::mem::take(&mut self.upgrade_lost);
        for space in &self.spaces {
            space.close();
            if !lost.contains(&space.connection_id) {
                self.replaced.insert(space.connection_id);
            }
        }
        // The old binary, until its `exec`, still takes connections, and
        // closes them unanswered: ask until the new one answers, its hello
        // and then its spaces (a closed connection is no answer).
        let start = Instant::now();
        let running = loop {
            let asked = self
                .backend
                .prepare()
                .and_then(|()| self.backend.upgraded_spaces());
            match asked {
                Ok(running) => break running,
                Err(e) if e.downcast_ref::<OtherVersion>().is_some() => return Err(e),
                Err(e) if start.elapsed() > REATTACH_WAIT => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let current = self.spaces.get(self.current).map(|s| s.id.clone());
        let ids: Vec<SpaceId> = self.spaces.iter().map(|s| s.id.clone()).collect();
        self.spaces.clear();
        self.terminals.clear();
        // A server that ends with its last space is gone: the new binary
        // failed to take over, and its programs ended with it.
        if running.is_empty() {
            bail!(
                "the new paddock server did not take over and has ended, with every program \
                 in it (see server.log in paddock's state directory)"
            );
        }
        for id in ids.iter().filter(|id| running.iter().any(|s| s.id == **id)) {
            self.attach(id)?;
        }
        self.attach_new_spaces();
        let found = current.and_then(|c| self.spaces.iter().position(|s| s.id == c));
        self.current = found.unwrap_or(0);
        Ok(())
    }

    /// Attach, at the end of the sidebar, the spaces made in the backend by
    /// someone else (`paddock worktree new`, or by hand); the
    /// current space stays. Every space's connection hears of a new space,
    /// so one already attached is skipped. A space that ended before it
    /// could be attached is no news.
    pub(super) fn attach_new_spaces(&mut self) {
        let known: Vec<SpaceId> = self.spaces.iter().map(|s| s.id.clone()).collect();
        for space in self.backend.spaces() {
            if known.contains(&space.id) {
                continue;
            }
            if let Err(e) = self.attach(&space.id) {
                let alive = self.backend.spaces().iter().any(|s| s.id == space.id);
                if alive {
                    self.notice = Some(format!("{e:#}"));
                }
            }
        }
    }

    /// Start a shell in a new tab of the current space and make it current.
    pub fn new_tab(&mut self) -> Result<()> {
        let Some(id) = self.current_space().map(|i| i.id.clone()) else {
            return Ok(());
        };
        let dir = self.tab_dir();
        self.open_tab(&id, dir).map(drop)
    }

    /// Start a shell in `dir` in a new tab of space `id`, current in it.
    /// Returns its terminal.
    fn open_tab(&mut self, id: &SpaceId, dir: PathBuf) -> Result<TerminalId> {
        let pane = self.backend.new_tab(id, &dir)?;
        self.refresh(id)?;
        Ok(pane)
    }

    /// [`Self::refresh`] after the backend reported a change, showing a
    /// failure as a notice unless the space has just ended (its end follows
    /// and takes the space away).
    pub(super) fn refresh_reported(&mut self, id: &SpaceId) {
        if let Err(e) = self.refresh(id) {
            let alive = self.backend.spaces().iter().any(|s| s.id == *id);
            if let Some(message) = refresh_failure(&e, alive) {
                self.notice = Some(message);
            }
        }
    }

    /// Read space `id`'s tabs and splits again from the backend: new
    /// terminals get an emulator (they start empty, so there is nothing to
    /// replay), ended ones lose theirs, and every emulator takes its
    /// terminal's size. Floats' terminals count too; a float that ended is
    /// no longer shown.
    pub(super) fn refresh(&mut self, id: &SpaceId) -> Result<()> {
        let Some(i) = self.spaces.iter().position(|it| it.id == *id) else {
            return Ok(());
        };
        let structure = self.backend.structure(id)?;
        if structure.tabs.is_empty() && !structure.floats.is_empty() {
            // The last tab closed and only floats keep the space: the
            // space closes with its last tab, its floats with it.
            return self.backend.close_space(id);
        }
        if structure.float_active {
            // A float is never the current tab: the backend made it
            // current when the current tab closed, so show the tab before it.
            if let Some(tab) = structure.tabs.get(structure.current) {
                self.backend.select(&tab.id, None)?;
            }
        }
        self.remember_float_panes(&structure);
        let mut kept = Vec::new();
        for split in structure.splits() {
            let size = clamp((split.rect.width, split.rect.height));
            self.terminals
                .entry(split.id.clone())
                .or_insert_with(|| Terminal::new(size))
                .resize(size);
            kept.push(&split.id);
        }
        let gone: Vec<TerminalId> = self.spaces[i]
            .terminals()
            .filter(|t| !kept.contains(t))
            .cloned()
            .collect();
        for terminal in gone {
            self.terminals.remove(&terminal);
        }
        self.spaces[i].structure = structure;
        self.forget_ended_float_noting_quick_exit();
        Ok(())
    }

    /// Make tab `index` (0-based) of the current space current; missing tabs
    /// are ignored.
    pub fn select_tab(&mut self, index: usize) -> Result<()> {
        let Some(space) = self.current_space_mut() else {
            return Ok(());
        };
        let Some(tab) = space.tabs().get(index).map(|t| t.id.clone()) else {
            return Ok(());
        };
        // Show it at once; the backend confirms with an event.
        space.structure.current = index;
        self.backend.select(&tab, None)
    }

    /// The next (`forward`) or previous tab of the current space, wrapping.
    pub fn cycle_tab(&mut self, forward: bool) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        let n = space.tabs().len();
        if n == 0 {
            return Ok(());
        }
        let at = space.structure.current;
        let next = if forward { at + 1 } else { at + n - 1 } % n;
        self.select_tab(next)
    }

    /// Move the current tab `step` places (`-1` left, `1` right) within its
    /// space, swapping it with its neighbour in the backend; at either end nothing
    /// moves.
    pub fn move_tab(&mut self, step: isize) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        let at = space.structure.current;
        let Some(to) = at
            .checked_add_signed(step)
            .filter(|&i| i < space.tabs().len())
        else {
            return Ok(());
        };
        let (from, to_id) = (space.tabs()[at].id.clone(), space.tabs()[to].id.clone());
        let id = space.id.clone();
        self.backend.swap_tabs(&from, &to_id)?;
        self.refresh(&id)
    }

    /// Make space `index` current; its tabs show in the tab bar.
    pub fn select_space(&mut self, index: usize) {
        if index < self.spaces.len() {
            self.current = index;
        }
    }

    /// Bring the terminal a session row names to the screen: its space and
    /// tab become current and it gets the focus.
    pub fn jump_to(&mut self, pane: &str) {
        let terminal = match self.row_terminal(pane, EXTERNAL_OPEN) {
            Ok(Some(terminal)) => terminal,
            Ok(None) => return,
            Err(e) => {
                self.notice = Some(e.to_string());
                return;
            }
        };
        let Some(i) = self.space_of(&terminal) else {
            return;
        };
        self.current = i;
        let space = &mut self.spaces[i];
        let Some(t) = space.tab_of(&terminal) else {
            return;
        };
        space.structure.current = t;
        let tab = &mut space.structure.tabs[t];
        tab.focus(&terminal);
        let id = tab.id.clone();
        self.report(|app| app.backend.select(&id, Some(&terminal)));
    }

    /// Act on a name typed into the prompt for `target`: rename the current
    /// tab or space, start a new space (a blank name keeps the default, or
    /// the old name), or check a branch out in a new worktree.
    pub fn submit_name(&mut self, target: &NameTarget, name: &str) -> Result<()> {
        let name = name.trim();
        match target {
            NameTarget::NewSpace if name.is_empty() => self.new_space(),
            NameTarget::NewSpace => self.new_space_named(name),
            NameTarget::Rename(_) if name.is_empty() => Ok(()),
            NameTarget::Rename(what) => self.rename(*what, name),
            NameTarget::NewWorktree(_) if name.is_empty() => Ok(()),
            NameTarget::NewWorktree(tree) => self.new_worktree(tree, name),
        }
    }

    /// Rename the current tab or space in the backend, so the name survives
    /// reattaching. The backend may adjust a space name (tmux replaces `.`
    /// and `:`).
    fn rename(&mut self, what: Renamed, name: &str) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        let id = space.id.clone();
        match what {
            Renamed::Space => self.backend.rename_space(&id, name)?,
            Renamed::Tab => match space.current_tab() {
                Some(tab) => self.backend.rename_tab(&tab.id, name)?,
                None => return Ok(()),
            },
        }
        self.refresh(&id)
    }

    /// `C-a R`: make the focused terminal's directory the current space's
    /// root, saying so in a notice.
    pub fn set_root(&mut self) -> Result<()> {
        let (Some(pane), Some(space)) = (self.current_terminal(), self.current_space()) else {
            return Ok(());
        };
        let (id, name) = (space.id.clone(), space.name().to_string());
        let Some(dir) = self.dir_of(&pane) else {
            bail!("the focused terminal's directory is unknown: the space root stays");
        };
        self.backend.set_root(&id, &dir)?;
        let root = dir.to_string_lossy();
        self.notice = Some(format!("{name}'s root: {}", HomePaths::new().show(&root)));
        // Read again now, so the finder is never stale, even for a space
        // whose place the background read has not reached yet.
        // The branch's status is not read here, on the UI thread: the last
        // one stays until the background read brings a new one, unless the
        // directory or branch changed, which it no longer describes.
        if let Some(place) = watch::place(&self.backend, &id) {
            let status = self
                .places
                .get(&id)
                .filter(|old| old.same_checkout(&place))
                .and_then(|old| old.status);
            self.places.insert(id, watch::Place { status, ..place });
        }
        Ok(())
    }

    /// What the prompt for `target` starts with: the current name, the
    /// default for a new space, or nothing for a branch.
    pub fn name_of(&self, target: &NameTarget) -> String {
        let space = self.current_space();
        match target {
            NameTarget::Rename(Renamed::Space) => space.map(|i| i.name().to_string()),
            NameTarget::Rename(Renamed::Tab) => {
                space.and_then(|i| i.current_tab()).map(|t| t.name.clone())
            }
            NameTarget::NewSpace => Some(self.new_space_name()),
            NameTarget::NewWorktree(_) => None,
        }
        .unwrap_or_default()
    }

    /// End the terminal a session row names, first asking if an agent
    /// there is running or waiting.
    /// The backend closes its tab with its last terminal, and its space
    /// with its last tab, and reports each.
    pub fn close(&mut self, pane: &str) -> Result<()> {
        let Some(terminal) = self.row_terminal(pane, EXTERNAL_CLOSE)? else {
            bail!(TERMINAL_CLOSED);
        };
        match self.session_in(pane).map(|s| s.state) {
            Some(state @ (State::Running | State::Waiting)) => {
                self.ask_close(CloseTarget::Terminal(terminal), state);
                Ok(())
            }
            _ => self.backend.close_terminal(&terminal),
        }
    }

    /// End space `id` with all its tabs, first asking if an agent is
    /// running or waiting in any of its panes.
    pub fn close_space(&mut self, id: &SpaceId) -> Result<()> {
        let busy = self
            .rows()
            .into_iter()
            .filter(|r| r.space == id.as_str())
            .map(|r| r.state)
            .find(|s| matches!(s, State::Running | State::Waiting));
        match busy {
            Some(state) => {
                self.ask_close(CloseTarget::Space(id.clone()), state);
                Ok(())
            }
            None => self.backend.close_space(id),
        }
    }

    /// Ask before closing `target`, where an agent is `state`. A panel with
    /// the keyboard (the sidebar or the dashboard) gets it back after.
    pub(super) fn ask_close(&mut self, target: CloseTarget, state: State) {
        let back = std::mem::replace(&mut self.mode, Mode::Normal).into_panel();
        self.mode = Mode::ConfirmClose {
            target,
            state,
            back,
        };
    }

    /// Close what a close question was about, once the user said yes.
    pub(super) fn close_now(&mut self, target: &CloseTarget) -> Result<()> {
        match target {
            CloseTarget::Terminal(terminal) => self.backend.close_terminal(terminal),
            CloseTarget::Space(id) => self.backend.close_space(id),
            CloseTarget::Worktree { target, confirmed } => {
                self.remove_worktree_space(target, confirmed.as_ref())
            }
        }
    }

    /// Continue a copy of the Claude session in pane `pane` in a new tab of
    /// the same space, in the session's directory: `claude --resume <id>
    /// --fork-session`, typed into the new shell so the tab stays when
    /// Claude exits.
    pub fn fork(&mut self, pane: &str) -> Result<()> {
        let terminal = self.row_terminal(pane, EXTERNAL_FORK)?;
        let session = self.session_in(pane);
        // Only Claude continues a copy of a conversation.
        if session.is_some_and(|s| s.kind != AgentKind::Claude) {
            bail!("fork needs a Claude session");
        }
        let Some((session_id, cwd)) =
            session.and_then(|s| Some((s.session_id.clone()?, s.cwd.clone())))
        else {
            bail!("no Claude session here");
        };
        // The id comes from a hook payload and is typed into a shell, where a
        // control character would end the line even inside quotes.
        if !is_session_id(&session_id) {
            bail!("unexpected Claude session id");
        }
        let Some((i, terminal)) = terminal.and_then(|t| Some((self.space_of(&t)?, t))) else {
            bail!(TERMINAL_CLOSED);
        };
        let dir = match cwd.map(PathBuf::from).filter(|p| p.is_dir()) {
            Some(dir) => dir,
            None => self
                .dir_of(&terminal)
                .unwrap_or_else(|| self.start_dir.clone()),
        };
        let id = self.spaces[i].id.clone();
        let new = self.open_tab(&id, dir)?;
        self.current = i;
        let command = format!("claude --resume {} --fork-session\r", quote(&session_id));
        self.input(&new, command.as_bytes())
    }

    /// Forget a space that ended. Returns whether any space is left.
    pub fn space_ended(&mut self, id: &SpaceId) -> bool {
        let Some(i) = self.spaces.iter().position(|it| it.id == *id) else {
            return !self.spaces.is_empty();
        };
        let space = self.spaces.remove(i);
        for pane in space.terminals() {
            self.terminals.remove(pane);
        }
        self.forget_ended_float_noting_quick_exit();
        space.detach();
        if i < self.current || self.current >= self.spaces.len() {
            self.current = self.current.saturating_sub(1);
        }
        self.report(|app| app.save());
        !self.spaces.is_empty()
    }
}

/// What to tell the user when reading space structure failed with `e`:
/// nothing if the space is gone (`alive` false), as its end is on the way.
fn refresh_failure(e: &anyhow::Error, alive: bool) -> Option<String> {
    alive.then(|| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_refresh_is_reported_unless_its_space_just_ended() {
        let e = anyhow::anyhow!("no server");
        assert_eq!(refresh_failure(&e, true).as_deref(), Some("no server"));
        // Its end is on the way and takes the space away.
        assert_eq!(refresh_failure(&e, false), None);
    }
}
