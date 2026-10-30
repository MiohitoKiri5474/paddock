//! Ticket 03: one terminal, drawn by paddock, on paddock's server.

mod harness;

use harness::{Sandbox, PROMPT};

/// Whether process `pid` runs.
fn alive(pid: &str) -> bool {
    let status = std::process::Command::new("kill")
        .args(["-0", pid])
        .status()
        .expect("run kill");
    status.success()
}

#[test]
fn launching_shows_a_working_shell() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("echo $((6 * 7))");

    paddock.wait_for("command output", |s| s.lines().any(|l| l.trim() == "42"));
    assert_eq!(sandbox.space_names(), ["space-1"]);
}

#[test]
fn screen_title_escapes_are_not_drawn() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    // oh-my-zsh sets the title like this inside tmux; it must not land on screen.
    paddock.type_line("printf '\\033k%s\\033\\\\' cla''ude; echo done");

    paddock.wait_for("command output", |s| {
        s.lines()
            .any(|l| l.trim_end().ends_with("done") && !l.contains("echo"))
    });
    let rows = paddock.rows();
    assert!(
        rows.iter().any(|l| l == "done") && !rows.iter().any(|l| l.contains("claude")),
        "title drawn on screen:\n{}",
        paddock.screen()
    );
}

#[test]
fn typed_text_and_utf8_reach_the_shell() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("printf '%s\\n' 漢字-ok | tr a-z A-Z");

    paddock.wait_for_text("漢字-OK");
}

#[test]
fn mouse_wheel_scrolls_the_terminal_history() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line("seq 1 200");
    paddock.wait_for("seq to finish", |s| s.lines().any(|l| l.trim() == "200"));

    for _ in 0..10 {
        paddock.wheel(true, 10, 10);
    }

    // Before: rows show 172..200 and the prompt. 10 notches of 3 lines move that up 30.
    paddock.wait_for("scrolled view", |s| s.contains("[30/"));
    let rows = paddock.rows();
    assert_eq!(
        rows.last().map(String::as_str),
        Some("171"),
        "{}",
        paddock.screen()
    );
    assert!(
        !rows.iter().any(|r| r.starts_with(PROMPT)),
        "prompt still visible"
    );

    for _ in 0..10 {
        paddock.wheel(false, 10, 10);
    }
    paddock.wait_for("back at the bottom", |s| {
        !s.contains("[") && s.contains(PROMPT)
    });
}

#[test]
fn typing_while_scrolled_returns_to_the_live_screen() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line("seq 1 100");
    paddock.wait_for("seq", |s| s.lines().any(|l| l.trim() == "100"));
    paddock.wheel(true, 5, 5);
    paddock.wait_for_text("[3/");

    paddock.type_line("echo back-live");

    paddock.wait_for("live screen", |s| {
        s.contains("back-live") && !s.contains("[3/")
    });
}

#[test]
fn mouse_wheel_goes_to_programs_that_ask_for_the_mouse() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    // A program that turns on SGR mouse reporting and shows what it receives.
    paddock.type_line("printf '\\033[?1000h\\033[?1006h'; stty -echo; cat -v");
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Outer row 10 is terminal row 9 (the tab bar takes row 0): reported 1-based as 10.
    paddock.wheel(true, 4, 10);
    paddock.send(b"\r"); // cat reads whole lines

    paddock.wait_for_text("^[[<64;5;10M");
}

#[test]
fn detaching_keeps_the_shell_and_relaunching_restores_history() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo marker-before-detach; seq 1 300");
    paddock.wait_for("seq", |s| s.lines().any(|l| l.trim() == "300"));
    let before = sandbox.shells();

    paddock.detach_keys();

    assert!(paddock.wait_exit(), "paddock should exit cleanly on C-a d");
    assert_eq!(sandbox.shells(), before, "the same shell is still running");
    let pid = before[0].rsplit(' ').next().unwrap_or_default();
    assert!(alive(pid), "the shell is alive: {before:?}");

    let mut again = sandbox.launch();
    again.wait_for("restored screen", |s| s.lines().any(|l| l.trim() == "300"));
    assert_eq!(sandbox.space_names(), ["space-1"]);

    // Scroll back to the very first line of the restored history.
    for _ in 0..120 {
        again.wheel(true, 10, 10);
    }
    again.wait_for_text("marker-before-detach");

    // The reattached terminal still takes input.
    for _ in 0..120 {
        again.wheel(false, 10, 10);
    }
    again.type_line("echo after-reattach");
    again.wait_for_text("after-reattach\n");
}

#[test]
fn exiting_the_shell_quits_paddock_and_the_next_launch_starts_a_new_one() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("exit");

    assert!(paddock.wait_exit());
    assert!(sandbox.space_names().is_empty());
    let mut again = sandbox.launch();
    again.wait_for_text(PROMPT);
    again.type_line("echo fresh");
    again.wait_for_text("fresh\n");
}

#[test]
fn resizing_the_outer_terminal_resizes_the_terminal() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.resize(80, 20);
    let size = || sandbox.terminal("space-1").size();
    let start = std::time::Instant::now();
    while size() != "48x19" {
        assert!(start.elapsed() < harness::TIMEOUT, "pane is {}", size());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    paddock.type_line("stty size");

    paddock.wait_for_text("19 48"); // the tab bar takes a row, the sidebar 32 columns
}

#[test]
fn kitty_encoded_prefix_detaches_too() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo kitty-marker");
    paddock.wait_for_text("kitty-marker\n");
    let before = sandbox.shells();

    // paddock asks for kitty key disambiguation, so Ghostty sends C-a as CSI 97;5u.
    paddock.send(b"\x1b[97;5u");
    paddock.send(b"d");

    assert!(paddock.wait_exit());
    assert_eq!(sandbox.shells(), before);
    let again = sandbox.launch();
    again.wait_for_text("kitty-marker\n");
}

#[test]
fn terminals_know_paddocks_state_dir_for_hooks() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("echo \"state-$XDG_STATE_HOME\"");

    let state = sandbox.path("state").to_string_lossy().into_owned();
    let want = format!("state-{state}");
    paddock.wait_for("the variable", |s| s.lines().any(|l| l.trim() == want));
}
