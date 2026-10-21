//! Tickets 04 and 14: sidebar items (tmux sessions), their tabs (windows)
//! and prefix keys. Shell tabs are named after their directory: the test
//! shells start in the sandbox's `home`.

mod harness;

use harness::{Paddock, Sandbox, CURRENT_ITEM, PROMPT};

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

/// Window names of tmux session `session`, in order.
fn windows(sandbox: &Sandbox, session: &str) -> Vec<String> {
    sandbox
        .tmux(&["list-windows", "-t", session, "-F", "#{window_name}"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Wait until the sidebar lists exactly these spaces, all in the test
/// shells' home (`~`, no git repository), without agents.
fn wait_for_spaces(paddock: &Paddock, names: &[&str]) {
    let at_home: Vec<(&str, &str)> = names.iter().map(|n| (*n, "~")).collect();
    wait_for_spaces_at(paddock, &at_home);
}

/// Wait until the sidebar lists exactly these spaces, as `(name, place)`,
/// without agents.
fn wait_for_spaces_at(paddock: &Paddock, spaces: &[(&str, &str)]) {
    let mut want = vec![" SPACES".to_string()];
    for (name, place) in spaces {
        want.extend([format!("   {name}"), format!("   {place}")]);
    }
    want.push(" AGENTS".to_string());
    let want: Vec<&str> = want.iter().map(String::as_str).collect();
    wait_for_sidebar(paddock, &want);
}

/// Wait until the sidebar shows exactly these lines (blank ones dropped).
fn wait_for_sidebar(paddock: &Paddock, want: &[&str]) {
    let lines = || -> Vec<String> {
        paddock
            .sidebar()
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect()
    };
    let start = std::time::Instant::now();
    while start.elapsed() < harness::TIMEOUT {
        if lines() == want {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!(
        "sidebar is {:?}, not {want:?}; screen:\n{}",
        lines(),
        paddock.full_screen()
    );
}

#[test]
fn the_sidebar_shows_the_first_space_and_the_tab_bar_its_tab() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);

    paddock.wait_for_tabs(&["1:home"]);
    assert_eq!(paddock.current_tab(), "1:home");
    wait_for_spaces(&paddock, &["space-1"]);
    assert_eq!(sandbox.session_names(), ["space-1"]);
}

#[test]
fn c_opens_a_new_tab_as_a_window_of_the_current_space() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    new_tab(&mut paddock, &["1:home", "2:home"]);

    assert_eq!(sandbox.session_names(), ["space-1"]);
    assert_eq!(windows(&sandbox, "space-1"), ["home", "home"]);
    wait_for_spaces(&paddock, &["space-1"]);
    paddock.type_line("echo in-second-tab");
    paddock.wait_for_text("in-second-tab\n");
    let active = sandbox.display("space-1", "#{window_index}");
    assert_eq!(active, "1", "tmux shows the same current tab");
}

#[test]
fn capital_c_opens_a_new_space_with_its_own_tabs() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);

    paddock.command(b"C\r");

    paddock.wait_for_tabs(&["1:home"]);
    wait_for_spaces(&paddock, &["space-1", "space-2"]);
    assert_eq!(sandbox.session_names(), ["space-1", "space-2"]);
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo in-the-new-item");
    paddock.wait_for_text("in-the-new-item\n");
}

#[test]
fn picking_a_space_shows_its_tabs() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo first-item-marker");
    paddock.wait_for_text("first-item-marker\n");
    new_tab(&mut paddock, &["1:home", "2:home"]);
    paddock.command(b"C\r");
    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_text(PROMPT);

    // The sidebar: k moves up to space-1, Enter shows its tabs.
    paddock.command(b"w");
    paddock.send(b"k\r");
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_current_tab("2:home");
    paddock.command(b"1");
    paddock.wait_for_text("first-item-marker\n");

    // A click on the second space (screen row 3) goes back.
    paddock.click_screen(3, 3);
    paddock.wait_for_tabs(&["1:home"]);
    paddock.type_line("echo keys-reach-the-item");
    paddock.wait_for_text("keys-reach-the-item\n");
}

#[test]
fn the_current_space_has_its_own_background() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
    wait_for_spaces(&paddock, &["space-1", "space-2"]);

    assert_eq!(paddock.colors(3, 5).1, CURRENT_ITEM, "current: space-2");
    assert_eq!(
        paddock.colors(4, 20).1,
        CURRENT_ITEM,
        "across both its lines"
    );
    assert_eq!(paddock.colors(1, 5).1, vt100::Color::Default);
}

#[test]
fn the_current_space_background_comes_from_the_config() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[colors]\ncurrent_space = \"#102030\"\n");
    let paddock = ready(&sandbox);

    paddock.wait_for("current item colour", |_| {
        paddock.colors(1, 5).1 == vt100::Color::Rgb(0x10, 0x20, 0x30)
    });
}

#[test]
fn n_p_and_numbers_switch_tabs_and_each_keeps_its_screen() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo first-tab-marker");
    paddock.wait_for_text("first-tab-marker\n");
    new_tab(&mut paddock, &["1:home", "2:home"]);
    new_tab(&mut paddock, &["1:home", "2:home", "3:home"]);

    paddock.command(b"n");
    paddock.wait_for_current_tab("1:home");
    paddock.wait_for_text("first-tab-marker");

    paddock.command(b"p");
    paddock.wait_for_current_tab("3:home");
    assert!(!paddock.screen().contains("first-tab-marker"));

    paddock.command(b"2");
    paddock.wait_for_current_tab("2:home");

    paddock.command(b"9"); // no tab 9: nothing changes
    paddock.command(b"1");
    paddock.wait_for_current_tab("1:home");
}

#[test]
fn tabs_are_named_after_what_runs_in_them() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);

    paddock.type_line("sleep 30");
    paddock.wait_for_tabs(&["1:home", "2:sleep"]);
    paddock.send(b"\x03");
    paddock.type_line("cd /");
    paddock.wait_for_tabs(&["1:home", "2:/"]);
}

#[test]
fn background_tabs_keep_receiving_output() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("sleep 1; echo done-while-hidden");
    new_tab(&mut paddock, &["1:home", "2:home"]);
    std::thread::sleep(std::time::Duration::from_millis(1500));

    paddock.command(b"1");

    paddock.wait_for_text("done-while-hidden\n");
}

#[test]
fn comma_renames_the_tab_and_the_name_survives_reattach() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]); // named by tmux by now

    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    paddock.send(b"\x15build logs\r"); // C-u clears the old name

    paddock.wait_for_tabs(&["1:build logs"]);
    assert_eq!(windows(&sandbox, "space-1"), ["build logs"]);
    // A renamed tab keeps its name whatever runs in it.
    paddock.type_line("cd /");
    std::thread::sleep(std::time::Duration::from_millis(500));
    paddock.wait_for_tabs(&["1:build logs"]);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let again = ready(&sandbox);
    again.wait_for_tabs(&["1:build logs"]);
}

#[test]
fn dollar_renames_the_space() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"$");
    paddock.wait_for_popup("Rename space", "space-1");
    paddock.send(b"\x15api-server\r");

    wait_for_spaces(&paddock, &["api-server"]);
    assert_eq!(sandbox.session_names(), ["api-server"]);
}

#[test]
fn renaming_a_space_to_a_taken_name_shows_an_error_and_keeps_the_old_name() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
    paddock.wait_for_text(PROMPT);

    paddock.command(b"$");
    paddock.wait_for_popup("Rename space", "space-2");
    paddock.send(b"\x15space-1\r");

    // The popup closes; the notice shows in the tab bar.
    paddock.wait_for_no_popup();
    paddock.wait_for("error notice", |s| {
        s.lines().next().unwrap_or("").contains("duplicate session")
    });
    assert_eq!(sandbox.session_names(), ["space-1", "space-2"]);
}

#[test]
fn renaming_can_be_cancelled_and_does_not_reach_the_shell() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]); // named by tmux by now

    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    paddock.send(b"zzz");
    paddock.wait_for_popup("Rename tab", "homezzz");
    paddock.send(b"\x1b");

    paddock.wait_for_no_popup();
    paddock.wait_for_tabs(&["1:home"]);
    assert_eq!(windows(&sandbox, "space-1"), ["home"]);
    // Esc right before a key would read as Alt with that key.
    std::thread::sleep(std::time::Duration::from_millis(200));
    paddock.type_line("echo ok-after-cancel");
    paddock.wait_for_text("ok-after-cancel\n");
    assert!(!paddock.screen().contains("zzz"));
}

#[test]
fn x_closes_the_tab_with_its_last_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);

    paddock.command(b"x");

    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_current_tab("1:home");
    assert_eq!(windows(&sandbox, "space-1"), ["home"]);
}

#[test]
fn closing_a_spaces_last_tab_closes_the_space() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
    paddock.wait_for_text(PROMPT);
    wait_for_spaces(&paddock, &["space-1", "space-2"]);

    paddock.command(b"x");

    wait_for_spaces(&paddock, &["space-1"]);
    assert_eq!(sandbox.session_names(), ["space-1"]);
    paddock.type_line("echo back-in-the-first");
    paddock.wait_for_text("back-in-the-first\n");
}

#[test]
fn closing_the_last_pane_quits_paddock() {
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
    new_tab(&mut paddock, &["1:home", "2:home"]);

    paddock.type_line("exit");

    paddock.wait_for_tabs(&["1:home"]);
    paddock.type_line("echo still-here");
    paddock.wait_for_text("still-here\n");
}

#[test]
fn question_mark_shows_key_help_and_any_key_closes_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"?");

    paddock.wait_for("help", |_| paddock.full_screen().contains("paddock keys"));
    for line in [
        "new tab in this space",
        "new space",
        "next tab",
        "previous tab",
        "rename tab",
        "rename space",
        "send C-a to the terminal",
    ] {
        assert!(
            paddock.full_screen().contains(line),
            "help lacks {line:?}:\n{}",
            paddock.full_screen()
        );
    }
    paddock.send(b"q");
    paddock.wait_for("help closed", |_| {
        !paddock.full_screen().contains("paddock keys")
    });
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
fn reattaching_shows_every_space_and_tab_as_tmux_has_them() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);
    new_tab(&mut paddock, &["1:home", "2:home", "3:home"]);
    paddock.type_line("echo third-tab-marker");
    paddock.wait_for_text("third-tab-marker\n");
    paddock.command(b"C\r");
    paddock.wait_for_tabs(&["1:home"]);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let mut again = ready(&sandbox);
    // The item that was current comes back current.
    again.wait_for_tabs(&["1:home"]);
    again.command(b"w");
    again.send(b"k\r");
    again.wait_for_tabs(&["1:home", "2:home", "3:home"]);
    again.wait_for_current_tab("3:home");
    again.wait_for_text("third-tab-marker\n");
}

#[test]
fn sessions_of_an_older_paddock_open_as_one_space_each() {
    let sandbox = Sandbox::new();
    // As v1 left the server: one session per terminal, and none of the
    // options paddock added since.
    for name in ["terminal-1", "api"] {
        sandbox.tmux(&["new-session", "-d", "-s", name, "-c", "/"]);
    }
    std::fs::create_dir_all(sandbox.path("state/paddock")).unwrap();
    std::fs::write(
        sandbox.path("state/paddock/layout.toml"),
        "current = 1\n[[tabs]]\nfocus = \"$0\"\nroot = { leaf = \"$0\" }\n",
    )
    .unwrap();

    let paddock = sandbox.launch();

    wait_for_spaces_at(&paddock, &[("terminal-1", "/"), ("api", "/")]);
    paddock.wait_for_tabs(&["1:/"]);
    assert!(!paddock.full_screen().contains("invalid layout"));
}

#[test]
fn the_prefix_comes_from_the_config_file() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys]\nprefix = \"C-b\"\n");
    let mut paddock = ready(&sandbox);

    paddock.send(b"\x02c"); // C-b c
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_text(PROMPT);

    // C-a is now ordinary input.
    paddock.type_line("stty -echo; cat -v");
    std::thread::sleep(std::time::Duration::from_millis(300));
    paddock.send(b"\x01\r");
    paddock.wait_for("C-a passed through", |s| s.lines().any(|l| l == "^A"));
    paddock.send(b"\x02?");
    paddock.wait_for("help", |_| {
        paddock.full_screen().contains("send C-b to the terminal")
    });
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

/// Run `seq`-like output in pane `target` for about two seconds, then
/// reattach paddock while it still streams, and check nothing was lost.
fn reattach_while_streaming(sandbox: &Sandbox, paddock: Paddock, target: &str) -> Paddock {
    sandbox.tmux(&[
        "send-keys",
        "-t",
        target,
        "for i in {1..4000}; do echo stream-$i; (( i % 100 )) || sleep 0.05; done; echo stream-done",
        "Enter",
    ]);
    let capture = || sandbox.tmux(&["capture-pane", "-p", "-t", target]);
    let start = std::time::Instant::now();
    while !capture().contains("stream-1") {
        assert!(start.elapsed() < harness::TIMEOUT, "never started");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut paddock = paddock;
    paddock.detach_keys();
    assert!(paddock.wait_exit());
    let again = ready(sandbox);
    assert!(
        !capture().contains("stream-done"),
        "precondition: still streaming after the attach"
    );
    again
}

#[test]
fn reattaching_while_a_background_tab_streams_loses_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);
    paddock.command(b"2");
    paddock.wait_for_current_tab("2:home");

    let mut again = reattach_while_streaming(&sandbox, paddock, "space-1:0");

    again.command(b"1");
    again.wait_for_current_tab("1:home");
    check_stream(&sandbox, &mut again, "space-1:0", 0);
}

#[test]
fn reattaching_a_split_tab_while_one_pane_streams_loses_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"-");
    paddock.wait_for("two prompts", |s| s.matches(PROMPT).count() == 2);
    let top = sandbox.tmux(&["display-message", "-p", "-t", "space-1:0.0", "#{pane_id}"]);

    let mut again = reattach_while_streaming(&sandbox, paddock, top.trim());

    check_stream(&sandbox, &mut again, top.trim(), 0);
}

/// Wait for the stream in pane `target` (drawn from row 1, column `col`) to
/// end, then compare what paddock drew and its history with tmux's.
fn check_stream(sandbox: &Sandbox, paddock: &mut Paddock, target: &str, col: u16) {
    paddock.wait_for("stream-done and the prompt", |s| {
        let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
        lines
            .windows(2)
            .any(|w| w[0] == "stream-done" && w[1].starts_with(PROMPT))
    });
    let height: usize = sandbox
        .display(target, "#{pane_height}")
        .parse()
        .expect("height");
    let tmux_rows: Vec<String> = sandbox
        .tmux(&["capture-pane", "-p", "-t", target])
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let ours: Vec<String> = paddock
        .rows()
        .into_iter()
        .skip(1)
        .take(height)
        .map(|r| r.trim_end().to_string())
        .collect();
    assert_eq!(
        ours[..tmux_rows.len().min(height)],
        tmux_rows[..tmux_rows.len().min(height)],
        "drawn pane differs from tmux"
    );
    // Lost output would show up as missing history: compare history sizes.
    let history = sandbox.display(target, "#{history_size}");
    paddock.wheel(true, col + 2, 2);
    paddock.wait_for_text(&format!("[3/{history}]"));
}

#[test]
fn a_space_shows_its_directory_and_git_branch() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path("home/project/api");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q", "-b", "dev"]);
    let mut paddock = ready(&sandbox);
    wait_for_spaces(&paddock, &["space-1"]);

    paddock.type_line("cd project/api");

    // Read on the git thread's next round, every few seconds.
    wait_for_spaces_at(&paddock, &[("space-1", "~/project/api  dev")]);
}

#[test]
fn c_a_angle_brackets_move_the_tab_left_and_right() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);
    new_tab(&mut paddock, &["1:home", "2:home", "3:home"]);
    for (i, name) in ["a", "b", "c"].iter().enumerate() {
        sandbox.tmux(&["rename-window", "-t", &format!("space-1:{i}"), name]);
    }
    paddock.wait_for_tabs(&["1:a", "2:b", "3:c"]);

    paddock.command(b"<");
    paddock.wait_for_tabs(&["1:a", "2:c", "3:b"]);
    paddock.wait_for_current_tab("2:c");
    assert_eq!(windows(&sandbox, "space-1"), ["a", "c", "b"]);
    paddock.command(b"<");
    paddock.wait_for_tabs(&["1:c", "2:a", "3:b"]);
    paddock.command(b"<"); // already first: no wrap
    paddock.command(b">");
    paddock.wait_for_tabs(&["1:a", "2:c", "3:b"]);
    paddock.wait_for_current_tab("2:c");
    assert_eq!(windows(&sandbox, "space-1"), ["a", "c", "b"]);
}

#[test]
fn c_a_tab_goes_back_to_the_last_page_across_spaces() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"\t");
    paddock.wait_for_text("no last page");
    paddock.type_line("echo one-$((1))");
    paddock.wait_for_text("one-1\n");
    new_tab(&mut paddock, &["1:home", "2:home"]);
    paddock.command(b"C\r");
    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_text(PROMPT);

    paddock.command(b"\t");
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_current_tab("2:home");
    paddock.command(b"\t");
    paddock.wait_for_tabs(&["1:home"]);
    // And back once more, to space-1's second tab.
    paddock.command(b"\t");
    paddock.wait_for_current_tab("2:home");
    paddock.type_line("echo where-$((2))");
    paddock.wait_for_text("where-2\n");
    let capture = sandbox.tmux(&["capture-pane", "-p", "-t", "space-1:1"]);
    assert!(capture.contains("where-2"), "{capture}");
}

#[test]
fn c_a_tab_skips_a_closed_page_for_an_older_one() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox); // A1
    paddock.command(b"C\r"); // space-2: B1
    paddock.wait_for_text(PROMPT);
    new_tab(&mut paddock, &["1:home", "2:home"]); // B2
    paddock.command(b"1"); // back on B1
    paddock.wait_for_current_tab("1:home");

    // B1 closes; space-2 shows B2, the page before B1 is A1.
    paddock.command(b"x");
    paddock.wait_for_tabs(&["1:home"]);
    paddock.command(b"\t");

    // The current space's background is on space-1 again.
    paddock.wait_for("space-1 current", |_| {
        paddock.colors(1, 5).1 == CURRENT_ITEM
    });
    paddock.type_line("echo back-in-a1");
    paddock.wait_for_text("back-in-a1\n");
    let capture = sandbox.tmux(&["capture-pane", "-p", "-t", "space-1:0"]);
    assert!(capture.contains("back-in-a1"), "{capture}");
}
