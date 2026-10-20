//! Ticket 03: one terminal, drawn by paddock, on the hidden tmux server.

mod harness;

use harness::{Sandbox, PROMPT};

/// The session's single pane, as `pane_id pane_pid pane_dead`.
fn pane(sandbox: &Sandbox) -> String {
    sandbox
        .tmux(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_id} #{pane_pid} #{pane_dead}",
        ])
        .trim()
        .to_string()
}

#[test]
fn launching_shows_a_working_shell() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("echo $((6 * 7))");

    paddock.wait_for("command output", |s| s.lines().any(|l| l.trim() == "42"));
    let sessions = sandbox.tmux(&["list-sessions", "-F", "#{session_name}"]);
    assert_eq!(sessions.trim(), "workspace-1");
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
fn hidden_server_has_a_clean_environment_and_its_own_config() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    let global_env = sandbox.tmux(&["show-environment", "-g"]);
    assert!(!global_env.contains("SUPACODE_"), "{global_env}");
    assert!(!global_env.contains("TERM_PROGRAM"), "{global_env}");

    // tmux sets its own TERM_PROGRAM=tmux in panes; the launching app's value must not leak.
    paddock.type_line("env | grep -c -E 'SUPACODE_|leaky-terminal'; echo env-checked");
    paddock.wait_for_text("env-checked");
    // tmux sets its own TERM_PROGRAM in every pane; pin exactly what shells see.
    paddock.type_line("echo tp=$TERM_PROGRAM tmux-var=${TMUX%%,*}");
    paddock.wait_for_text("tp=tmux tmux-var=");
    let tmux_var = paddock
        .rows()
        .into_iter()
        .find(|r| r.starts_with("tp="))
        .unwrap_or_default();
    assert!(
        !tmux_var.contains("/tmp/nope"),
        "outer TMUX leaked: {tmux_var}"
    );
    assert!(
        paddock.rows().iter().any(|l| l == "0"),
        "leaked variables:\n{}",
        paddock.screen()
    );

    let options = sandbox.tmux(&["show-options", "-g"]);
    for expected in ["status off", "prefix None"] {
        assert!(
            options.lines().any(|l| l == expected),
            "missing {expected:?}:\n{options}"
        );
    }
    let server_options = sandbox.tmux(&["show-options", "-s"]);
    assert!(
        server_options.lines().any(|l| l == "escape-time 0"),
        "{server_options}"
    );
    assert!(
        server_options.lines().any(|l| l == "extended-keys on"),
        "{server_options}"
    );
}

#[test]
fn only_the_paddock_socket_is_used() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    // Both paddock and its shell ran with this TMUX_TMPDIR: no "default" socket appeared.
    assert_eq!(sandbox.sockets(), vec!["paddock-test".to_string()]);
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
    // tmux itself did not scroll: paddock keeps its own scrollback.
    assert_eq!(
        sandbox
            .tmux(&[
                "display-message",
                "-p",
                "-t",
                "workspace-1",
                "#{pane_in_mode}"
            ])
            .trim(),
        "0"
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
    let before = pane(&sandbox);

    paddock.detach_keys();

    assert!(paddock.wait_exit(), "paddock should exit cleanly on C-a d");
    assert_eq!(pane(&sandbox), before, "the same shell is still running");
    assert!(before.ends_with(" 0"), "pane is alive: {before}");

    let mut again = sandbox.launch();
    again.wait_for("restored screen", |s| s.lines().any(|l| l.trim() == "300"));
    assert_eq!(
        sandbox
            .tmux(&["list-sessions", "-F", "#{session_name}"])
            .trim(),
        "workspace-1"
    );

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
fn config_and_state_follow_the_xdg_overrides() {
    let sandbox = Sandbox::new();
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    let conf = sandbox.path("state/paddock/tmux.conf");
    let text = std::fs::read_to_string(&conf).expect("generated tmux.conf in XDG_STATE_HOME");
    assert!(text.contains("set -s escape-time 0"), "{text}");
    assert!(
        !sandbox.path("home/.local").exists(),
        "nothing written under HOME"
    );
}

#[test]
fn exiting_the_shell_quits_paddock_and_the_next_launch_starts_a_new_one() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("exit");

    assert!(paddock.wait_exit());
    assert_eq!(sandbox.tmux(&["list-sessions"]).trim(), "");
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
    let size = || {
        sandbox.tmux(&[
            "display-message",
            "-p",
            "-t",
            "workspace-1",
            "#{pane_width}x#{pane_height}",
        ])
    };
    let start = std::time::Instant::now();
    while size().trim() != "56x19" {
        assert!(start.elapsed() < harness::TIMEOUT, "pane is {}", size());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    paddock.type_line("stty size");

    paddock.wait_for_text("19 56"); // the tab bar takes a row, the sidebar 24 columns
}

#[test]
fn kitty_encoded_prefix_detaches_too() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo kitty-marker");
    paddock.wait_for_text("kitty-marker\n");
    let before = pane(&sandbox);

    // paddock asks for kitty key disambiguation, so Ghostty sends C-a as CSI 97;5u.
    paddock.send(b"\x1b[97;5u");
    paddock.send(b"d");

    assert!(paddock.wait_exit());
    assert_eq!(pane(&sandbox), before);
    let again = sandbox.launch();
    again.wait_for_text("kitty-marker\n");
}

#[test]
fn terminals_know_paddocks_socket_and_state_dir_for_hooks() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.type_line("echo \"pane-$PADDOCK_TMUX_SOCKET\" \"$XDG_STATE_HOME\"");

    let state = sandbox.path("state").to_string_lossy().into_owned();
    let want = format!("pane-paddock-test {state}");
    paddock.wait_for("the variables", |s| s.lines().any(|l| l.trim() == want));
}
