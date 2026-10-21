//! `paddock worktree rm`: the delete `x` does, without its questions.
//! What `x` asks about, `rm` refuses unless a `--discard-*` flag agreed to
//! it, and what `x` never deletes, `rm` never does. In steps: find the
//! worktree, refuse what is never deleted, check what would be lost or
//! hit, then remove (which checks again first).

use super::worktree::{find, ids, inside, name_of, print_json, state_in, Context, Space};
use super::{allowed, Code, Discard, Fail};
use crate::git::{self, WorkingTree, Worktree};
use crate::sessions::State;
use crate::worktree::{self as shared, Risks, WorktreeSpace};
use serde::Serialize;
use std::path::Path;

/// The worktree `rm` was asked about, in its repository.
struct Found {
    tree: WorkingTree,
    w: Worktree,
    /// What messages call it: its branch, else its directory's name.
    label: String,
}

impl Found {
    fn refused(&self, why: &str) -> Fail {
        Fail::new(Code::Refused, format!("{} {why}", self.label))
    }
}

/// What `rm` prints with `--json`.
#[derive(Serialize)]
struct Removed<'a> {
    path: &'a str,
    branch: Option<&'a str>,
    /// The spaces closed with it.
    closed: Vec<String>,
    /// Whether `branch -d` deleted the branch (it keeps an unmerged one).
    branch_deleted: bool,
}

pub(super) fn rm(
    cx: &Context,
    from: Option<&Path>,
    target: &str,
    discard: Discard,
    json: bool,
) -> Result<(), Fail> {
    let found = resolve(cx, from, target)?;
    never_deleted(cx, &found)?;
    let spaces = cx.spaces();
    let closing = shared::spaces_in(&cx.server, &ids(&spaces), &found.w.path)?;
    let pid = cx.server_pid();
    not_the_caller_s(cx, &found, &closing, pid)?;
    let confirmed = check(cx, &found, &spaces, &closing, pid, discard)?;
    let target = WorktreeSpace {
        space: None,
        spaces: closing.clone(),
        path: found.w.path.clone(),
        branch: found.w.branch.clone(),
        main: found.tree.main.clone(),
    };
    let removal = shared::remove(&cx.server, &target, Some(&confirmed))?;
    if let Some(notice) = &removal.notice {
        eprintln!("paddock: {notice}");
    }
    if json {
        return print_json(&Removed {
            path: &found.w.path,
            branch: found.w.branch.as_deref(),
            closed: closing
                .iter()
                .map(|id| name_of(&spaces, id).to_string())
                .collect(),
            branch_deleted: removal.branch_deleted,
        });
    }
    println!("{}", found.w.path);
    Ok(())
}

/// The worktree `target` names in the repository of `--from` (else of
/// the current directory).
fn resolve(cx: &Context, from: Option<&Path>, target: &str) -> Result<Found, Fail> {
    let tree = cx.tree(from)?;
    let w = find(&tree, cx.base(from), target)?;
    let label = w
        .branch
        .clone()
        .unwrap_or_else(|| git::dir_name(&w.path).to_string());
    Ok(Found { tree, w, label })
}

/// Refuse what is never deleted, whatever the flags: the main checkout,
/// and a worktree paddock did not make.
fn never_deleted(cx: &Context, found: &Found) -> Result<(), Fail> {
    if found.w.path == found.tree.main {
        return Err(found.refused("is the main checkout: paddock never deletes it"));
    }
    let linked = WorkingTree {
        top: found.w.path.clone(),
        main: found.tree.main.clone(),
    };
    if !shared::is_ours(&cx.directory()?, &linked) {
        return Err(
            found.refused("was not made by paddock (not under [worktrees] directory): left alone")
        );
    }
    Ok(())
}

/// Refuse if the caller runs inside the worktree: its current directory,
/// or its terminal (in a space that would close, or in a directory in it).
fn not_the_caller_s(
    cx: &Context,
    found: &Found,
    closing: &[String],
    pid: Option<u32>,
) -> Result<(), Fail> {
    let top = &found.w.path;
    if inside(&cx.cwd.to_string_lossy(), top) {
        return Err(found.refused("holds the current directory: run rm from outside it"));
    }
    if let Some(pane) = cx.caller_pane(pid) {
        let space = cx.server.display(&pane, "#{session_id}").ok();
        let path = cx.server.display(&pane, "#{pane_current_path}").ok();
        let here =
            space.is_some_and(|s| closing.contains(&s)) || path.is_some_and(|p| inside(&p, top));
        if here {
            return Err(found.refused("holds the calling terminal: run rm from another one"));
        }
    }
    Ok(())
}

/// What would be lost (`allowed` given the flags; a late check is exit
/// 4), then who would be hit: a Claude running or waiting in any pane,
/// floats' included, of the spaces that close, or in any pane whose
/// directory is in the worktree. Returns what the delete may lose.
fn check(
    cx: &Context,
    found: &Found,
    spaces: &[Space],
    closing: &[String],
    pid: Option<u32>,
    discard: Discard,
) -> Result<Risks, Fail> {
    let top = found.w.path.as_str();
    let risks = shared::risks_within(top, cx.risk_deadline);
    let detached = found.w.branch.is_none();
    let confirmed = allowed(&found.label, risks.as_ref(), detached, discard)?;
    let (sessions, late) = cx.sessions(pid);
    if let Some(late) = late {
        return Err(late);
    }
    let busy = spaces
        .iter()
        .flat_map(|s| {
            let closes = closing.contains(&s.id);
            shared::panes(&cx.server, &s.id)
                .into_iter()
                .filter(move |p| closes || inside(&p.path, top))
                .map(move |p| (s, p.id))
        })
        .find_map(|(s, pane)| match state_in(&sessions, &pane) {
            Some(state @ (State::Running | State::Waiting)) => Some((s, state)),
            _ => None,
        });
    if let Some((space, state)) = busy {
        let why = format!(
            "has Claude {} in space {}: nothing deleted",
            state.label(),
            space.name
        );
        return Err(found.refused(&why));
    }
    Ok(confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use std::process::Command;
    use std::time::Duration;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    #[test]
    fn a_risk_check_past_its_deadline_exits_4_and_leaves_tree_space_and_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a"), "x\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "a"]);
        let paths = Paths {
            socket: format!("paddock-unit-rm-{}", std::process::id()),
            state_dir: root.join("state"),
            config_dir: root.join("config"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let wt = root.join("wt");
        let config = format!("[worktrees]\ndirectory = {:?}\n", wt.to_string_lossy());
        std::fs::write(paths.config_dir.join("config.toml"), config).unwrap();
        let mut cx = Context::new(paths, root.clone(), None);
        // Nothing can be checked in no time. The check comes before
        // `claude agents`, so no real Claude is asked.
        cx.risk_deadline = Duration::ZERO;

        let tree = cx.tree(Some(&repo)).unwrap();
        let made = shared::create(&wt, &tree, "feat/t", |name, path| cx.new_space(name, &path));
        let got = made
            .as_ref()
            .ok()
            .map(|_| rm(&cx, Some(&repo), "feat/t", Discard::default(), false));
        let names: Vec<String> = cx.server.sessions().into_iter().map(|s| s.name).collect();
        let socket = cx.server.run(&["display-message", "-p", "#{socket_path}"]);
        let _ = cx.server.run(&["kill-server"]);
        if let Ok(socket) = socket {
            let _ = std::fs::remove_file(socket.trim());
        }

        let (path, _) = made.unwrap();
        let err = got.unwrap().unwrap_err();
        assert_eq!(err.code, Code::Late, "{err:?}");
        assert_eq!(
            err.message,
            "couldn't check feat/t in time: nothing deleted"
        );
        assert!(path.is_dir(), "the worktree stays");
        assert_eq!(names, ["repo/feat/t"], "the space stays");
        let branch = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "--list", "feat/t"])
            .output()
            .unwrap();
        assert!(!branch.stdout.is_empty(), "the branch stays");
    }
}
