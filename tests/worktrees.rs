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

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);

/// The tmux session id (`$N`) of session `name`.
fn session_id(sandbox: &Sandbox, name: &str) -> String {
    sandbox
        .tmux(&["list-sessions", "-F", "#{session_id}\t#{session_name}"])
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(_, n)| *n == name)
        .map(|(id, _)| id.to_string())
        .unwrap_or_else(|| panic!("no session {name}"))
}

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

/// Wait until the whole screen (the popup is centred over the sidebar too)
/// shows every one of `parts`.
fn wait_for_all(paddock: &Paddock, parts: &[&str]) {
    paddock.wait_for(&format!("{parts:?}"), |_| {
        let s = paddock.full_screen();
        parts.iter().all(|p| s.contains(p))
    });
}

/// `x` on the current space, from the sidebar (`C-a w` puts it there).
fn x_on_current_space(paddock: &mut Paddock) {
    paddock.command(b"w");
    paddock.send(b"x");
}

/// Open the only worktree `C-a O` lists, named `name`, and wait for its
/// space in the sidebar.
fn open_listed(paddock: &mut Paddock, name: &str) {
    paddock.command(b"O");
    paddock.wait_for_popup("Open worktree", name);
    paddock.send(b"\r");
    wait_for_worktree_row(paddock, name);
}

#[test]
fn x_on_a_clean_merged_worktree_space_deletes_it_after_one_yes() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/d");
    wait_for_worktree_row(&paddock, "repo/feat/d");
    let path = worktree(&sandbox, "feat/d");

    // `n` cancels: nothing is deleted, nor is the space closed.
    x_on_current_space(&mut paddock);
    paddock.wait_for_popup(
        "Delete worktree",
        "delete feat/d and its worktree (ignored files too)? y/n",
    );
    paddock.send(b"n");
    paddock.wait_for_no_popup();
    std::thread::sleep(Duration::from_millis(200));
    assert!(path.is_dir() && has_branch(&dir, "feat/d"), "kept");
    assert_eq!(sandbox.session_names(), ["space-1", "repo/feat/d"]);

    x_on_current_space(&mut paddock);
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

    x_on_current_space(&mut paddock);
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
fn a_dirty_unmerged_worktree_names_both_risks_and_the_branch_is_kept() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/u");
    wait_for_worktree_row(&paddock, "repo/feat/u");
    let path = worktree(&sandbox, "feat/u");
    commit(&path, "work");
    std::fs::write(path.join("loose.txt"), "work\n").unwrap();

    x_on_current_space(&mut paddock);
    paddock.wait_for_popup("Delete worktree", "delete feat/u");
    paddock.send(b"y");
    // Wrapped over two lines of the box.
    wait_for_all(
        &paddock,
        &[
            "feat/u has uncommitted changes and is not merged into main:",
            "(the branch is kept) y/n",
        ],
    );
    paddock.send(b"y");

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    paddock.wait_for("the worktree gone", |_| !path.exists());
    // `branch -d`, never `-D`: git keeps the branch, and a notice says so.
    paddock.wait_for_text("branch feat/u kept");
    assert!(has_branch(&dir, "feat/u"));
}

#[test]
fn commits_made_after_the_question_are_never_lost() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/c");
    wait_for_worktree_row(&paddock, "repo/feat/c");
    let path = worktree(&sandbox, "feat/c");
    std::fs::write(path.join("loose.txt"), "work\n").unwrap();

    x_on_current_space(&mut paddock);
    paddock.wait_for_popup("Delete worktree", "delete feat/c");
    paddock.send(b"y");
    paddock.wait_for_popup(
        "Delete worktree",
        "feat/c has uncommitted changes: delete anyway? y/n",
    );
    // The question was only about changes; they become a commit now.
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "late"]);
    paddock.send(b"y");

    paddock.wait_for_text("has commits not in its base now");
    assert!(path.join("loose.txt").is_file());
    assert_eq!(sandbox.session_names(), ["space-1", "repo/feat/c"]);
}

#[test]
fn every_space_in_the_worktree_closes_with_it() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/s");
    wait_for_worktree_row(&paddock, "repo/feat/s");
    paddock.wait_for_text(PROMPT);
    // A second space, started in the worktree's terminal's directory.
    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\r");
    wait_for_sessions(&paddock, &sandbox, &["space-1", "repo/feat/s", "space-2"]);
    wait_for_worktree_row(&paddock, "space-2");
    let path = worktree(&sandbox, "feat/s");
    // Claude runs in the first space, not the one `x` is pressed on.
    let first = session_id(&sandbox, "repo/feat/s");
    let payload = std::fs::read_to_string(format!("{SPIKE}/UserPromptSubmit.json"))
        .unwrap()
        .replace("\"/tmp/repo\"", &format!("{:?}", path.display()));
    sandbox.hook(&first, "UserPromptSubmit", &payload);
    paddock.wait_for("Claude running", |_| {
        paddock.full_screen().contains("running")
    });

    x_on_current_space(&mut paddock);
    paddock.wait_for_popup("Delete worktree", "delete feat/s");
    paddock.send(b"y");
    paddock.wait_for_popup("Close space", "an agent is running: close? y/n");
    assert!(path.is_dir(), "not before the answer");
    paddock.send(b"y");

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    paddock.wait_for("the worktree gone", |_| !path.exists());
}

#[test]
fn changes_made_after_the_question_are_never_forced_away() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);
    new_worktree(&mut paddock, "feat/r");
    wait_for_worktree_row(&paddock, "repo/feat/r");
    let path = worktree(&sandbox, "feat/r");
    commit(&path, "work");

    x_on_current_space(&mut paddock);
    paddock.wait_for_popup("Delete worktree", "delete feat/r");
    paddock.send(b"y");
    wait_for_all(&paddock, &["feat/r is not merged into main"]);
    // The question was only about the branch; new work comes in now.
    std::fs::write(path.join("late.txt"), "work\n").unwrap();
    paddock.send(b"y");

    paddock.wait_for_text("feat/r has uncommitted changes now: nothing deleted");
    assert!(path.join("late.txt").is_file());
    assert_eq!(sandbox.session_names(), ["space-1", "repo/feat/r"]);
}

#[test]
fn a_detached_worktree_with_its_own_commits_asks_again() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let path = worktree(&sandbox, "loose");
    git(
        &dir,
        &["worktree", "add", "-q", "--detach", &path.to_string_lossy()],
    );
    commit(&path, "detached-work");
    let mut paddock = in_repo(&sandbox, &dir);
    open_listed(&mut paddock, "repo/loose");

    x_on_current_space(&mut paddock);
    paddock.wait_for_popup("Delete worktree", "delete loose and its worktree");
    paddock.send(b"y");
    paddock.wait_for_popup(
        "Delete worktree",
        "loose has commits not in main: delete anyway? y/n",
    );
    paddock.send(b"y");

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    paddock.wait_for("the worktree gone", |_| !path.exists());
}

#[test]
fn x_on_a_worktree_space_paddock_did_not_make_only_closes_it() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    // As Claude's worktree isolation makes them: inside the repository.
    let agent = dir.join(".claude/worktrees/agent-a1");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worktree-agent-a1",
            &agent.to_string_lossy(),
        ],
    );
    let mut paddock = in_repo(&sandbox, &dir);
    open_listed(&mut paddock, "repo/worktree-agent-a1");

    x_on_current_space(&mut paddock);

    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    assert!(!paddock.full_screen().contains(" Delete worktree "));
    assert!(agent.is_dir(), "the worktree stays");
    assert!(has_branch(&dir, "worktree-agent-a1"), "so does its branch");
}

#[test]
fn the_main_checkout_is_never_deleted() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.commands]\ndelete_worktree = \"X\"\n");
    let dir = repo(&sandbox);
    let mut paddock = in_repo(&sandbox, &dir);

    paddock.command(b"X");
    paddock.wait_for_text("not a worktree space");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sandbox.session_names(), ["space-1"]);
    assert!(!paddock.full_screen().contains(" Delete worktree "));

    // Sidebar `x` on a space in the main checkout closes it, as before.
    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\r");
    wait_for_sessions(&paddock, &sandbox, &["space-1", "space-2"]);
    paddock.wait_for_text(PROMPT);
    x_on_current_space(&mut paddock);
    wait_for_sessions(&paddock, &sandbox, &["space-1"]);
    assert!(!paddock.full_screen().contains(" Delete worktree "));
    assert!(dir.join(".git").is_dir());
    assert_eq!(git(&dir, &["branch", "--show-current"]), "main");
}
