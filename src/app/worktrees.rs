//! Worktrees as spaces (ADR 0004, as amended): `C-a W` checks a branch
//! out in a new worktree and opens a space named `<repo>/<branch>` in it,
//! `C-a O` opens a space for a worktree that has none, and `x` on a
//! worktree space in the sidebar deletes the space with its worktree and
//! branch. The sidebar lists worktree spaces under their repository's
//! main-checkout space.

use super::sidebar::Pick;
use super::state::{App, CloseTarget, Mode, NameTarget};
use super::view::Home;
use crate::git::{self, Checkout, Worktree};
use crate::sessions::State;
use anyhow::{anyhow, bail, Result};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

/// A worktree space, as deleting it needs: its tmux session, its
/// worktree's directory and branch (`None` if detached), and the
/// repository's main checkout, where git runs once the worktree is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSpace {
    pub space: String,
    pub path: String,
    pub branch: Option<String>,
    pub main: String,
}

impl WorktreeSpace {
    /// What the questions call it: its branch, else its directory.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or(&self.path)
    }
}

/// A worktree the open list offers: the name its space would get, its
/// directory, and the line drawn for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub path: String,
    pub line: String,
}

/// How SPACES lists the spaces: a space in a repository's main checkout
/// (or outside any), then the spaces in that repository's linked
/// worktrees; the worktree spaces of a repository whose main checkout
/// has no space come last, under a header named after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceEntry {
    /// The space at this index in the app's spaces, and whether it is in
    /// a linked worktree.
    Space { index: usize, linked: bool },
    /// A repository's name.
    Repo(String),
}

/// The name a space in worktree `w` of repository `repo` gets:
/// `<repo>/<branch>`, or `<repo>` for the main checkout, or the
/// directory's name after the repository for a detached one.
fn space_name(repo: &str, main: &str, w: &Worktree) -> String {
    if w.path == main {
        return repo.to_string();
    }
    let dir = std::path::Path::new(&w.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| w.path.clone());
    format!("{repo}/{}", w.branch.as_deref().unwrap_or(&dir))
}

impl App {
    /// The directory an action on `pick` starts from: the space's, or the
    /// agent's pane's; without a pick, the focused terminal's.
    fn dir_for(&self, pick: Option<&Pick>) -> PathBuf {
        match pick {
            Some(Pick::Space(id)) => self.space_dir(id),
            Some(Pick::Agent(pane)) => self.dir_of(pane),
            None => None,
        }
        .unwrap_or_else(|| self.new_dir())
    }

    /// Space `id`'s directory: as read for the sidebar, else its first
    /// tab's first pane's now.
    fn space_dir(&self, id: &str) -> Option<PathBuf> {
        if let Some(place) = self.places.get(id) {
            return Some(PathBuf::from(&place.path));
        }
        let space = self.spaces.iter().find(|s| s.id == id)?;
        let pane = &space.windows().first()?.panes.first()?.id;
        self.dir_of(pane)
    }

    /// The repository the directory of `pick` (see [`Self::dir_for`]) is
    /// in, as a notice if none.
    fn checkout_for(&mut self, pick: Option<&Pick>) -> Option<Checkout> {
        let dir = self.dir_for(pick);
        let checkout = git::checkout(&dir.to_string_lossy());
        if checkout.is_none() {
            self.notice = Some(format!("{} is not in a git repository", dir.display()));
        }
        checkout
    }

    /// `C-a W` (or `W` on a space in the sidebar): ask for a branch name
    /// to check out in a new worktree of the repository.
    pub(super) fn start_new_worktree(&mut self, pick: Option<&Pick>) {
        if let Some(checkout) = self.checkout_for(pick) {
            self.start_naming(NameTarget::NewWorktree(checkout));
        }
    }

    /// Check `branch` out in a new worktree of `checkout`'s repository
    /// (`<directory>/<repo>/<branch>`, creating the branch if there is
    /// none) and open a space named `<repo>/<branch>` in it. Nothing is
    /// left half-made: if the space fails, the worktree goes again, and
    /// the branch too if it was made for it.
    pub fn new_worktree(&mut self, checkout: &Checkout, branch: &str) -> Result<()> {
        let branch = branch.trim();
        if !git::valid_branch(branch) {
            bail!("{branch:?} is not a valid branch name");
        }
        let repo = checkout.repo();
        let path = git::worktree_path(&self.worktrees.directory, repo, branch);
        if path.exists() {
            bail!("{} exists", path.display());
        }
        // From the focused working tree: a new branch starts at its HEAD.
        let created = git::add_worktree(&checkout.top, &path, branch).map_err(|e| anyhow!(e))?;
        let name = format!("{repo}/{branch}");
        if let Err(e) = self.new_space_in(&name, path.clone()) {
            let _ = git::remove_worktree(
                &checkout.main,
                &path.to_string_lossy(),
                created.then_some(branch),
                true,
            );
            return Err(e);
        }
        Ok(())
    }

    /// `C-a O` (or `o` on a space in the sidebar): list the repository's
    /// worktrees that have no space, to open one. A panel with the
    /// keyboard gets it back when the list closes.
    pub(super) fn start_open_worktree(&mut self, pick: Option<&Pick>) {
        let Some(checkout) = self.checkout_for(pick) else {
            return;
        };
        let home = Home::from_env();
        let repo = checkout.repo();
        let taken: Vec<&str> = self
            .spaces
            .iter()
            .filter_map(|s| self.places.get(&s.id)?.checkout.as_ref())
            .map(|c| c.top.as_str())
            .collect();
        let candidates: Vec<Choice> = git::worktrees(&checkout.main)
            .into_iter()
            .filter(|w| !taken.contains(&w.path.as_str()))
            .map(|w| {
                let name = space_name(repo, &checkout.main, &w);
                let line = format!("{name}  {}", home.show(&w.path));
                Choice {
                    name,
                    path: w.path,
                    line,
                }
            })
            .collect();
        if candidates.is_empty() {
            self.notice = Some(format!("every worktree of {repo} has a space"));
            return;
        }
        let back = std::mem::replace(&mut self.mode, Mode::Normal).into_panel();
        self.mode = Mode::OpenWorktree {
            candidates,
            selected: 0,
            back,
        };
    }

    /// A key while the list of worktrees is open: `j`/`k` (or arrows)
    /// move, `Enter` opens a space for the selected one, `Esc` closes the
    /// list. These keys are fixed, like the prompt's.
    pub(super) fn open_worktree_key(
        &mut self,
        candidates: Vec<Choice>,
        selected: usize,
        back: Option<super::state::Panel>,
        key: KeyEvent,
    ) {
        let last = candidates.len().saturating_sub(1);
        let selected = match key.code {
            KeyCode::Esc => {
                self.mode = Mode::back_to(back);
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::back_to(back);
                if let Some(c) = candidates.get(selected) {
                    let dir = PathBuf::from(&c.path);
                    self.report(|app| app.new_space_in(&c.name, dir));
                    self.relayout_and_save();
                }
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => (selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => selected.saturating_sub(1),
            _ => selected,
        };
        self.mode = Mode::OpenWorktree {
            candidates,
            selected,
            back,
        };
    }

    /// Space `id` as a worktree space, if its directory is a linked
    /// worktree (once read); the main checkout is never one.
    pub(super) fn worktree_space(&self, id: &str) -> Option<WorktreeSpace> {
        let checkout = self.places.get(id)?.checkout.as_ref()?;
        if !checkout.is_linked() {
            return None;
        }
        Some(WorktreeSpace {
            space: id.to_string(),
            path: checkout.top.clone(),
            branch: git::current_branch(&checkout.top),
            main: checkout.main.clone(),
        })
    }

    /// `x` on a worktree space in the sidebar, or `delete_worktree`: ask
    /// before deleting space `id` with its worktree and branch. A panel
    /// with the keyboard gets it back after.
    pub(super) fn delete_worktree(&mut self, id: &str) -> Result<()> {
        let Some(target) = self.worktree_space(id) else {
            bail!("not a worktree space: the main checkout stays");
        };
        let back = std::mem::replace(&mut self.mode, Mode::Normal).into_panel();
        self.mode = Mode::ConfirmDelete {
            target,
            risk: None,
            back,
        };
        Ok(())
    }

    /// `y` to a delete question. The first time, work that would be lost
    /// (uncommitted changes, or a branch not in its base) is asked about
    /// again, naming it; else the space, worktree and branch go.
    pub(super) fn confirm_delete(
        &mut self,
        target: WorktreeSpace,
        risk: Option<String>,
        back: Option<super::state::Panel>,
    ) {
        if risk.is_none() {
            if let Some(risk) = self.delete_risk(&target) {
                self.mode = Mode::ConfirmDelete {
                    target,
                    risk: Some(risk),
                    back,
                };
                return;
            }
        }
        self.mode = Mode::back_to(back);
        let force = risk.is_some();
        self.report(|app| app.delete_worktree_space(target, force));
    }

    /// What deleting `target` would lose, if anything.
    fn delete_risk(&self, target: &WorktreeSpace) -> Option<String> {
        if git::is_dirty(&target.path) {
            return Some(format!("{} has uncommitted changes", target.label()));
        }
        let branch = target.branch.as_deref()?;
        if git::in_base(&target.main, branch) {
            return None;
        }
        let base = git::default_branch(&target.main).unwrap_or_else(|| "its base".into());
        Some(format!("{branch} is not merged into {base}"))
    }

    /// Delete `target`: close its space, asking first if Claude is running
    /// or waiting in it (as `close` does), then remove the worktree
    /// (`force`: with its uncommitted changes) and delete the branch.
    fn delete_worktree_space(&mut self, target: WorktreeSpace, force: bool) -> Result<()> {
        let busy = self
            .rows()
            .into_iter()
            .filter(|r| r.space == target.space)
            .map(|r| r.state)
            .find(|s| matches!(s, State::Running | State::Waiting));
        match busy {
            Some(state) => {
                self.ask_close(CloseTarget::Worktree { target, force }, state);
                Ok(())
            }
            None => self.remove_worktree_space(&target, force),
        }
    }

    /// Close `target`'s space, then remove its worktree and delete its
    /// branch (`branch -d`: one not in its base stays, and git says so).
    pub(super) fn remove_worktree_space(
        &mut self,
        target: &WorktreeSpace,
        force: bool,
    ) -> Result<()> {
        self.server.kill_session(&target.space)?;
        git::remove_worktree(&target.main, &target.path, target.branch.as_deref(), force)
            .map_err(|e| anyhow!(e))
    }

    /// The spaces in sidebar order (see [`SpaceEntry`]).
    pub(super) fn space_entries(&self) -> Vec<SpaceEntry> {
        let checkouts: Vec<Option<&Checkout>> = self
            .spaces
            .iter()
            .map(|s| self.places.get(&s.id)?.checkout.as_ref())
            .collect();
        group(&checkouts)
    }
}

/// The sidebar order of spaces whose working trees are `checkouts` (by
/// space index; `None` outside a repository, or not read yet): see
/// [`SpaceEntry`].
fn group(checkouts: &[Option<&Checkout>]) -> Vec<SpaceEntry> {
    let linked: Vec<usize> = (0..checkouts.len())
        .filter(|&i| checkouts[i].is_some_and(Checkout::is_linked))
        .collect();
    let mut placed = vec![false; checkouts.len()];
    let mut out = Vec::new();
    // A repository's worktree spaces, in order, under its main checkout.
    let under = |main: &str, out: &mut Vec<SpaceEntry>, placed: &mut Vec<bool>| {
        for &j in &linked {
            if !placed[j] && checkouts[j].is_some_and(|c| c.main == main) {
                placed[j] = true;
                out.push(SpaceEntry::Space {
                    index: j,
                    linked: true,
                });
            }
        }
    };
    for i in (0..checkouts.len()).filter(|i| !linked.contains(i)) {
        out.push(SpaceEntry::Space {
            index: i,
            linked: false,
        });
        if let Some(c) = checkouts[i] {
            under(&c.main, &mut out, &mut placed);
        }
    }
    for &j in &linked {
        if placed[j] {
            continue;
        }
        if let Some(c) = checkouts[j] {
            out.push(SpaceEntry::Repo(c.repo().to_string()));
            under(&c.main, &mut out, &mut placed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(top: &str, main: &str) -> Checkout {
        Checkout {
            top: top.into(),
            main: main.into(),
        }
    }

    #[test]
    fn worktree_spaces_follow_their_repository_s_space_or_get_a_header() {
        let api = checkout("/r/api", "/r/api");
        let api_x = checkout("/w/api/feat-x", "/r/api");
        let api_y = checkout("/w/api/feat-y", "/r/api");
        let pad_z = checkout("/w/paddock/feat-z", "/r/paddock");
        let space = |index, linked| SpaceEntry::Space { index, linked };
        // Spaces: a worktree of api, a plain directory, paddock's
        // worktree, api's main checkout, another worktree of api, one not
        // read yet.
        let got = group(&[
            Some(&api_x),
            None,
            Some(&pad_z),
            Some(&api),
            Some(&api_y),
            None,
        ]);
        assert_eq!(
            got,
            [
                space(1, false),
                space(3, false),
                space(0, true),
                space(4, true),
                space(5, false),
                SpaceEntry::Repo("paddock".into()),
                space(2, true),
            ]
        );
        assert_eq!(group(&[]), []);
        // Two spaces in the main checkout: the first gets the worktrees.
        let got = group(&[Some(&api), Some(&api), Some(&api_x)]);
        assert_eq!(got, [space(0, false), space(2, true), space(1, false)]);
    }

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: path.into(),
            branch: branch.map(str::to_string),
        }
    }

    #[test]
    fn a_worktree_space_is_named_after_its_repository_and_branch() {
        assert_eq!(
            space_name("api", "/r/api", &wt("/r/api", Some("main"))),
            "api"
        );
        assert_eq!(
            space_name("api", "/r/api", &wt("/w/api/feat-x", Some("feat/x"))),
            "api/feat/x"
        );
        assert_eq!(
            space_name("api", "/r/api", &wt("/w/api/loose", None)),
            "api/loose"
        );
    }
}
