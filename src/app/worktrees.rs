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
use super::view::Home;
use crate::git::{self, WorkingTree, Worktree};
use crate::sessions::State;
use anyhow::{anyhow, bail, Result};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// How long the whole check of what a delete would lose may take; the
/// second question waits for it.
const RISK_DEADLINE: Duration = Duration::from_secs(5);
/// What the questions call a base whose name is not known.
const BASE: &str = "its base";

/// A worktree space, as deleting it needs: its tmux session, every space
/// in that worktree (it first, read again before deleting), its
/// worktree's directory and branch (`None` if detached), and the
/// repository's main checkout, where git runs once the worktree is gone.
// ponytail: paths are `String` here and in `git` (as tmux and git print
// them) but `PathBuf` in config and `worktree_path`; one type would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSpace {
    pub space: String,
    pub spaces: Vec<String>,
    pub path: String,
    pub branch: Option<String>,
    pub main: String,
}

impl WorktreeSpace {
    /// What the questions call it: its branch, else (detached) its directory's name.
    pub fn label(&self) -> &str {
        self.branch
            .as_deref()
            .unwrap_or_else(|| git::dir_name(&self.path))
    }
}

/// What deleting a worktree would lose, beyond its ignored files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Risks {
    /// Uncommitted changes (untracked files count).
    pub dirty: bool,
    /// The base's name, if commits checked out there are not in it.
    pub unmerged: Option<String>,
    /// The check did not finish in time: nothing is known, so both of
    /// the above are assumed.
    pub late: bool,
}

impl Risks {
    /// What a check that did not finish in time assumes: every risk.
    fn unknown() -> Self {
        Self {
            dirty: true,
            unmerged: Some(BASE.into()),
            late: true,
        }
    }
}

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
            return format!(
                "couldn't check {label} in time: delete anyway, discarding any changes?{kept} y/n"
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

/// The name a space in worktree `w` of repository `repo` gets:
/// `<repo>/<branch>`, or `<repo>` for the main checkout, or the
/// directory's name after the repository for a detached one.
fn space_name(repo: &str, main: &str, w: &Worktree) -> String {
    if w.path == main {
        return repo.to_string();
    }
    let name = w
        .branch
        .as_deref()
        .unwrap_or_else(|| git::dir_name(&w.path));
    format!("{repo}/{name}")
}

impl App {
    /// The directory an action on `pick` starts from: the space's, or the
    /// agent's pane's; without a pick, the focused terminal's. Read now.
    fn dir_for(&self, pick: Option<&Pick>) -> PathBuf {
        match pick {
            Some(Pick::Space(id)) => self.space_dir_now(id),
            Some(Pick::Agent(pane)) => self.dir_of(pane),
            None => None,
        }
        .unwrap_or_else(|| self.new_dir())
    }

    /// Space `id`'s directory, read now (the sidebar's may be seconds
    /// old): its first tab's first pane's.
    fn space_dir_now(&self, id: &str) -> Option<PathBuf> {
        let space = self.spaces.iter().find(|s| s.id == id)?;
        let pane = &space.windows().first()?.panes.first()?.id;
        self.dir_of(pane)
    }

    /// Space `id`'s working tree, read now.
    fn tree_now(&self, id: &str) -> Option<WorkingTree> {
        git::working_tree(&self.space_dir_now(id)?.to_string_lossy())
    }

    /// The working tree the directory of `pick` (see [`Self::dir_for`]) is
    /// in, or a notice if none.
    fn tree_for(&mut self, pick: Option<&Pick>) -> Option<WorkingTree> {
        let dir = self.dir_for(pick);
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
        let branch = branch.trim();
        if !git::valid_branch(&tree.top, branch) {
            bail!("{branch:?} is not a valid branch name");
        }
        let repo = tree.repo();
        let path = git::worktree_path(&self.worktrees.directory, repo, branch);
        if path.exists() {
            bail!("{} exists", path.display());
        }
        // From the focused working tree: a new branch starts at its HEAD.
        let created = git::add_worktree(&tree.top, &path, branch).map_err(|e| anyhow!(e))?;
        let name = format!("{repo}/{branch}");
        if let Err(e) = self.new_space_in(&name, path.clone()) {
            // Best effort: it was made a moment ago, with nothing in it.
            let _ = git::remove_worktree(&tree.main, &path.to_string_lossy(), true);
            if created {
                let _ = git::delete_branch(&tree.main, branch);
            }
            return Err(e);
        }
        Ok(())
    }

    /// `C-a O` (or `o` in the sidebar): list the repository's worktrees
    /// that no space is in now, to open one. A panel with the keyboard
    /// gets it back when the list closes.
    pub(super) fn start_open_worktree(&mut self, pick: Option<&Pick>) {
        let Some(tree) = self.tree_for(pick) else {
            return;
        };
        let home = Home::from_env();
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
    /// directory is in a linked worktree paddock made (see [`is_ours`]).
    /// The main checkout, and worktrees elsewhere (Claude's own), never
    /// are.
    pub(super) fn worktree_space(&self, id: &str) -> Option<WorktreeSpace> {
        let tree = self.tree_now(id)?;
        if !tree.is_linked() || !is_ours(&self.worktrees.directory, &tree.top) {
            return None;
        }
        Some(WorktreeSpace {
            space: id.to_string(),
            spaces: vec![id.to_string()],
            branch: git::current_branch(&tree.top),
            path: tree.top,
            main: tree.main,
        })
    }

    /// Every space whose working tree is the one at `top`, read now.
    fn spaces_in(&self, top: &str) -> Vec<String> {
        self.spaces
            .iter()
            .filter(|s| self.tree_now(&s.id).is_some_and(|t| t.top == top))
            .map(|s| s.id.clone())
            .collect()
    }

    /// `delete_worktree`: ask before deleting space `id` with its worktree
    /// and branch, if it is a worktree space; else say why not.
    pub(super) fn delete_worktree(&mut self, id: &str) -> Result<()> {
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
    /// Claude is running or waiting in any of them (as `close` does), then
    /// remove the worktree and delete the branch. `confirmed` is what the
    /// user said yes to losing.
    fn delete_worktree_space(
        &mut self,
        mut target: WorktreeSpace,
        confirmed: Option<Risks>,
    ) -> Result<()> {
        target.spaces = self.spaces_in(&target.path);
        if !target.spaces.contains(&target.space) {
            target.spaces.push(target.space.clone());
        }
        let busy = self
            .rows()
            .into_iter()
            .filter(|r| target.spaces.contains(&r.space))
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
        let path = &target.path;
        let label = target.label();
        let force = confirmed.is_some_and(|r| r.dirty);
        if !force && git::is_dirty(path) {
            bail!("{label} has uncommitted changes now: nothing deleted");
        }
        let unmerged_ok = confirmed.is_some_and(|r| r.unmerged.is_some());
        if !unmerged_ok && !git::in_base(path, "HEAD") {
            bail!("{label} has commits not in its base now: nothing deleted");
        }
        let alive: Vec<String> = self.server.sessions().into_iter().map(|s| s.id).collect();
        for space in target.spaces.iter().filter(|s| alive.contains(s)) {
            self.server.kill_session(space)?;
        }
        // The path first: a notice cut to the tab bar still names it.
        git::remove_worktree(&target.main, path, force)
            .map_err(|e| anyhow!("space closed; {} left: {e}", Home::from_env().show(path)))?;
        if let Some(branch) = &target.branch {
            if let Err(e) = git::delete_branch(&target.main, branch) {
                self.notice = Some(format!("worktree removed; branch {branch} kept: {e}"));
            }
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
    risks_within(&target.path, RISK_DEADLINE)
}

/// What deleting the worktree at `path` would lose, read in at most
/// `limit` all told (its several git reads share it). Not known in time,
/// it counts as every risk, so the user is still asked.
fn risks_within(path: &str, limit: Duration) -> Option<Risks> {
    let (tx, rx) = mpsc::channel();
    let path = path.to_string();
    // Left to finish on its own if late: each read has its own timeout.
    std::thread::spawn(move || {
        let dirty = git::is_dirty(&path);
        let unmerged = (!git::in_base(&path, "HEAD"))
            .then(|| git::default_branch(&path).unwrap_or_else(|| BASE.into()));
        let risky = dirty || unmerged.is_some();
        let _ = tx.send(risky.then_some(Risks {
            dirty,
            unmerged,
            late: false,
        }));
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| Some(Risks::unknown()))
}

/// Whether the worktree at `top` is one paddock made, and so may delete:
/// `<directory>/<repo>/<name>`, as `C-a W` lays them out, exactly. A
/// worktree deeper down (a repository's `.claude/worktrees/agent-*` when
/// repositories live under `directory`) is not.
fn is_ours(directory: &Path, top: &str) -> bool {
    let directory = git::resolved(&directory.to_string_lossy());
    Path::new(top).parent().and_then(Path::parent) == Some(Path::new(&directory))
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

    #[test]
    fn only_worktrees_laid_out_as_paddock_makes_them_are_its_own() {
        let dir = Path::new("/nonexistent/code");
        assert!(is_ours(dir, "/nonexistent/code/repo/feat-x"));
        assert!(!is_ours(
            dir,
            "/nonexistent/code/repo/.claude/worktrees/agent-x"
        ));
        assert!(!is_ours(dir, "/nonexistent/code/repo"), "a main checkout");
        assert!(!is_ours(dir, "/elsewhere/repo/feat-x"));
    }

    #[test]
    fn a_risk_check_past_its_deadline_counts_as_every_risk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        let late = risks_within(&dir, Duration::ZERO);
        assert_eq!(late, Some(Risks::unknown()));
    }

    #[test]
    fn the_delete_question_names_every_risk() {
        let q = |branch: Option<&str>, risks: Option<Risks>| {
            DeleteQuestion {
                target: WorktreeSpace {
                    space: "$1".into(),
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
            "couldn't check x in time: delete anyway, discarding any changes? y/n"
        );
    }
}
