//! Tickets 04 and 14: sidebar items (tmux sessions), their tabs (windows)
//! and prefix keys. Shell tabs are named after their directory: the test
//! shells start in the sandbox's `home`.

mod harness;

use harness::{tmux_only, Paddock, Sandbox, CURRENT_ITEM, PROMPT};

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

/// Tab names of space `space`, in order, as the backend has them.
fn tab_names(sandbox: &Sandbox, space: &str) -> Vec<String> {
    sandbox.space(space).tab_names()
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
    assert_eq!(sandbox.space_names(), ["space-1"]);
}

#[test]
fn c_opens_a_new_tab_in_the_current_space() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    new_tab(&mut paddock, &["1:home", "2:home"]);

    assert_eq!(sandbox.space_names(), ["space-1"]);
    assert_eq!(tab_names(&sandbox, "space-1"), ["home", "home"]);
    wait_for_spaces(&paddock, &["space-1"]);
    paddock.type_line("echo in-second-tab");
    paddock.wait_for_text("in-second-tab\n");
    let current = sandbox.space("space-1").current;
    assert_eq!(current, 1, "the backend has the same current tab");
}

#[test]
fn capital_c_opens_a_new_space_with_its_own_tabs() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);

    paddock.command(b"C\r");

    paddock.wait_for_tabs(&["1:home"]);
    wait_for_spaces(&paddock, &["space-1", "space-2"]);
    assert_eq!(sandbox.space_names(), ["space-1", "space-2"]);
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

    paddock.type_line("cat");
    // tmux names a tab when its terminal writes: if it looked before cat
    // had the terminal (a loaded machine), it looks again only at cat's
    // echo. Each key typed is one more look, until the name is there.
    let start = std::time::Instant::now();
    while paddock.tabs().0 != ["1:home", "2:cat"] && start.elapsed() < harness::TIMEOUT {
        paddock.send(b"x");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    paddock.wait_for_tabs(&["1:home", "2:cat"]);
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
    assert_eq!(tab_names(&sandbox, "space-1"), ["build logs"]);
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
    assert_eq!(sandbox.space_names(), ["api-server"]);
}

#[test]
fn a_tab_and_a_space_keep_a_hash_in_their_name() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]); // named by tmux by now

    // tmux expands formats in names: `v#S` would become `vspace-1`.
    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    paddock.send(b"\x15v#S\r");
    paddock.wait_for_tabs(&["1:v#S"]);
    assert_eq!(tab_names(&sandbox, "space-1"), ["v#S"]);

    paddock.command(b"$");
    paddock.wait_for_popup("Rename space", "space-1");
    paddock.send(b"\x15v#S\r");
    wait_for_spaces(&paddock, &["v#S"]);
    assert_eq!(sandbox.space_names(), ["v#S"]);
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
    assert_eq!(sandbox.space_names(), ["space-1", "space-2"]);
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
    assert_eq!(tab_names(&sandbox, "space-1"), ["home"]);
    // Esc right before a key would read as Alt with that key.
    std::thread::sleep(std::time::Duration::from_millis(200));
    paddock.type_line("echo ok-after-cancel");
    paddock.wait_for_text("ok-after-cancel\n");
    assert!(!paddock.screen().contains("zzz"));
}

#[test]
fn x_closes_the_tab_with_its_last_split() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);

    paddock.command(b"x");

    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_current_tab("1:home");
    assert_eq!(tab_names(&sandbox, "space-1"), ["home"]);
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
    assert_eq!(sandbox.space_names(), ["space-1"]);
    paddock.type_line("echo back-in-the-first");
    paddock.wait_for_text("back-in-the-first\n");
}

#[test]
fn closing_the_last_terminal_quits_paddock() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"x");

    assert!(paddock.wait_exit());
    assert!(sandbox.space_names().is_empty());
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

    // The whole help, not only its title: a frame may be read in parts.
    let lines = [
        "paddock keys",
        "new tab in this space",
        "new space",
        "next tab",
        "previous tab",
        "rename tab",
        "rename space",
        "C-a again sends C-a to the terminal",
    ];
    paddock.wait_for("the help with every line", |_| {
        let screen = paddock.full_screen();
        lines.iter().all(|line| screen.contains(line))
    });
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
fn reattaching_shows_every_space_and_tab_as_the_backend_has_them() {
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

/// tmux-only: an older paddock's server is a tmux server.
#[test]
fn tmux_only_sessions_of_an_older_paddock_open_as_one_space_each() {
    let sandbox = Sandbox::new();
    // As v1 left the server: one session per terminal, and none of the
    // options paddock added since.
    for name in ["terminal-1", "api"] {
        tmux_only::tmux(&sandbox, &["new-session", "-d", "-s", name, "-c", "/"]);
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
        paddock
            .full_screen()
            .contains("C-b again sends C-b to the terminal")
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
    assert!(sandbox.space_names().is_empty(), "no terminal was started");
}

/// Lines the stream prints before `stream-done`.
const STREAM_LINES: usize = 4000;

/// Start `seq`-like output in the focused terminal for about two seconds;
/// it makes `streamed` in the sandbox once done.
fn start_stream(sandbox: &Sandbox, paddock: &mut Paddock) {
    let done = sandbox.path("streamed");
    paddock.type_line(&format!(
        "for i in {{1..{STREAM_LINES}}}; do echo stream-$i; (( i % 100 )) || sleep 0.05; done; \
         echo stream-done; : > '{}'",
        done.display()
    ));
    paddock.wait_for("the stream started", |s| {
        s.lines().any(|l| l.starts_with("stream-"))
    });
}

/// Reattach paddock while the stream still runs.
fn reattach_while_streaming(sandbox: &Sandbox, mut paddock: Paddock) -> Paddock {
    paddock.detach_keys();
    assert!(paddock.wait_exit());
    let again = sandbox.launch();
    // A prompt, or the stream where no prompt shows until it ends.
    again.wait_for("a terminal drawn", |s| {
        s.contains(PROMPT) || s.lines().any(|l| l.starts_with("stream-"))
    });
    assert!(
        !sandbox.path("streamed").exists(),
        "precondition: still streaming after the attach"
    );
    again
}

#[test]
fn reattaching_while_a_background_tab_streams_loses_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, &["1:home", "2:home"]);
    paddock.command(b"1");
    paddock.wait_for_current_tab("1:home");
    start_stream(&sandbox, &mut paddock);
    paddock.command(b"2");
    paddock.wait_for_current_tab("2:home");

    let mut again = reattach_while_streaming(&sandbox, paddock);

    again.command(b"1");
    again.wait_for_current_tab("1:home");
    check_stream(&sandbox, &mut again, "space-1:0", 0);
}

#[test]
fn reattaching_a_split_tab_while_one_split_streams_loses_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"-");
    paddock.wait_for("two prompts", |s| s.matches(PROMPT).count() == 2);
    paddock.command(b"k"); // the top split
    start_stream(&sandbox, &mut paddock);

    let mut again = reattach_while_streaming(&sandbox, paddock);

    check_stream(&sandbox, &mut again, "space-1:0.0", 0);
}

/// tmux-only: what paddock draws after a reattach while streaming is what
/// tmux holds for the terminal, row for row.
#[test]
fn tmux_only_a_terminal_reattached_while_streaming_is_drawn_as_tmux_holds_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    start_stream(&sandbox, &mut paddock);

    let again = reattach_while_streaming(&sandbox, paddock);

    again.wait_for("stream-done and the prompt", |s| {
        let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
        lines
            .windows(2)
            .any(|w| w[0] == "stream-done" && w[1].starts_with(PROMPT))
    });
    let height = usize::from(sandbox.terminal("space-1").height);
    let held: Vec<String> = tmux_only::tmux(&sandbox, &["capture-pane", "-p", "-t", "space-1"])
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    let drawn: Vec<String> = again.rows().into_iter().skip(1).take(height).collect();
    let n = held.len().min(height);
    assert_eq!(drawn[..n], held[..n], "drawn terminal differs from tmux's");
}

/// Wait for the stream in terminal `target` (drawn from row 1, column
/// `col`) to end, then check what paddock drew ends with every last line
/// of it, and that its scrollback is as long as the backend's.
fn check_stream(sandbox: &Sandbox, paddock: &mut Paddock, target: &str, col: u16) {
    paddock.wait_for("stream-done and the prompt", |s| {
        let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
        lines
            .windows(2)
            .any(|w| w[0] == "stream-done" && w[1].starts_with(PROMPT))
    });
    let terminal = sandbox.terminal(target);
    let height = usize::from(terminal.height);
    let ours: Vec<String> = paddock
        .rows()
        .into_iter()
        .skip(1)
        .take(height)
        .map(|r| r.trim_end().to_string())
        .collect();
    // The last rows: the stream's tail, `stream-done`, then the prompt.
    let tail = (STREAM_LINES + 3 - height..=STREAM_LINES).map(|i| format!("stream-{i}"));
    let want: Vec<String> = tail.chain(["stream-done".to_string()]).collect();
    assert_eq!(ours[..height - 1], want, "drawn terminal");
    assert!(ours[height - 1].starts_with(PROMPT), "{ours:?}");
    // Lost output would show up as missing scrollback: compare its length.
    let scrollback = terminal.scrollback.expect("the terminal's scrollback");
    paddock.wheel(true, col + 2, 2);
    paddock.wait_for_text(&format!("[3/{scrollback}]"));
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
    for (key, name) in [(b"1", "a"), (b"2", "b"), (b"3", "c")] {
        paddock.command(key);
        paddock.command(b",");
        paddock.wait_for_popup("Rename tab", "home");
        paddock.send(format!("\x15{name}\r").as_bytes()); // C-u clears the old name
    }
    paddock.wait_for_tabs(&["1:a", "2:b", "3:c"]);

    paddock.command(b"<");
    paddock.wait_for_tabs(&["1:a", "2:c", "3:b"]);
    paddock.wait_for_current_tab("2:c");
    assert_eq!(tab_names(&sandbox, "space-1"), ["a", "c", "b"]);
    paddock.command(b"<");
    paddock.wait_for_tabs(&["1:c", "2:a", "3:b"]);
    paddock.command(b"<"); // already first: no wrap
    paddock.command(b">");
    paddock.wait_for_tabs(&["1:a", "2:c", "3:b"]);
    paddock.wait_for_current_tab("2:c");
    assert_eq!(tab_names(&sandbox, "space-1"), ["a", "c", "b"]);
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
    // Shown is space-1's (space-2 has one tab), and current there too.
    assert_eq!(sandbox.space("space-1").current, 1);
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
    // What is typed now reaches A1: its shell moves to a directory of its
    // own, and space-2's stays put.
    let a1 = sandbox.path("home/a1");
    std::fs::create_dir(&a1).unwrap();
    let a1 = std::fs::canonicalize(a1).unwrap();
    paddock.type_line(&format!("cd '{}'", a1.display()));
    let dir = |space: &str| sandbox.terminal(space).dir.map(std::path::PathBuf::from);
    paddock.wait_for("A1 in a1", |_| dir("space-1").as_ref() == Some(&a1));
    assert_ne!(dir("space-2").as_ref(), Some(&a1));
}
