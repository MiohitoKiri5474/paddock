//! Worktrees as spaces (ADR 0004, as amended): `C-a W` checks a branch
//! out in a new worktree and opens a space named `<repo>/<branch>` in it,
//! `C-a O` opens a space for a worktree that has none, and `x` on a
//! worktree space in the sidebar deletes the space with its worktree and
//! branch. Only worktrees under `[worktrees] directory` are deleted;
//! others (such as Claude's own agent worktrees) are only closed. The
//! sidebar lists worktree spaces under their repository's main-checkout
//! space.

use super::sidebar::Pick;
use super::state::{App, CloseTarget, Mode, NameTarget, Panel};
use crate::backend::SpaceId;
use crate::git::{self, WorkingTree};
use crate::paths::HomePaths;
use crate::sessions::State;
use crate::worktree::{self as shared, space_name, Risks, WorktreeSpace, RISK_DEADLINE};
use anyhow::{bail, Result};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

/// The delete question: about `target`; the second time, `risks` holds
/// what would be lost; `back` is the panel asked from, to return to.
pub struct DeleteQuestion {
    pub target: WorktreeSpace,
    pub risks: Option<Risks>,
    pub back: Option<Panel>,
}

impl DeleteQuestion {
    /// The question as the prompt shows it.
    pub fn text(&self) -> String {
        let label = self.target.label();
        let Some(risks) = &self.risks else {
            return format!("delete {label} and its worktree (ignored files too)? y/n");
        };
        let kept = if self.target.branch.is_some() {
            " (the branch is kept)"
        } else {
            ""
        };
        if risks.late {
            // Detached, its commits go with the worktree.
            let lost = if kept.is_empty() {
                "any changes and commits"
            } else {
                "any changes"
            };
            return format!(
                "couldn't check {label} in time: delete anyway, discarding {lost}?{kept} y/n"
            );
        }
        let mut what = Vec::new();
        if risks.dirty {
            what.push("has uncommitted changes".to_string());
        }
        match (&risks.unmerged, &self.target.branch) {
            (Some(base), Some(_)) => what.push(format!("is not merged into {base}")),
            (Some(base), None) => what.push(format!("has commits not in {base}")),
            (None, _) => {}
        }
        let what = what.join(" and ");
        if risks.unmerged.is_some() && self.target.branch.is_some() {
            format!("{label} {what}: remove the worktree anyway?{kept} y/n")
        } else {
            format!("{label} {what}: delete anyway? y/n")
        }
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

/// The open list: the worktrees offered, the line the cursor is on, and
/// the panel asked from, to return to.
pub struct WorktreeList {
    pub candidates: Vec<Choice>,
    pub selected: usize,
    pub back: Option<Panel>,
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
    /// The header over a repository's worktree spaces: its name.
    RepoHeader(String),
}

impl App {
    /// The directory an action on `pick` starts from: the space's, or the
    /// agent's pane's; without a pick, the focused terminal's. Read now.
    /// An external agent has none: refused, never the focused terminal's.
    fn dir_for(&self, pick: Option<&Pick>) -> Result<PathBuf> {
        let dir = match pick {
            Some(Pick::Space(id)) => self.space_dir_now(id),
            Some(Pick::Agent(pane)) => self.row_dir(pane)?,
            None => None,
        };
        Ok(dir.unwrap_or_else(|| self.new_dir()))
    }

    /// Space `id`'s directory, read now (the sidebar's may be seconds
    /// old), as [`shared::space_dir`] reads it for every decision about
    /// worktrees; `None` if it is gone.
    fn space_dir_now(&self, id: &SpaceId) -> Option<PathBuf> {
        shared::space_dir(&self.backend, id)
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
    }

    /// Space `id`'s working tree, read now.
    fn tree_now(&self, id: &SpaceId) -> Option<WorkingTree> {
        git::working_tree(&self.space_dir_now(id)?.to_string_lossy())
    }

    /// The working tree the directory of `pick` (see [`Self::dir_for`]) is
    /// in, or a notice if none.
    fn tree_for(&mut self, pick: Option<&Pick>) -> Option<WorkingTree> {
        let dir = match self.dir_for(pick) {
            Ok(dir) => dir,
            Err(e) => {
                self.notice = Some(format!("{e:#}"));
                return None;
            }
        };
        let tree = git::working_tree(&dir.to_string_lossy());
        if tree.is_none() {
            self.notice = Some(format!("{} is not in a git repository", dir.display()));
        }
        tree
    }

    /// `C-a W` (or `W` in the sidebar): ask for a branch name to check out
    /// in a new worktree of the repository.
    pub(super) fn start_new_worktree(&mut self, pick: Option<&Pick>) {
        if let Some(tree) = self.tree_for(pick) {
            self.start_naming(NameTarget::NewWorktree(tree));
        }
    }

    /// Check `branch` out in a new worktree of `tree`'s repository
    /// (`<directory>/<repo>/<branch>`, creating the branch if there is
    /// none) and open a space named `<repo>/<branch>` in it. Nothing is
    /// left half-made: if the space fails, the worktree goes again, and
    /// the branch too if it was made for it.
    pub fn new_worktree(&mut self, tree: &WorkingTree, branch: &str) -> Result<()> {
        let directory = self.worktrees.directory.clone();
        shared::create(&directory, tree, branch, |name, path| {
            self.new_space_in(name, path)
        })?;
        Ok(())
    }

    /// `C-a O` (or `o` in the sidebar): list the repository's worktrees
    /// that no space is in now, to open one. A panel with the keyboard
    /// gets it back when the list closes.
    pub(super) fn start_open_worktree(&mut self, pick: Option<&Pick>) {
        let Some(tree) = self.tree_for(pick) else {
            return;
        };
        let home = HomePaths::new();
        let repo = tree.repo();
        let taken: Vec<String> = self
            .spaces
            .iter()
            .filter_map(|s| self.tree_now(&s.id))
            .map(|t| t.top)
            .collect();
        let candidates: Vec<Choice> = git::worktrees(&tree.main)
            .into_iter()
            .filter(|w| !taken.contains(&w.path))
            .map(|w| {
                let name = space_name(repo, &tree.main, &w);
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
        self.mode = Mode::OpenWorktree(WorktreeList {
            candidates,
            selected: 0,
            back,
        });
    }

    /// A key while the list of worktrees is open: `j`/`k` (or arrows)
    /// move, `Enter` opens a space for the selected one, `Esc` closes the
    /// list. These keys are fixed, like the prompt's.
    pub(super) fn open_worktree_key(&mut self, list: WorktreeList, key: KeyEvent) {
        let last = list.candidates.len().saturating_sub(1);
        let selected = match key.code {
            KeyCode::Esc => {
                self.mode = Mode::back_to(list.back);
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::back_to(list.back);
                if let Some(c) = list.candidates.get(list.selected) {
                    let dir = PathBuf::from(&c.path);
                    self.report(|app| app.new_space_in(&c.name, dir));
                    self.relayout_and_save();
                }
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => (list.selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => list.selected.saturating_sub(1),
            _ => list.selected,
        };
        self.mode = Mode::OpenWorktree(WorktreeList { selected, ..list });
    }

    /// Space `id` as a worktree space paddock may delete, read now: its
    /// directory is in a linked worktree paddock made (see [`shared::is_ours`]).
    /// The main checkout, and worktrees elsewhere (Claude's own), never
    /// are.
    pub(super) fn worktree_space(&self, id: &SpaceId) -> Option<WorktreeSpace> {
        let tree = self.tree_now(id)?;
        if !tree.is_linked() || !shared::is_ours(&self.worktrees.directory, &tree) {
            return None;
        }
        Some(WorktreeSpace {
            space: Some(id.clone()),
            spaces: vec![id.clone()],
            branch: git::current_branch(&tree.top),
            path: tree.top,
            main: tree.main,
        })
    }

    /// Every space whose working tree is the one at `top`, read now (see
    /// [`shared::spaces_in`]).
    fn spaces_in(&self, top: &str) -> Result<Vec<SpaceId>> {
        let ids: Vec<SpaceId> = self.spaces.iter().map(|s| s.id.clone()).collect();
        Ok(shared::spaces_in(&self.backend, &ids, top)?)
    }

    /// `delete_worktree`: ask before deleting space `id` with its worktree
    /// and branch, if it is a worktree space; else say why not.
    pub(super) fn delete_worktree(&mut self, id: &SpaceId) -> Result<()> {
        match self.worktree_space(id) {
            Some(target) => {
                self.ask_delete(target);
                Ok(())
            }
            None => bail!(
                "not a worktree space: the main checkout and worktrees outside \
                 [worktrees] directory stay"
            ),
        }
    }

    /// Ask before deleting `target`. A panel with the keyboard gets it
    /// back after.
    pub(super) fn ask_delete(&mut self, target: WorktreeSpace) {
        let back = std::mem::replace(&mut self.mode, Mode::Normal).into_panel();
        self.mode = Mode::ConfirmDelete(DeleteQuestion {
            target,
            risks: None,
            back,
        });
    }

    /// `y` to a delete question. The first time, work that would be lost
    /// (uncommitted changes, commits not in the base) is asked about
    /// again, naming all of it; else the spaces, worktree and branch go.
    pub(super) fn confirm_delete(&mut self, q: DeleteQuestion) {
        if q.risks.is_none() {
            if let Some(risks) = delete_risks(&q.target) {
                self.mode = Mode::ConfirmDelete(DeleteQuestion {
                    risks: Some(risks),
                    ..q
                });
                return;
            }
        }
        self.mode = Mode::back_to(q.back);
        let (target, confirmed) = (q.target, q.risks);
        self.report(|app| app.delete_worktree_space(target, confirmed));
    }

    /// Delete `target`: close every space in its worktree, asking first if
    /// an agent is running or waiting in any of them (as `close` does), then
    /// remove the worktree and delete the branch. `confirmed` is what the
    /// user said yes to losing.
    fn delete_worktree_space(
        &mut self,
        mut target: WorktreeSpace,
        confirmed: Option<Risks>,
    ) -> Result<()> {
        target.spaces = self.spaces_in(&target.path)?;
        if let Some(space) = target.space.clone().filter(|s| !target.spaces.contains(s)) {
            target.spaces.push(space);
        }
        let busy = self
            .rows()
            .into_iter()
            .filter(|r| target.spaces.iter().any(|s| *s == r.space))
            .map(|r| r.state)
            .find(|s| matches!(s, State::Running | State::Waiting));
        match busy {
            Some(state) => {
                self.ask_close(CloseTarget::Worktree { target, confirmed }, state);
                Ok(())
            }
            None => self.remove_worktree_space(&target, confirmed.as_ref()),
        }
    }

    /// Close every space in `target`'s worktree, remove the worktree and
    /// delete its branch. Uncommitted changes go only if the user
    /// `confirmed` them (`--force`); changes or commits the user was not
    /// asked about (they came after the question) leave everything as it
    /// is, with a notice. A branch git does not find merged is kept, with
    /// a notice.
    pub(super) fn remove_worktree_space(
        &mut self,
        target: &WorktreeSpace,
        confirmed: Option<&Risks>,
    ) -> Result<()> {
        if let Some(notice) = shared::remove(&self.backend, target, confirmed)?.notice {
            self.notice = Some(notice);
        }
        Ok(())
    }

    /// The spaces in sidebar order (see [`SpaceEntry`]), from the working
    /// trees read in the background.
    pub(super) fn space_entries(&self) -> Vec<SpaceEntry> {
        let trees: Vec<Option<&WorkingTree>> = self
            .spaces
            .iter()
            .map(|s| self.places.get(&s.id)?.tree.as_ref())
            .collect();
        group(&trees)
    }
}

/// What deleting `target` would lose, if anything: uncommitted changes,
/// and commits checked out there that are not in the base (a branch's or
/// a detached HEAD's).
fn delete_risks(target: &WorktreeSpace) -> Option<Risks> {
    shared::risks_within(&target.path, RISK_DEADLINE)
}

/// The sidebar order of spaces whose working trees are `trees` (by space
/// index; `None` outside a repository, or not read yet): see
/// [`SpaceEntry`].
fn group(trees: &[Option<&WorkingTree>]) -> Vec<SpaceEntry> {
    let linked: Vec<usize> = (0..trees.len())
        .filter(|&i| trees[i].is_some_and(WorkingTree::is_linked))
        .collect();
    let mut placed = vec![false; trees.len()];
    let mut out = Vec::new();
    // A repository's worktree spaces, in order, under its main checkout.
    let under = |main: &str, out: &mut Vec<SpaceEntry>, placed: &mut Vec<bool>| {
        for &j in &linked {
            if !placed[j] && trees[j].is_some_and(|t| t.main == main) {
                placed[j] = true;
                out.push(SpaceEntry::Space {
                    index: j,
                    linked: true,
                });
            }
        }
    };
    for i in (0..trees.len()).filter(|i| !linked.contains(i)) {
        out.push(SpaceEntry::Space {
            index: i,
            linked: false,
        });
        if let Some(t) = trees[i] {
            under(&t.main, &mut out, &mut placed);
        }
    }
    for &j in &linked {
        if placed[j] {
            continue;
        }
        if let Some(t) = trees[j] {
            out.push(SpaceEntry::RepoHeader(t.repo().to_string()));
            under(&t.main, &mut out, &mut placed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(top: &str, main: &str) -> WorkingTree {
        WorkingTree {
            top: top.into(),
            main: main.into(),
        }
    }

    #[test]
    fn worktree_spaces_follow_their_repository_s_space_or_get_a_header() {
        let api = tree("/r/api", "/r/api");
        let api_x = tree("/w/api/feat-x", "/r/api");
        let api_y = tree("/w/api/feat-y", "/r/api");
        let pad_z = tree("/w/paddock/feat-z", "/r/paddock");
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
                SpaceEntry::RepoHeader("paddock".into()),
                space(2, true),
            ]
        );
        assert_eq!(group(&[]), []);
        // Two spaces in the main checkout: the first gets the worktrees.
        let got = group(&[Some(&api), Some(&api), Some(&api_x)]);
        assert_eq!(got, [space(0, false), space(2, true), space(1, false)]);
    }

    #[test]
    fn the_delete_question_names_every_risk() {
        let q = |branch: Option<&str>, risks: Option<Risks>| {
            DeleteQuestion {
                target: WorktreeSpace {
                    space: Some("$1".into()),
                    spaces: vec!["$1".into()],
                    path: "/w/repo/x".into(),
                    branch: branch.map(str::to_string),
                    main: "/r/repo".into(),
                },
                risks,
                back: None,
            }
            .text()
        };
        let risks = |dirty, unmerged: Option<&str>| {
            Some(Risks {
                dirty,
                unmerged: unmerged.map(str::to_string),
                late: false,
            })
        };
        assert_eq!(
            q(Some("feat/x"), None),
            "delete feat/x and its worktree (ignored files too)? y/n"
        );
        assert_eq!(
            q(Some("feat/x"), risks(true, None)),
            "feat/x has uncommitted changes: delete anyway? y/n"
        );
        assert_eq!(
            q(Some("feat/x"), risks(false, Some("main"))),
            "feat/x is not merged into main: remove the worktree anyway? (the branch is kept) y/n"
        );
        assert_eq!(
            q(Some("feat/x"), risks(true, Some("main"))),
            "feat/x has uncommitted changes and is not merged into main: \
             remove the worktree anyway? (the branch is kept) y/n"
        );
        assert_eq!(
            q(None, risks(false, Some("main"))),
            "x has commits not in main: delete anyway? y/n"
        );
        // A check that ran out of time says so, and claims nothing.
        assert_eq!(
            q(Some("feat/x"), Some(Risks::unknown())),
            "couldn't check feat/x in time: delete anyway, discarding any changes? \
             (the branch is kept) y/n"
        );
        assert_eq!(
            q(None, Some(Risks::unknown())),
            "couldn't check x in time: delete anyway, discarding any changes and commits? y/n"
        );
    }
}
