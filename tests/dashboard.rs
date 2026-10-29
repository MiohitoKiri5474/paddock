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
/// The start of the dashboard's key line, as drawn while it is open.
const OPEN: &str = "j down  k up";
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

/// Report `event` for a Claude session in the terminal `terminal`
/// names, working in `cwd`.
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
    two_tabs_running(sandbox, None)
}

/// [`two_tabs`], with `line` typed into the first tab before the second
/// opens.
fn two_tabs_running(sandbox: &Sandbox, line: Option<&str>) -> Paddock {
    let mut paddock = ready(sandbox);
    if let Some(line) = line {
        paddock.type_line(line);
    }
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Wait until `text` shows anywhere, sidebar included (item states show
/// only there).
fn wait_anywhere(paddock: &Paddock, text: &str) {
    paddock.wait_for(text, |_| paddock.full_screen().contains(text));
}

/// Tab count of space-1.
fn tabs(sandbox: &Sandbox) -> usize {
    sandbox.space("space-1").tabs.len()
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

/// Ticket 33: every row shows its agent's kind, `claude` for now.
#[test]
fn rows_are_tagged_with_their_agents_kind() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    claude(&sandbox, "space-1:0", "Stop", "s1", "/w/repo");
    wait_anywhere(&paddock, "idle");
    // The sidebar leaves the agent's kind out: one line, its space last.
    paddock.wait_for("the sidebar's line", |_| {
        paddock
            .sidebar()
            .iter()
            .any(|l| l == " ● repo  idle  space-1")
    });
    assert!(
        !paddock.sidebar().iter().any(|l| l.contains("claude")),
        "{}",
        paddock.sidebar().join("\n")
    );
    open(&mut paddock);
    paddock.wait_for_text("AGENT");
    paddock.wait_for("the dashboard's tag", |s| {
        s.lines().any(|l| {
            l.contains(" repo ") && l.contains(" idle ") && l.trim_end().ends_with(" claude")
        })
    });
}

#[test]
fn o_opens_the_dashboard_and_esc_closes_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    // Empty, it still opens, and says so.
    open(&mut paddock);
    paddock.wait_for_text("no agent sessions yet");
    paddock.send(b"\x1b");
    paddock.wait_for("the empty dashboard closed", |s| !s.contains(OPEN));

    claude(&sandbox, "space-1:0", "Stop", "s1", "/w/repo");
    wait_anywhere(&paddock, "idle");
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
fn slash_filters_the_list_enter_keeps_the_filter_and_esc_clears_it() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "space-1:0", "PermissionRequest", "s1", "/w/alpha");
    claude(&sandbox, "space-1:1", "Stop", "s2", "/w/bravo");
    paddock.wait_for_tabs(&["1:● claude", "2:● claude"]);
    open(&mut paddock);
    paddock.wait_for_text(" bravo");

    paddock.send(b"/");
    paddock.wait_for_text("Enter keep  Esc clear");
    paddock.send(b"bra");
    paddock.wait_for("only bravo listed", |s| {
        s.contains(" / bra") && s.contains(" bravo") && !s.contains(" alpha")
    });
    // The selection moved to a listed line: its preview shows.
    paddock.wait_for_text("preview: bravo");
    // The filter takes the prefix too: `C-a c` types `c`, opens no tab.
    paddock.send(b"\x01c");
    paddock.wait_for_text(" / brac");
    assert_eq!(tabs(&sandbox), 2);
    paddock.send(b"\x7f");
    paddock.wait_for("the c removed", |s| !s.contains(" / brac"));
    paddock.send(b"\r");
    paddock.wait_for_text(OPEN);
    paddock.wait_for("the filter kept", |s| {
        s.contains(" / bra") && !s.contains(" alpha")
    });
    // Back in the list, keys act on it: `j` stays on bravo, the only line.
    paddock.send(b"j");
    paddock.wait_for_text("preview: bravo");

    paddock.send(b"/\x1b");
    paddock.wait_for("the filter cleared", |s| {
        !s.contains(" / ") && s.contains(" alpha") && s.contains(" bravo")
    });
    paddock.wait_for_text(OPEN);
}

#[test]
fn the_list_sits_above_the_preview_and_the_keys_at_the_bottom() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    claude(&sandbox, "space-1:0", "Stop", "s1", "/w/repo");
    wait_anywhere(&paddock, "idle");
    open(&mut paddock);
    paddock.wait_for_text("preview: repo");

    let rows = paddock.rows();
    let at = |what: &str| {
        rows.iter()
            .position(|r| r.contains(what))
            .unwrap_or_else(|| panic!("{what} not drawn:\n{}", rows.join("\n")))
    };
    let (title, row, preview) = (at(" dashboard"), at(" repo"), at("preview: repo"));
    assert_eq!(title, 1, "right under the tab bar");
    let screen = rows.join("\n");
    // The title, the column header, a blank line, then the rows.
    assert_eq!(row, title + 3, "{screen}");
    assert_eq!(rows[title + 2], "", "{screen}");
    // A blank line, then the preview in a border, its title in the top one.
    assert!(row < preview, "{screen}");
    assert_eq!(rows[preview - 1], "", "{screen}");
    assert!(rows[preview].starts_with("╭ preview: repo"), "{screen}");
    let (last, above) = (rows.len() - 1, rows.len() - 2);
    assert!(rows[above].starts_with("╰─"), "{screen}");
    assert!(rows[last].starts_with(&format!(" {OPEN}")), "{screen}");
}

#[test]
fn j_and_k_move_and_enter_jumps_to_the_rows_terminal() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    // Waiting sorts before idle.
    claude(&sandbox, "space-1:0", "PermissionRequest", "s1", "/w/alpha");
    claude(&sandbox, "space-1:1", "Stop", "s2", "/w/bravo");
    paddock.wait_for_tabs(&["1:● claude", "2:● claude"]);

    open(&mut paddock);
    paddock.wait_for_text("preview: alpha");
    // A blank line parts the two sessions.
    let rows = paddock.rows();
    let at = |label: &str| {
        rows.iter()
            .position(|r| r.starts_with(&format!(" {label}")))
    };
    let (alpha, bravo) = (at("alpha"), at("bravo"));
    assert_eq!(bravo, alpha.map(|a| a + 2), "{}", rows.join("\n"));
    assert_eq!(rows[alpha.unwrap_or_default() + 1], "");
    paddock.send(b"j");
    paddock.wait_for_text("preview: bravo");
    paddock.send(b"k");
    paddock.wait_for_text("preview: alpha");

    paddock.send(b"\r");
    paddock.wait_for_current_tab("1:● claude");
    paddock.wait_for("the dashboard closed", |s| !s.contains(OPEN));
}

#[test]
fn d_dismisses_a_row_and_it_stays_gone_after_reopening() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "space-1:0", "PermissionRequest", "s1", "/w/alpha");
    claude(&sandbox, "space-1:1", "Stop", "s2", "/w/bravo");
    paddock.wait_for_tabs(&["1:● claude", "2:● claude"]);

    open(&mut paddock);
    paddock.wait_for_text("alpha");
    paddock.wait_for_text("bravo");
    paddock.send(b"D");
    paddock.wait_for("alpha dismissed", |s| !s.contains("alpha"));
    paddock.wait_for_text("preview: bravo");
    assert_eq!(tabs(&sandbox), 2, "panes kept");

    paddock.detach_keys();
    assert!(paddock.wait_exit());
    let mut again = ready(&sandbox);
    wait_anywhere(&again, "idle");
    open(&mut again);
    again.wait_for_text("preview: bravo");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!again.screen().contains("alpha"), "{}", again.screen());
}

/// Three tabs, each with a waiting Claude session, alpha, bravo and
/// charlie in tab order, listed in that order; the third tab current.
fn three_waiting(sandbox: &Sandbox) -> Paddock {
    let mut paddock = two_tabs(sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("3:home");
    paddock.wait_for_text(PROMPT);
    for (tab, session, dir) in [(0, "s1", "alpha"), (1, "s2", "bravo"), (2, "s3", "charlie")] {
        let terminal = format!("space-1:{tab}");
        claude(
            sandbox,
            &terminal,
            "PermissionRequest",
            session,
            &format!("/w/{dir}"),
        );
    }
    paddock.wait_for_tabs(&["1:● claude", "2:● claude", "3:● claude"]);
    paddock
}

#[test]
fn a_closed_terminals_session_leaves_and_the_selection_moves_to_the_next() {
    let sandbox = Sandbox::new();
    // No `claude agents` snapshots, whose polls would move the selection
    // anyway.
    sandbox.set_agents("not json");
    let mut paddock = three_waiting(&sandbox);
    open(&mut paddock);
    paddock.wait_for_text("preview: alpha");
    paddock.send(b"j");
    paddock.wait_for_text("preview: bravo");

    sandbox.end_terminal("space-1:1");
    paddock.wait_for("bravo left the list", |s| !s.contains("bravo"));
    // The session that took its place is selected, not the first.
    paddock.wait_for_text("preview: charlie");
    assert!(paddock.screen().contains("alpha"), "{}", paddock.screen());
}

#[test]
fn a_session_whose_claude_quit_leaves_while_its_terminal_stays() {
    let sandbox = Sandbox::new();
    sandbox.set_agents("not json");
    let mut paddock = three_waiting(&sandbox);
    open(&mut paddock);
    paddock.send(b"j");
    paddock.wait_for_text("preview: bravo");

    claude(&sandbox, "space-1:1", "SessionEnd", "s2", "/w/bravo");
    paddock.wait_for("bravo left everywhere", |_| {
        !paddock.full_screen().contains("bravo")
    });
    paddock.wait_for_text("preview: charlie");
    assert_eq!(tabs(&sandbox), 3, "terminal kept");

    // A new Claude in the same terminal is a new session, listed again.
    claude(&sandbox, "space-1:1", "SessionStart", "s4", "/w/delta");
    paddock.wait_for_text("delta");
}

#[test]
fn the_preview_shows_the_selected_terminal_live() {
    let sandbox = Sandbox::new();
    // No `claude agents` snapshots, whose polls redraw the screen anyway.
    sandbox.set_agents("not json");
    // The first tab prints once `go` exists.
    let go = sandbox.path("go");
    let wait = format!(
        "while [ ! -e '{}' ]; do sleep 0.05; done; echo live-$((6*7))",
        go.display()
    );
    let mut paddock = two_tabs_running(&sandbox, Some(&wait));
    claude(&sandbox, "space-1:0", "Stop", "s1", "/w/repo");
    wait_anywhere(&paddock, "done");
    open(&mut paddock);
    paddock.wait_for_text("preview: repo");

    // The first tab is not current: only the preview can show its pane.
    std::fs::write(&go, "").unwrap();
    paddock.wait_for_text("live-42");
    assert_eq!(paddock.current_tab(), "2:home");
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

/// Two tabs, the first one's Claude done and its terminal writing every
/// byte it reads to the returned file, ESC shown as `^[`.
fn two_tabs_with_cat(sandbox: &Sandbox) -> (Paddock, std::path::PathBuf) {
    let out = sandbox.path("typed.txt");
    let cat = format!("cat -v > {}", out.display());
    let paddock = two_tabs_running(sandbox, Some(&cat));
    claude(sandbox, "space-1:0", "Stop", "s1", "/w/repo");
    wait_anywhere(&paddock, "done");
    (paddock, out)
}

#[test]
fn i_types_into_the_previewed_terminal_and_esc_is_never_sent() {
    let sandbox = Sandbox::new();
    let (mut paddock, out) = two_tabs_with_cat(&sandbox);

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
    assert_eq!(paddock.current_tab(), "2:home", "no jump");
}

#[test]
fn esc_and_a_key_in_one_read_leave_interactive_preview_unsent() {
    let sandbox = Sandbox::new();
    let (mut paddock, out) = two_tabs_with_cat(&sandbox);

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
    claude(&sandbox, "space-1:0", "UserPromptSubmit", "s1", "/w/repo");
    wait_anywhere(&paddock, "running");

    open(&mut paddock);
    paddock.send(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"y");
    paddock.wait_for("its pane closed", |_| tabs(&sandbox) == 1);
    // Its session ended with its terminal: it left the list.
    paddock.wait_for_text("no agent sessions yet");
    paddock.wait_for_text(OPEN);
}

#[test]
fn a_click_outside_the_close_question_cancels_it_back_to_the_dashboard() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "space-1:0", "UserPromptSubmit", "s1", "/w/repo");
    wait_anywhere(&paddock, "running");

    open(&mut paddock);
    paddock.send(b"x");
    paddock.wait_for_popup("Close tab", "an agent is running: close? y/n");
    // Outside the 40x3 box centred on the 100x30 screen.
    paddock.click_screen(80, 25);

    paddock.wait_for_no_popup();
    paddock.wait_for_text(OPEN);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(tabs(&sandbox), 2);
}

#[test]
fn f_forks_the_selected_session() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    let dir = sandbox.path("home/repo");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let id = "0000cccc-0000-4000-8000-00000000000c";
    claude(&sandbox, "space-1:0", "Stop", id, dir.to_str().unwrap());
    wait_anywhere(&paddock, "done");

    open(&mut paddock);
    paddock.send(b"f");
    paddock.wait_for_current_tab("3:repo");
    assert_eq!(
        sandbox.claude_calls(),
        [format!("{} --resume {id} --fork-session", dir.display())]
    );
    paddock.wait_for("the dashboard closed", |s| !s.contains(OPEN));
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

#[test]
fn task_rows_show_base_diff_files_age_and_merged() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    git(&dir, &["switch", "-q", "-c", "feat/x"]);
    commit(&dir, "a");
    let mut paddock = ready(&sandbox);
    claude(
        &sandbox,
        "space-1:0",
        "UserPromptSubmit",
        SESSION,
        dir.to_str().unwrap(),
    );
    wait_anywhere(&paddock, "running");

    open(&mut paddock);
    paddock.wait_for_text("feat/x");
    // BASE, FILES and AGE, in their columns.
    paddock.wait_for("feat/x's fields", |s| {
        task_line(s, "feat/x") == "feat/x +1 −0 1 now"
    });

    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, "b");
    git(&dir, &["merge", "-q", "--no-edit", "feat/x"]);
    // No hook: the dashboard's own refresh picks this up.
    paddock.wait_for("feat/x merged", |s| {
        task_line(s, "feat/x").ends_with(" merged")
    });
}
