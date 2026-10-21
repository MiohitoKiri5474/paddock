//! Ticket 24: worktrees as spaces. `C-a W` checks a branch out in a new
//! worktree and opens a space in it, `C-a O` opens a space for a worktree
//! that has none, and `x` on a worktree space in the sidebar deletes it
//! with its worktree and branch. Against real temporary repositories; the
//! default `[worktrees] directory` lands in the sandbox's home.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Run git in `dir`, ignoring the user's own git configuration.
fn git(dir: &Path, args: &[&str]) -> String {
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
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `main` with one commit.
fn repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    commit(&dir, "one");
    dir
}

fn commit(dir: &Path, message: &str) {
    std::fs::write(dir.join(message), "x\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn has_branch(dir: &Path, branch: &str) -> bool {
    !git(dir, &["branch", "--list", branch]).is_empty()
}

/// Where `C-a W` puts the worktree of `branch`: the default directory in
/// the sandbox's home.
fn worktree(sandbox: &Sandbox, branch: &str) -> PathBuf {
    sandbox
        .path("home/.paddock/worktrees/repo")
        .join(branch.replace('/', "-"))
}

/// paddock with space-1's shell in the repository, once the sidebar shows
/// it there (the directory is read in the background).
fn in_repo(sandbox: &Sandbox, dir: &Path) -> Paddock {
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line(&format!("cd {}", dir.display()));
    paddock.wait_for("the sidebar on the repository", |_| {
        paddock.sidebar().iter().any(|r| r.ends_with("repo  main"))
    });
    paddock
}

fn wait_for_sessions(paddock: &Paddock, sandbox: &Sandbox, want: &[&str]) {
    paddock.wait_for(&format!("sessions {want:?}"), |_| {
        sandbox.session_names() == want
    });
}

/// The directory of session `name`'s first pane, links resolved.
fn session_dir(sandbox: &Sandbox, name: &str) -> Option<PathBuf> {
    let out = sandbox.tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name}\t#{pane_current_path}",
    ]);
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(s, _)| *s == name)
        .map(|(_, p)| PathBuf::from(p))
}

fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// `C-a W`, then `branch` and `Enter`.
fn new_worktree(paddock: &mut Paddock, branch: &str) {
    paddock.command(b"W");
    paddock.wait_for_popup("New worktree", "");
    paddock.send(branch.as_bytes());
    paddock.send(b"\r");
    paddock.wait_for_no_popup();
}

/// Wait until the sidebar lists `name` as a worktree space: one cell in.
fn wait_for_worktree_row(paddock: &Paddock, name: &str) {
    let row = format!("    {name}");
    paddock.wait_for(&format!("worktree row {name:?}"), |_| {
        paddock.sidebar().contains(&row)
    });
}

#[test]
fn c_a_w_makes_a_worktree_and_a_space_in_it() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);

    new_worktree(&mut paddock, "feat/x");

    wait_for_sessions(&paddock, &sandbox, &["space-1", "repo/feat/x"]);
    let path = worktree(&sandbox, "feat/x");
    assert!(path.is_dir(), "{}", path.display());
    assert_eq!(git(&path, &["branch", "--show-current"]), "feat/x");
    paddock.wait_for("the new space's shell in the worktree", |_| {
        session_dir(&sandbox, "repo/feat/x") == Some(real(&path))
    });
    // Listed under the repository's space, one cell in.
    wait_for_worktree_row(&paddock, "repo/feat/x");
    let rows = paddock.sidebar();
    let main = rows.iter().position(|r| r == "   space-1").unwrap();
    assert_eq!(rows[main + 2], "    repo/feat/x", "{rows:#?}");
}

#[test]
fn an_existing_branch_is_checked_out_and_bad_names_or_git_errors_make_nothing() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    git(&dir, &["branch", "feat/y"]);
    commit(&dir, "two");
    let tip = git(&dir, &["rev-parse", "feat/y"]);
    let mut paddock = in_repo(&sandbox, &dir);

    new_worktree(&mut paddock, "feat/y");
    wait_for_sessions(&paddock, &sandbox, &["space-1", "repo/feat/y"]);
    let path = worktree(&sandbox, "feat/y");
    assert_eq!(
        git(&path, &["rev-parse", "HEAD"]),
        tip,
        "the branch as it was"
    );

    new_worktree(&mut paddock, "bad..name");
    paddock.wait_for_text("not a valid branch name");
    assert!(!worktree(&sandbox, "bad..name").exists());

    // `main` is checked out in the main checkout: git refuses, and its
    // reason leads the notice.
    new_worktree(&mut paddock, "main");
    paddock.wait_for("git's reason", |_| {
        let bar = paddock.tab_bar();
        bar.contains("'main'") && !bar.contains("Preparing")
    });
    assert!(!worktree(&sandbox, "main").exists());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sandbox.session_names(), ["space-1", "repo/feat/y"]);
}

#[test]
fn c_a_o_lists_worktrees_without_a_space_and_opens_one() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let other = sandbox.path("wt/feat-o");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/o",
            &other.to_string_lossy(),
        ],
    );
    let mut paddock = in_repo(&sandbox, &dir);

    paddock.command(b"O");
    // Only the worktree without a space: the main checkout has space-1.
    paddock.wait_for_popup("Open worktree", "repo/feat/o");
    assert!(!paddock.full_screen().contains("│ repo  "));
    paddock.send(b"\r");

    wait_for_sessions(&paddock, &sandbox, &["space-1", "repo/feat/o"]);
    paddock.wait_for("the space's shell in the worktree", |_| {
        session_dir(&sandbox, "repo/feat/o") == Some(real(&other))
    });
    wait_for_worktree_row(&paddock, "repo/feat/o");

    // Now every worktree has a space.
    paddock.command(b"O");
    paddock.wait_for_text("every worktree of repo has a space");
}

#[test]
fn x_on_a_clean_merged_worktree_space_deletes_it_after_one_yes() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/d");
    wait_for_worktree_row(&paddock, "repo/feat/d");
    let path = worktree(&sandbox, "feat/d");

    // The sidebar on the current space: the new one.
    paddock.command(b"w");
    paddock.send(b"x");
    paddock.wait_for_popup("Delete worktree", "delete feat/d and its worktree? y/n");
    paddock.send(b"n");
    paddock.wait_for_no_popup();
    std::thread::sleep(Duration::from_millis(200));
    assert!(path.is_dir() && has_branch(&dir, "feat/d"), "kept");

    paddock.command(b"w");
    paddock.send(b"x");
    paddock.wait_for_popup("Delete worktree", "delete feat/d");
    paddock.send(b"y");

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    paddock.wait_for("the worktree and branch gone", |_| {
        !path.exists() && !has_branch(&dir, "feat/d")
    });
    assert_eq!(git(&dir, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn x_on_a_dirty_worktree_space_asks_again_naming_the_risk() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/e");
    wait_for_worktree_row(&paddock, "repo/feat/e");
    let path = worktree(&sandbox, "feat/e");
    std::fs::write(path.join("loose.txt"), "work\n").unwrap();

    paddock.command(b"w");
    paddock.send(b"x");
    paddock.wait_for_popup("Delete worktree", "delete feat/e");
    paddock.send(b"y");
    paddock.wait_for_popup(
        "Delete worktree",
        "feat/e has uncommitted changes: delete anyway? y/n",
    );
    assert!(path.is_dir(), "not yet");
    paddock.send(b"y");

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    paddock.wait_for("the worktree and branch gone", |_| {
        !path.exists() && !has_branch(&dir, "feat/e")
    });
}

#[test]
fn an_unmerged_branch_is_named_and_kept_by_branch_d() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/u");
    wait_for_worktree_row(&paddock, "repo/feat/u");
    let path = worktree(&sandbox, "feat/u");
    commit(&path, "work");

    paddock.command(b"w");
    paddock.send(b"x");
    paddock.wait_for_popup("Delete worktree", "delete feat/u");
    paddock.send(b"y");
    paddock.wait_for_popup(
        "Delete worktree",
        "feat/u is not merged into main: delete anyway? y/n",
    );
    paddock.send(b"y");

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    paddock.wait_for("the worktree gone", |_| !path.exists());
    // `branch -d`, never `-D`: git keeps the branch, and says so.
    paddock.wait_for_text("not fully merged");
    assert!(has_branch(&dir, "feat/u"));
}

#[test]
fn the_main_checkout_is_never_deleted() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.commands]\ndelete_worktree = \"X\"\n");
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);

    paddock.command(b"X");
    paddock.wait_for_text("the main checkout stays");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sandbox.session_names(), ["space-1"]);
    assert!(dir.join(".git").is_dir());
    assert!(!paddock.full_screen().contains(" Delete worktree "));
}
