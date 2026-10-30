//! Ticket 31: a root directory per space. A space's root is kept with
//! the space on the server, set when the space is made
//! (`C-a C`, `C-a W`, `paddock worktree new`) and changed with `C-a R`.
//! With `[spaces] new_tab_dir = "root"`, new tabs and splits start there;
//! by default they start in the focused terminal's directory, as before.
//! A space made before roots falls back to its first tab's directory.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT_MODE: &str = "[spaces]\nnew_tab_dir = \"root\"\n";

fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A new directory `name` in the sandbox's home, links resolved.
fn dir(sandbox: &Sandbox, name: &str) -> PathBuf {
    let d = sandbox.path("home").join(name);
    std::fs::create_dir_all(&d).unwrap();
    real(&d)
}

/// Space `name`'s root as the backend stores it; empty if none.
fn root(sandbox: &Sandbox, name: &str) -> String {
    sandbox.space(name).root.unwrap_or_default()
}

/// The directory of space `name`'s focused terminal.
fn focused_dir(sandbox: &Sandbox, name: &str) -> PathBuf {
    PathBuf::from(sandbox.terminal(name).dir.unwrap_or_default())
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// `cd` the focused terminal of session `space` to `to`, and wait until
/// the server sees it there.
fn cd(paddock: &mut Paddock, sandbox: &Sandbox, space: &str, to: &Path) {
    paddock.type_line(&format!("cd '{}'", to.display()));
    paddock.wait_for(&format!("{space} in {}", to.display()), |_| {
        focused_dir(sandbox, space) == to
    });
}

/// Wait until the focused pane of session `space` is a new one (not
/// `old`) whose shell is in `want`.
fn wait_for_new_pane_in(paddock: &Paddock, sandbox: &Sandbox, space: &str, old: &str, want: &Path) {
    paddock.wait_for(&format!("a new pane in {}", want.display()), |_| {
        let pane = sandbox.terminal(space);
        pane.id != old && pane.dir.as_deref().map(Path::new) == Some(want)
    });
}

fn focused_pane(sandbox: &Sandbox, space: &str) -> String {
    sandbox.terminal(space).id
}

/// Run git in `dir`, ignoring the user's own git configuration.
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
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repository on `main` with one commit.
fn repo(sandbox: &Sandbox) -> PathBuf {
    let dir = dir(sandbox, "repo");
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("one"), "x\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "one"]);
    dir
}

#[test]
fn the_first_space_and_c_a_c_take_the_focused_directory_as_root() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    assert_eq!(
        root(&sandbox, "space-1"),
        real(&sandbox.path("home")).to_string_lossy()
    );

    let a = dir(&sandbox, "a");
    cd(&mut paddock, &sandbox, "space-1", &a);
    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\r");
    paddock.wait_for("space-2", |_| sandbox.space_names().len() == 2);

    assert_eq!(root(&sandbox, "space-2"), a.to_string_lossy());
    // space-1's root stays where it was made.
    assert_eq!(
        root(&sandbox, "space-1"),
        real(&sandbox.path("home")).to_string_lossy()
    );
}

#[test]
fn a_worktree_space_s_root_is_its_worktree_from_the_keys_and_the_command() {
    let sandbox = Sandbox::new();
    let repo = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    cd(&mut paddock, &sandbox, "space-1", &repo);

    paddock.command(b"W");
    paddock.wait_for_popup("New worktree", "");
    paddock.send(b"feat/keys\r");
    paddock.wait_for("the worktree space", |_| {
        sandbox
            .space_names()
            .contains(&"repo/feat/keys".to_string())
    });
    let made = sandbox.path("home/.paddock/worktrees/repo/feat-keys");
    assert_eq!(
        real(Path::new(&root(&sandbox, "repo/feat/keys"))),
        real(&made)
    );

    let out = sandbox.run_in(&repo, &["worktree", "new", "feat/cli"], &[], "");
    assert!(out.status.success(), "{out:?}");
    let made = sandbox.path("home/.paddock/worktrees/repo/feat-cli");
    assert_eq!(
        real(Path::new(&root(&sandbox, "repo/feat/cli"))),
        real(&made)
    );
}

#[test]
fn by_default_new_tabs_and_splits_start_in_the_focused_directory() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let b = dir(&sandbox, "b");
    cd(&mut paddock, &sandbox, "space-1", &b);

    let old = focused_pane(&sandbox, "space-1");
    paddock.command(b"c");
    wait_for_new_pane_in(&paddock, &sandbox, "space-1", &old, &b);
    paddock.wait_for_text(PROMPT);

    let old = focused_pane(&sandbox, "space-1");
    paddock.command(b"|");
    wait_for_new_pane_in(&paddock, &sandbox, "space-1", &old, &b);
}

#[test]
fn with_root_new_tabs_and_splits_start_in_the_space_s_root() {
    let sandbox = Sandbox::new();
    sandbox.write_config(ROOT_MODE);
    let mut paddock = ready(&sandbox);
    let home = real(&sandbox.path("home"));
    let b = dir(&sandbox, "b");
    cd(&mut paddock, &sandbox, "space-1", &b);

    let old = focused_pane(&sandbox, "space-1");
    paddock.command(b"c");
    wait_for_new_pane_in(&paddock, &sandbox, "space-1", &old, &home);
    paddock.wait_for_text(PROMPT);
    cd(&mut paddock, &sandbox, "space-1", &b);

    let old = focused_pane(&sandbox, "space-1");
    paddock.command(b"-");
    wait_for_new_pane_in(&paddock, &sandbox, "space-1", &old, &home);
}

#[test]
fn c_a_r_makes_the_focused_directory_the_root_with_a_notice() {
    let sandbox = Sandbox::new();
    sandbox.write_config(ROOT_MODE);
    let mut paddock = ready(&sandbox);
    let b = dir(&sandbox, "b");
    cd(&mut paddock, &sandbox, "space-1", &b);

    paddock.command(b"R");
    paddock.wait_for("the notice", |s| {
        s.lines()
            .next()
            .unwrap_or("")
            .contains("space-1's root: ~/b")
    });
    assert_eq!(root(&sandbox, "space-1"), b.to_string_lossy());

    // New tabs now start there, wherever the focused terminal is.
    cd(&mut paddock, &sandbox, "space-1", &dir(&sandbox, "c"));
    let old = focused_pane(&sandbox, "space-1");
    paddock.command(b"c");
    wait_for_new_pane_in(&paddock, &sandbox, "space-1", &old, &b);
}

#[test]
fn the_finder_shows_a_space_s_root() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let b = dir(&sandbox, "b");
    cd(&mut paddock, &sandbox, "space-1", &b);
    paddock.command(b"R");
    paddock.wait_for("the root set", |_| {
        root(&sandbox, "space-1") == b.to_string_lossy()
    });
    // The first tab moves elsewhere: the space row still names the root.
    cd(&mut paddock, &sandbox, "space-1", &dir(&sandbox, "c"));

    paddock.command(b"g");
    paddock.wait_for("the space row with its root", |_| {
        paddock
            .full_screen()
            .lines()
            .any(|l| l.contains("space space-1") && l.contains("~/b"))
    });
}

#[test]
fn the_finder_shows_a_worktree_space_s_root_with_its_branch() {
    let sandbox = Sandbox::new();
    let repo = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    let out = sandbox.run_in(&repo, &["worktree", "new", "x"], &[], "");
    assert!(out.status.success(), "{out:?}");
    paddock.wait_for("the worktree space", |_| {
        paddock.sidebar().iter().any(|r| r.contains("repo/x"))
    });

    paddock.command(b"g");
    // The root as stored and the first tab's directory may be written
    // differently (`/tmp` is `/private/tmp`): still one place.
    paddock.wait_for("the space row with its root and branch", |_| {
        paddock
            .full_screen()
            .lines()
            .any(|l| l.contains("space repo/x") && l.contains("worktrees/repo/x  x"))
    });
}

#[test]
fn a_root_in_a_subdirectory_of_a_checkout_keeps_its_branch_in_the_finder() {
    let sandbox = Sandbox::new();
    let repo = repo(&sandbox);
    let sub = repo.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let mut paddock = ready(&sandbox);
    cd(&mut paddock, &sandbox, "space-1", &sub);
    paddock.command(b"R");
    paddock.wait_for("the root set", |_| {
        root(&sandbox, "space-1") == sub.to_string_lossy()
    });
    // The first tab leaves the root, for the top of the checkout.
    cd(&mut paddock, &sandbox, "space-1", &repo);
    paddock.wait_for("the sidebar on the repository", |_| {
        paddock.sidebar().iter().any(|r| r.ends_with("repo  main"))
    });

    paddock.command(b"g");
    paddock.wait_for("the space row with the root's branch", |_| {
        paddock
            .full_screen()
            .lines()
            .any(|l| l.contains("space space-1") && l.contains("~/repo/sub  main"))
    });
}
