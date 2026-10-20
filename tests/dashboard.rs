//! The dashboard (`C-a o`): rows with their details, a live preview of the
//! selected row's terminal, and interactive preview, driven by recorded hook
//! payloads against real temporary git repositories.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
/// The dashboard's title line, as drawn while it is open.
const OPEN: &str = "dashboard  j/k move";
/// The preview's title while keys go to the terminal.
const TYPING: &str = "typing into";
const ASK: &str = "close? y/n";

/// A recorded payload for session `session` in directory `cwd`.
fn payload(name: &str, session: &str, cwd: &str) -> String {
    let path = if name.starts_with("PermissionRequest") {
        format!("{OURS}/{name}.json")
    } else {
        format!("{SPIKE}/{name}.json")
    };
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .replace(SESSION, session)
        .replace("\"/tmp/repo\"", &format!("\"{cwd}\""))
}

/// Report `event` for a Claude session in `terminal`, working in `cwd`.
fn claude(sandbox: &Sandbox, terminal: &str, event: &str, session: &str, cwd: &str) {
    sandbox.hook(terminal, event, &payload(event, session, cwd));
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Two tabs, the second current.
fn two_tabs(sandbox: &Sandbox) -> Paddock {
    let mut paddock = ready(sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:terminal-2");
    paddock.wait_for_text(PROMPT);
    paddock
}

fn open(paddock: &mut Paddock) {
    paddock.command(b"o");
    paddock.wait_for_text(OPEN);
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

/// A repository on `main` with one commit.
fn repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    commit(&dir, "one");
    dir
}

#[test]
fn o_opens_the_dashboard_and_esc_closes_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    // Empty, it still opens, and says so.
    open(&mut paddock);
    paddock.wait_for_text("no Claude sessions yet");
    paddock.send(b"\x1b");
    paddock.wait_for("the empty dashboard closed", |s| !s.contains(OPEN));

    claude(&sandbox, "terminal-1", "Stop", "s1", "/w/repo");
    paddock.wait_for_text("idle");
    open(&mut paddock);
    paddock.wait_for_text("preview: repo");

    paddock.send(b"\x1b");
    paddock.wait_for("the dashboard closed", |s| !s.contains(OPEN));
    // The keyboard is back in the terminal.
    paddock.type_line("echo back-$((1+1))");
    paddock.wait_for_text("back-2");

    open(&mut paddock);
    paddock.command(b"o");
    paddock.wait_for("the dashboard closed again", |s| !s.contains(OPEN));
}

#[test]
fn j_and_k_move_and_enter_jumps_to_the_rows_terminal() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    // Waiting sorts before idle.
    claude(
        &sandbox,
        "terminal-1",
        "PermissionRequest",
        "s1",
        "/w/alpha",
    );
    claude(&sandbox, "terminal-2", "Stop", "s2", "/w/bravo");
    paddock.wait_for_text("waiting");
    paddock.wait_for_text("idle");

    open(&mut paddock);
    paddock.wait_for_text("preview: alpha");
    paddock.send(b"j");
    paddock.wait_for_text("preview: bravo");
    paddock.send(b"k");
    paddock.wait_for_text("preview: alpha");

    paddock.send(b"\r");
    paddock.wait_for_current_tab("1:terminal-1");
    paddock.wait_for("the dashboard closed", |s| !s.contains(OPEN));
}

#[test]
fn d_dismisses_a_row_and_it_stays_gone_after_reopening() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(
        &sandbox,
        "terminal-1",
        "PermissionRequest",
        "s1",
        "/w/alpha",
    );
    claude(&sandbox, "terminal-2", "Stop", "s2", "/w/bravo");
    paddock.wait_for_text("alpha");
    paddock.wait_for_text("bravo");

    open(&mut paddock);
    paddock.send(b"D");
    paddock.wait_for("alpha dismissed", |s| !s.contains("alpha"));
    paddock.wait_for_text("preview: bravo");
    assert_eq!(
        sandbox.session_names(),
        ["terminal-1", "terminal-2"],
        "terminals kept"
    );

    paddock.detach_keys();
    assert!(paddock.wait_exit());
    let mut again = ready(&sandbox);
    again.wait_for_text("bravo");
    open(&mut again);
    again.wait_for_text("preview: bravo");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!again.screen().contains("alpha"), "{}", again.screen());
}

#[test]
fn the_preview_shows_the_selected_terminal_live() {
    let sandbox = Sandbox::new();
    // No `claude agents` snapshots, whose polls redraw the screen anyway.
    sandbox.set_agents("not json");
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-1", "Stop", "s1", "/w/repo");
    paddock.wait_for_text("idle");
    open(&mut paddock);
    paddock.wait_for_text("preview: repo");

    // terminal-1 is not in the current tab: only the preview can show it.
    sandbox.tmux(&[
        "send-keys",
        "-t",
        "terminal-1",
        "echo live-$((6*7))",
        "Enter",
    ]);
    paddock.wait_for_text("live-42");
    assert_eq!(paddock.current_tab(), "2:terminal-2");
}

/// Wait until `path` holds exactly `want`.
fn wait_for_file(path: &Path, want: &str) {
    let start = Instant::now();
    while start.elapsed() < harness::TIMEOUT {
        if std::fs::read_to_string(path).is_ok_and(|t| t == want) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{} is {:?}", path.display(), std::fs::read_to_string(path));
}

#[test]
fn i_types_into_the_previewed_terminal_and_esc_is_never_sent() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-1", "Stop", "s1", "/w/repo");
    paddock.wait_for_text("idle");
    // terminal-1 writes every byte it reads to a file, ESC shown as `^[`.
    let out = sandbox.path("typed.txt");
    let cat = format!("cat -v > {}", out.display());
    sandbox.tmux(&["send-keys", "-t", "terminal-1", &cat, "Enter"]);

    open(&mut paddock);
    paddock.send(b"I");
    paddock.wait_for_text(TYPING);
    paddock.send(b"qz");
    paddock.wait_for_text("qz");
    paddock.send(b"\x1b");
    paddock.wait_for("interactive preview left", |s| !s.contains(TYPING));
    paddock.wait_for_text(OPEN);

    paddock.send(b"I");
    paddock.wait_for_text(TYPING);
    paddock.send(b"wv\r");
    paddock.send(b"\x04");
    wait_for_file(&out, "qzwv\n");
    assert!(!paddock.screen().contains("^["), "{}", paddock.screen());
    assert_eq!(paddock.current_tab(), "2:terminal-2", "no jump");
}

#[test]
fn esc_and_a_key_in_one_read_leave_interactive_preview_unsent() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-1", "Stop", "s1", "/w/repo");
    paddock.wait_for_text("idle");
    let out = sandbox.path("typed.txt");
    let cat = format!("cat -v > {}", out.display());
    sandbox.tmux(&["send-keys", "-t", "terminal-1", &cat, "Enter"]);

    open(&mut paddock);
    paddock.send(b"I");
    paddock.wait_for_text(TYPING);
    // One write: crossterm reads it as Alt+q, not Esc then q.
    paddock.send(b"\x1bq");
    paddock.wait_for("interactive preview left", |s| !s.contains(TYPING));
    paddock.wait_for_text(OPEN);

    paddock.send(b"I");
    paddock.wait_for_text(TYPING);
    paddock.send(b"ok\r");
    paddock.send(b"\x04");
    // A leak would read `^[qok`.
    wait_for_file(&out, "ok\n");
}

#[test]
fn x_asks_while_claude_runs_then_returns_to_the_dashboard() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-1", "UserPromptSubmit", "s1", "/w/repo");
    paddock.wait_for_text("running");

    open(&mut paddock);
    paddock.send(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"y");
    paddock.wait_for("terminal-1 closed", |_| {
        sandbox.session_names() == ["terminal-2"]
    });
    paddock.wait_for_text("repo: no terminal");
    paddock.wait_for_text(OPEN);
}

#[test]
fn f_forks_the_selected_session() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    let dir = sandbox.path("home/repo");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let id = "0000cccc-0000-4000-8000-00000000000c";
    claude(&sandbox, "terminal-1", "Stop", id, dir.to_str().unwrap());
    paddock.wait_for_text("idle");

    open(&mut paddock);
    paddock.send(b"f");
    paddock.wait_for_current_tab("3:terminal-3");
    assert_eq!(
        sandbox.claude_calls(),
        [format!("{} --resume {id} --fork-session", dir.display())]
    );
    paddock.wait_for("the dashboard closed", |s| !s.contains(OPEN));
}

#[test]
fn task_rows_show_base_diff_files_age_and_merged() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    git(&dir, &["switch", "-q", "-c", "feat/x"]);
    commit(&dir, "a");
    let mut paddock = ready(&sandbox);
    claude(
        &sandbox,
        "terminal-1",
        "UserPromptSubmit",
        SESSION,
        dir.to_str().unwrap(),
    );
    paddock.wait_for_text("feat/x");

    open(&mut paddock);
    paddock.wait_for_text("base +1 −0  1 file  now");
    assert!(!paddock.screen().contains("merged"));

    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, "b");
    git(&dir, &["merge", "-q", "--no-edit", "feat/x"]);
    // No hook: the dashboard's own refresh picks this up.
    paddock.wait_for_text("merged");
}
