//! Ticket 36: pull request status on the dashboard, read with a stub `gh`
//! (see the harness's `write_gh_stub`) against real temporary git
//! repositories. The real `gh` and its login are never touched.

mod harness;

use harness::{Paddock, Sandbox, PROMPT, ROWS};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
/// Columns of the screen in these tests: the session area beside the
/// sidebar fits every column with the PR in full (81 cells for this list).
const WIDE: u16 = 120;
/// The start of the dashboard's key line, as drawn while it is open.
const OPEN: &str = "j down  k up";
/// `feat/x`'s pull request: open, checks passing, approved; and an older
/// one of `main`'s.
const PRS: &str = r#"[
  {"number":123,"state":"OPEN","isDraft":false,"headRefName":"feat/x",
   "reviewDecision":"APPROVED","url":"https://github.com/o/r/pull/123",
   "statusCheckRollup":[{"__typename":"CheckRun","name":"test","workflowName":"CI",
     "status":"COMPLETED","conclusion":"SUCCESS","startedAt":"2026-10-22T10:00:00Z"}]},
  {"number":5,"state":"MERGED","isDraft":false,"headRefName":"main",
   "reviewDecision":"","url":"https://github.com/o/r/pull/5","statusCheckRollup":[]}
]"#;

/// Report `UserPromptSubmit` for a Claude session in the first terminal,
/// working in `cwd`.
fn running_in(sandbox: &Sandbox, cwd: &Path) {
    let path = format!("{SPIKE}/UserPromptSubmit.json");
    let payload = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .replace("\"/tmp/repo\"", &format!("\"{}\"", cwd.display()));
    sandbox.hook("space-1:0", "UserPromptSubmit", &payload);
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

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), "x\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// A repository with one commit on `main` and one on `feat/x`, checked
/// out.
fn repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    commit(&dir, "one");
    git(&dir, &["switch", "-q", "-c", "feat/x"]);
    commit(&dir, "a");
    dir
}

/// paddock with a session working on `feat/x` in `dir`, the dashboard
/// open and its git fields read.
fn dashboard(sandbox: &Sandbox, dir: &Path) -> Paddock {
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    // Wide enough for every column, the PR in full, beside the sidebar.
    paddock.resize(WIDE, ROWS);
    running_in(sandbox, dir);
    paddock.wait_for("running", |_| paddock.full_screen().contains("running"));
    open(&mut paddock);
    paddock.wait_for("feat/x's git fields", |s| {
        task_line(s, "feat/x").starts_with("feat/x +1 −0 1 now")
    });
    paddock
}

fn open(paddock: &mut Paddock) {
    paddock.command(b"o");
    paddock.wait_for_text(OPEN);
}

/// The list line of the task row on `branch` in `screen`, spaces collapsed;
/// empty if not drawn.
fn task_line(screen: &str, branch: &str) -> String {
    screen
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|l| l.starts_with(branch))
        .unwrap_or_default()
}

/// The `gh pr list` paddock runs, in `dir`.
fn pr_list_in(dir: &Path) -> String {
    let dir = dir.canonicalize().unwrap();
    format!(
        "{} pr list --state all --limit 20 --json \
         number,state,isDraft,headRefName,statusCheckRollup,reviewDecision,url",
        dir.display()
    )
}

/// `gh.log` in the sandbox's state directory.
fn gh_log(sandbox: &Sandbox) -> Vec<String> {
    std::fs::read_to_string(sandbox.path("state/paddock/gh.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Hold for `d`, for counts that must not change meanwhile.
fn hold(d: Duration) {
    std::thread::sleep(d);
}

#[test]
fn a_branchs_pr_shows_in_the_pr_column_coloured_by_its_state() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let paddock = dashboard(&sandbox, &dir);
    paddock.wait_for_text(" PR ");
    paddock.wait_for("feat/x's PR", |s| {
        task_line(s, "feat/x") == "feat/x +1 −0 1 now #123 open ✓ approved"
    });
    assert_eq!(
        sandbox.gh_calls(),
        [pr_list_in(&dir)],
        "one call, in the repository"
    );
    let (x, y) = paddock.find("#123").expect("drawn");
    let (fg, _) = paddock.colors(y, paddock.left() + x);
    assert_eq!(fg, vt100::Color::Idx(2), "open is green");
    // Nothing went wrong: no log.
    assert!(gh_log(&sandbox).is_empty());
}

#[test]
fn without_a_pr_a_merged_branch_reads_merged() {
    let sandbox = Sandbox::new();
    // gh answers, but has nothing for feat/x.
    sandbox.set_gh("[]");
    let dir = repo(&sandbox);
    let paddock = dashboard(&sandbox, &dir);
    sandbox.wait_for_gh_calls(1);
    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, "b");
    git(&dir, &["merge", "-q", "--no-edit", "feat/x"]);
    paddock.wait_for("feat/x merged", |s| {
        task_line(s, "feat/x").ends_with(" merged")
    });
}

#[test]
fn a_narrow_dashboard_shortens_the_pr_then_drops_it() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    paddock.wait_for("the full PR", |s| {
        task_line(s, "feat/x").ends_with("#123 open ✓ approved")
    });
    paddock.resize(100, ROWS);
    paddock.wait_for("the short PR", |s| {
        task_line(s, "feat/x").ends_with(" #123 ✓") && s.contains(" PR")
    });
    paddock.resize(86, ROWS);
    paddock.wait_for("no PR column", |s| {
        !s.contains("#123") && !s.contains(" PR ") && task_line(s, "feat/x").starts_with("feat/x")
    });
    paddock.resize(WIDE, ROWS);
    paddock.wait_for("the full PR again", |s| {
        task_line(s, "feat/x").ends_with("#123 open ✓ approved")
    });
}

#[test]
fn gh_is_asked_once_a_minute_by_default_and_toggling_the_dashboard_adds_no_call() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    paddock.wait_for("feat/x's PR", |s| task_line(s, "feat/x").contains("#123"));
    hold(Duration::from_secs(3));
    assert_eq!(sandbox.gh_calls().len(), 1, "cached for 60 s");
    // Closed, nothing is asked; opened again within the 10 s floor, nor
    // then (past it, at once: see `app::prs`'s unit tests).
    paddock.send(b"\x1b");
    paddock.wait_for("closed", |s| !s.contains(OPEN));
    hold(Duration::from_secs(2));
    assert_eq!(sandbox.gh_calls().len(), 1, "nothing while closed");
    for _ in 0..3 {
        open(&mut paddock);
        paddock.send(b"\x1b");
        paddock.wait_for("closed", |s| !s.contains(OPEN));
    }
    open(&mut paddock);
    hold(Duration::from_secs(2));
    assert_eq!(sandbox.gh_calls().len(), 1, "toggling within the floor");
    paddock.wait_for("feat/x's PR still", |s| {
        task_line(s, "feat/x").contains("#123")
    });
}

#[test]
fn refresh_secs_sets_how_often_gh_is_asked() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[github]\nrefresh_secs = 1\n");
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let _paddock = dashboard(&sandbox, &dir);
    let start = Instant::now();
    let calls = sandbox.wait_for_gh_calls(3);
    assert!(start.elapsed() < Duration::from_secs(8));
    assert!(calls.iter().all(|c| *c == pr_list_in(&dir)), "{calls:?}");
}

#[test]
fn refresh_secs_zero_never_runs_gh_and_p_says_there_is_no_pr() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[github]\nrefresh_secs = 0\n");
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    paddock.send(b"p");
    paddock.wait_for_text("no pull request for feat/x");
    hold(Duration::from_secs(2));
    assert!(sandbox.gh_calls().is_empty(), "{:?}", sandbox.gh_calls());
    assert!(!task_line(&paddock.screen(), "feat/x").contains('#'));
}

#[test]
fn p_opens_the_sessions_pr_with_gh_in_its_repository() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    paddock.wait_for("feat/x's PR", |s| task_line(s, "feat/x").contains("#123"));
    paddock.send(b"p");
    let calls = sandbox.wait_for_gh_calls(2);
    let dir = dir.canonicalize().unwrap();
    assert_eq!(calls[1], format!("{} pr view --web 123", dir.display()));
    // paddock runs with GH_REPO set; gh never sees it, else every
    // repository would be asked about that one.
    assert_eq!(sandbox.gh_repo_seen(), ["<unset>", "<unset>"]);
    // The dashboard stays, with no notice.
    hold(Duration::from_millis(500));
    let screen = paddock.full_screen();
    assert!(screen.contains(OPEN));
    assert!(!screen.contains("cannot open"), "{screen}");
}

#[test]
fn p_while_an_open_runs_is_ignored() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    sandbox.slow_gh_view();
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    paddock.wait_for("feat/x's PR", |s| task_line(s, "feat/x").contains("#123"));
    paddock.send(b"p");
    sandbox.wait_for_gh_calls(2);
    paddock.send(b"p");
    paddock.send(b"p");
    hold(Duration::from_secs(1));
    let views = |calls: Vec<String>| calls.iter().filter(|c| c.contains("pr view")).count();
    assert_eq!(views(sandbox.gh_calls()), 1, "no stacking");
    // Once it is done (3 s), `p` opens again.
    hold(Duration::from_secs(3));
    paddock.send(b"p");
    assert_eq!(views(sandbox.wait_for_gh_calls(3)), 2);
}

#[test]
fn without_gh_the_dashboard_shows_git_alone_and_logs_one_line() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[github]\nrefresh_secs = 1\n");
    sandbox.remove_gh();
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    let start = Instant::now();
    while gh_log(&sandbox).is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10), "nothing logged");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Tried every second meanwhile, logged once.
    hold(Duration::from_secs(3));
    let log = gh_log(&sandbox);
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("cannot run"), "{log:?}");
    let screen = paddock.full_screen();
    assert!(!screen.contains("cannot"), "no notice: {screen}");
    assert_eq!(task_line(&paddock.screen(), "feat/x"), "feat/x +1 −0 1 now");
    // `p` has no PR to open.
    paddock.send(b"p");
    paddock.wait_for_text("no pull request for feat/x");
}

#[test]
fn a_failing_gh_shows_git_alone_with_no_notice() {
    let sandbox = Sandbox::new();
    // No answer set: the stub fails as a logged-out gh.
    let dir = repo(&sandbox);
    let paddock = dashboard(&sandbox, &dir);
    sandbox.wait_for_gh_calls(1);
    let start = Instant::now();
    while gh_log(&sandbox).is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10), "nothing logged");
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = gh_log(&sandbox);
    assert_eq!(log.len(), 1);
    assert!(log[0].contains("gh auth login"), "gh's reason: {log:?}");
    hold(Duration::from_millis(500));
    assert_eq!(task_line(&paddock.screen(), "feat/x"), "feat/x +1 −0 1 now");
    assert!(!paddock.full_screen().contains("auth"));
}

#[test]
fn a_slow_gh_never_stalls_the_screen_and_is_given_up_after_5_s() {
    let sandbox = Sandbox::new();
    sandbox.slow_gh();
    let dir = repo(&sandbox);
    let mut paddock = dashboard(&sandbox, &dir);
    sandbox.wait_for_gh_calls(1);
    // While gh hangs, keys still work at once.
    let start = Instant::now();
    paddock.send(b"\x1b");
    paddock.wait_for("closed", |s| !s.contains(OPEN));
    open(&mut paddock);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    // And git keeps being read: a new commit shows.
    commit(&dir, "c");
    paddock.wait_for("feat/x's two commits", |s| {
        task_line(s, "feat/x").starts_with("feat/x +2 −0 2 now")
    });
    let start = Instant::now();
    while gh_log(&sandbox).is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10), "never gave up");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(gh_log(&sandbox)[0].contains("gh did not finish in 5s"));
}

// The sidebar marks each space's branch's PR, read while the
// dashboard is closed too, as long as the sidebar shows.

/// A repository at `~/r` (short enough for the mark beside `feat/x` in
/// the sidebar), on `feat/x`.
fn home_repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("home/r");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    commit(&dir, "one");
    git(&dir, &["switch", "-q", "-c", "feat/x"]);
    dir
}

/// The sidebar line naming `~/r`, spaces collapsed; empty if none.
fn space_place(paddock: &Paddock) -> String {
    paddock
        .sidebar()
        .iter()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|l| l.starts_with("~/r"))
        .unwrap_or_default()
}

/// Wait until the sidebar line naming `~/r` passes `pred`.
fn wait_for_place(paddock: &Paddock, what: &str, pred: impl Fn(&str) -> bool) {
    let start = Instant::now();
    while !pred(&space_place(paddock)) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{what}: {:?}",
            paddock.sidebar()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// paddock with its first space's shell in `~/r`.
fn in_home_repo(sandbox: &Sandbox) -> Paddock {
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line("cd ~/r");
    paddock
}

#[test]
fn the_sidebar_marks_a_spaces_pr_with_the_dashboard_closed() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    let dir = home_repo(&sandbox);
    let paddock = in_home_repo(&sandbox);
    wait_for_place(&paddock, "feat/x's PR", |l| l == "~/r feat/x #123 ✓");
    assert_eq!(sandbox.gh_calls(), [pr_list_in(&dir)], "one call");
    assert!(!paddock.full_screen().contains(OPEN), "never opened");
    let rows = paddock.sidebar();
    let (y, line) = rows
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains("#123"))
        .expect("drawn");
    let x = line.chars().position(|c| c == '#').unwrap();
    let (fg, _) = paddock.colors(y as u16, x as u16);
    assert_eq!(fg, vt100::Color::Idx(2), "open is green");
    // A branch without a PR: nothing extra.
    sandbox.set_gh("[]");
    git(&dir, &["switch", "-q", "-c", "feat/none"]);
    wait_for_place(&paddock, "no mark", |l| l == "~/r feat/none");
}

#[test]
fn with_the_sidebar_hidden_and_the_dashboard_closed_gh_is_never_asked() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[github]\nrefresh_secs = 1\n");
    sandbox.set_gh(PRS);
    let dir = home_repo(&sandbox);
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.command(b"b");
    paddock.wait_for("sidebar hidden", |_| paddock.left() == 0);
    paddock.type_line("cd ~/r");
    hold(Duration::from_secs(3));
    assert!(sandbox.gh_calls().is_empty(), "{:?}", sandbox.gh_calls());
    // Shown again: asked at once, then every second.
    paddock.command(b"b");
    wait_for_place(&paddock, "feat/x's PR", |l| l == "~/r feat/x #123 ✓");
    let calls = sandbox.wait_for_gh_calls(2);
    assert!(calls.iter().all(|c| *c == pr_list_in(&dir)), "{calls:?}");
    // Hidden again: no more.
    paddock.command(b"b");
    paddock.wait_for("sidebar hidden", |_| paddock.left() == 0);
    hold(Duration::from_millis(1500));
    let before = sandbox.gh_calls().len();
    hold(Duration::from_secs(3));
    assert_eq!(sandbox.gh_calls().len(), before, "none while nothing shows");
}

#[test]
fn refresh_secs_zero_marks_no_space_and_never_runs_gh() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[github]\nrefresh_secs = 0\n");
    sandbox.set_gh(PRS);
    home_repo(&sandbox);
    let paddock = in_home_repo(&sandbox);
    wait_for_place(&paddock, "feat/x", |l| l == "~/r feat/x");
    hold(Duration::from_secs(2));
    assert_eq!(space_place(&paddock), "~/r feat/x");
    assert!(sandbox.gh_calls().is_empty(), "{:?}", sandbox.gh_calls());
}
