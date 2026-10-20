//! Keys from `config.toml`: remapped actions work, their old keys do not,
//! the help shows them, and a bad `[keys.…]` table stops paddock.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};

/// A launched paddock with its first shell ready.
fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Windows on the hidden server, in every space.
fn window_count(sandbox: &Sandbox) -> usize {
    sandbox.tmux(&["list-windows", "-a"]).lines().count()
}

/// Run `echo <word>` in the focused shell and wait for its output: every
/// key sent before it has been handled.
fn echo(paddock: &mut Paddock, word: &str) {
    paddock.type_line(&format!("echo {word}"));
    paddock.wait_for_text(&format!("{word}\n"));
}

#[test]
fn a_remapped_command_works_and_its_old_key_does_not() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.commands]\nnew_tab = \"t\"\n");
    let mut paddock = ready(&sandbox);

    paddock.command(b"c");
    echo(&mut paddock, "after-c");
    assert_eq!(window_count(&sandbox), 1, "C-a c no longer opens a tab");
    paddock.wait_for_tabs(&["1:home"]);

    paddock.command(b"t");
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_current_tab("2:home");
}

#[test]
fn a_remapped_sidebar_key_works_and_its_old_key_does_not() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.sidebar]\nup = \"K\"\n");
    let mut paddock = ready(&sandbox);
    paddock.command(b"C");
    paddock.wait_for_text(PROMPT);
    echo(&mut paddock, "in-two");

    // `k` no longer moves: Enter opens space-2, the one already shown.
    paddock.command(b"w");
    paddock.wait_for("space-2 selected", |_| paddock.reversed(3, 5));
    paddock.send(b"k\r");
    echo(&mut paddock, "still-on-two");
    assert!(paddock.screen().contains("in-two"));

    // `K` moves up to space-1, and Enter shows it.
    paddock.command(b"w");
    paddock.wait_for("space-2 selected", |_| paddock.reversed(3, 5));
    paddock.send(b"K");
    paddock.wait_for("space-1 selected", |_| paddock.reversed(1, 5));
    paddock.send(b"\r");
    echo(&mut paddock, "on-one");
    assert!(!paddock.screen().contains("in-two"));
}

#[test]
fn the_help_shows_remapped_keys_and_not_unbound_ones() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.commands]\nnew_tab = \"t\"\ndetach = \"\"\n");
    let mut paddock = ready(&sandbox);

    paddock.command(b"?");
    paddock.wait_for("help", |_| paddock.full_screen().contains("paddock keys"));
    let help = paddock.full_screen();
    assert!(help.contains("t        new tab in this space"), "{help}");
    assert!(!help.contains("detach"), "{help}");
}

#[test]
fn a_key_bound_twice_stops_paddock_with_a_clear_message() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.commands]\nnew_tab = \"n\"\n");
    let mut paddock = sandbox.launch();

    assert!(!paddock.wait_exit(), "paddock should fail");
    paddock.wait_for_text("config.toml");
    paddock.wait_for_text("[keys.commands] \"n\" is bound to both new_tab and next_tab");
    assert!(
        sandbox.session_names().is_empty(),
        "no terminal was started"
    );
}

#[test]
fn paddock_keys_prints_the_defaults_as_toml() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["keys"], &[], "");
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        "[keys]\nprefix = \"C-a\"",
        "[keys.commands]\nnew_tab = \"c\"",
        "focus_left = [\"h\", \"Left\"]",
        "[keys.sidebar]",
        "[keys.dashboard]",
    ] {
        assert!(text.contains(line), "lacks {line:?}:\n{text}");
    }
    assert!(sandbox.sockets().is_empty(), "no tmux server was started");
}
