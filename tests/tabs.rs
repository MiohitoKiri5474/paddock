//! Ticket 04: tabs and prefix keys.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};

/// A launched paddock with its first shell ready.
fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Open a new tab and wait for its shell.
fn new_tab(paddock: &mut Paddock, labels: &[&str]) {
    paddock.command(b"c");
    paddock.wait_for_tabs(labels);
    paddock.wait_for_current_tab(labels[labels.len() - 1]);
    paddock.wait_for_text(PROMPT);
}

#[test]
fn the_tab_bar_shows_the_first_terminal() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);

    paddock.wait_for_tabs(&["1:terminal-1"]);
    assert_eq!(paddock.current_tab(), "1:terminal-1");
}

#[test]
fn c_opens_a_new_shell_tab_on_the_hidden_server() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);

    assert_eq!(sandbox.session_names(), ["terminal-1", "terminal-2"]);
    paddock.type_line("echo in-second-tab");
    paddock.wait_for_text("in-second-tab\n");
}

#[test]
fn n_p_and_numbers_switch_tabs_and_each_keeps_its_screen() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo first-tab-marker");
    paddock.wait_for_text("first-tab-marker\n");
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);
    new_tab(
        &mut paddock,
        &["1:terminal-1", "2:terminal-2", "3:terminal-3"],
    );

    paddock.command(b"n");
    paddock.wait_for_current_tab("1:terminal-1");
    paddock.wait_for_text("first-tab-marker");

    paddock.command(b"p");
    paddock.wait_for_current_tab("3:terminal-3");
    assert!(!paddock.screen().contains("first-tab-marker"));

    paddock.command(b"2");
    paddock.wait_for_current_tab("2:terminal-2");

    paddock.command(b"9"); // no tab 9: nothing changes
    paddock.command(b"1");
    paddock.wait_for_current_tab("1:terminal-1");
}

#[test]
fn background_tabs_keep_receiving_output() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("sleep 1; echo done-while-hidden");
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);
    std::thread::sleep(std::time::Duration::from_millis(1500));

    paddock.command(b"1");

    paddock.wait_for_text("done-while-hidden\n");
}

#[test]
fn comma_renames_the_tab_and_the_name_survives_reattach() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b",");
    paddock.wait_for_text("rename tab: terminal-1");
    paddock.send(b"\x15build logs\r"); // C-u clears the old name

    paddock.wait_for_tabs(&["1:build logs"]);
    assert_eq!(sandbox.session_names(), ["build logs"]);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let again = ready(&sandbox);
    again.wait_for_tabs(&["1:build logs"]);
}

#[test]
fn renaming_can_be_cancelled_and_does_not_reach_the_shell() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b",");
    paddock.wait_for_text("rename tab:");
    paddock.send(b"zzz\x1b");

    paddock.wait_for_tabs(&["1:terminal-1"]);
    paddock.type_line("echo ok-after-cancel");
    paddock.wait_for_text("ok-after-cancel\n");
    assert!(!paddock.screen().contains("zzz"));
}

#[test]
fn x_closes_the_tab_and_ends_its_terminal() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);

    paddock.command(b"x");

    paddock.wait_for_tabs(&["1:terminal-1"]);
    paddock.wait_for_current_tab("1:terminal-1");
    assert_eq!(sandbox.session_names(), ["terminal-1"]);
}

#[test]
fn closing_the_last_tab_quits_paddock() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"x");

    assert!(paddock.wait_exit());
    assert!(sandbox.session_names().is_empty());
}

#[test]
fn a_shell_exiting_closes_only_its_tab() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);

    paddock.type_line("exit");

    paddock.wait_for_tabs(&["1:terminal-1"]);
    paddock.type_line("echo still-here");
    paddock.wait_for_text("still-here\n");
}

#[test]
fn question_mark_shows_key_help_and_any_key_closes_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"?");

    paddock.wait_for_text("paddock keys");
    for line in [
        "new shell tab",
        "next / previous tab",
        "rename tab",
        "send C-a to the terminal",
    ] {
        assert!(
            paddock.screen().contains(line),
            "help lacks {line:?}:\n{}",
            paddock.screen()
        );
    }
    paddock.send(b"q");
    paddock.wait_for("help closed", |s| !s.contains("paddock keys"));
    paddock.type_line("echo typed-after-help");
    paddock.wait_for_text("typed-after-help\n");
    assert!(
        !paddock.screen().contains("qecho"),
        "the closing key reached the shell"
    );
}

#[test]
fn prefix_twice_sends_a_literal_prefix() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("stty -echo; cat -v");
    std::thread::sleep(std::time::Duration::from_millis(300));

    paddock.send(b"\x01\x01\r");

    paddock.wait_for("literal C-a", |s| s.lines().any(|l| l == "^A"));
}

#[test]
fn reattaching_shows_every_terminal_as_a_tab() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);
    new_tab(
        &mut paddock,
        &["1:terminal-1", "2:terminal-2", "3:terminal-3"],
    );
    paddock.type_line("echo third-tab-marker");
    paddock.wait_for_text("third-tab-marker\n");
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let mut again = ready(&sandbox);
    again.wait_for_tabs(&["1:terminal-1", "2:terminal-2", "3:terminal-3"]);
    again.command(b"3");
    again.wait_for_text("third-tab-marker\n");
}

#[test]
fn the_prefix_comes_from_the_config_file() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys]\nprefix = \"C-b\"\n");
    let mut paddock = ready(&sandbox);

    paddock.send(b"\x02c"); // C-b c
    paddock.wait_for_tabs(&["1:terminal-1", "2:terminal-2"]);
    paddock.wait_for_text(PROMPT);

    // C-a is now ordinary input.
    paddock.type_line("stty -echo; cat -v");
    std::thread::sleep(std::time::Duration::from_millis(300));
    paddock.send(b"\x01\r");
    paddock.wait_for("C-a passed through", |s| s.lines().any(|l| l == "^A"));
    paddock.send(b"\x02?");
    paddock.wait_for_text("send C-b to the terminal");
}

#[test]
fn an_invalid_prefix_stops_paddock_with_a_clear_message() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys]\nprefix = \"Ctrl+A\"\n");
    let mut paddock = sandbox.launch();

    assert!(!paddock.wait_exit(), "paddock should fail");
    paddock.wait_for_text("invalid prefix key \"Ctrl+A\"");
    paddock.wait_for_text("config.toml");
    assert!(
        sandbox.session_names().is_empty(),
        "no terminal was started"
    );
}

#[test]
fn opening_a_tab_while_another_streams_output_loses_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    // About two seconds of output, well inside the history limit.
    paddock.type_line("for i in {1..4000}; do echo stream-$i; (( i % 100 )) || sleep 0.05; done; echo stream-done");
    paddock.wait_for_text("stream-1");

    // Attach the second terminal while the first is still printing.
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);
    let capture = || sandbox.tmux(&["capture-pane", "-p", "-t", "terminal-1"]);
    assert!(
        !capture().contains("stream-done"),
        "precondition: still streaming after the attach"
    );

    paddock.command(b"1");
    paddock.wait_for_current_tab("1:terminal-1");
    paddock.wait_for("stream-done and the prompt", |s| {
        let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
        lines
            .windows(2)
            .any(|w| w[0] == "stream-done" && w[1].starts_with(PROMPT))
    });

    let tmux_rows: Vec<String> = capture()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let ours: Vec<String> = paddock.rows().into_iter().skip(1).collect();
    assert_eq!(
        ours,
        tmux_rows[..ours.len()],
        "drawn terminal differs from tmux"
    );
    // Lost output would show up as missing history: compare history sizes.
    let tmux_history = sandbox.tmux(&[
        "display-message",
        "-p",
        "-t",
        "terminal-1",
        "#{history_size}",
    ]);
    paddock.wheel(true, 10, 10);
    let indicator = format!("[3/{}]", tmux_history.trim());
    paddock.wait_for_text(&indicator);
}

#[test]
fn renaming_to_a_taken_name_shows_an_error_and_keeps_the_old_name() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:terminal-1", "2:terminal-2"]);

    paddock.command(b",");
    paddock.wait_for_text("rename tab: terminal-2");
    paddock.send(b"\x15terminal-1\r");

    paddock.wait_for("error notice", |s| {
        s.lines().next().unwrap_or("").contains("duplicate session")
    });
    assert!(
        paddock.tab_bar().starts_with(" 1:terminal-1  2:terminal-2"),
        "{}",
        paddock.tab_bar()
    );
    assert_eq!(sandbox.session_names(), ["terminal-1", "terminal-2"]);
}
