//! Keys from `config.toml`: remapped actions work, their old keys do not,
//! the help shows them, and a bad `[keys.…]` table stops paddock. Direct
//! chords (ticket 22) are commands without the prefix.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::time::Duration;

/// The notice an unset `[keys.direct]` gives at start.
const CONTROL_CHARS_NOTICE: &str = "[keys.direct] takes C-h and C-j from the terminal";

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

/// Panes on the hidden server, in every space.
fn pane_count(sandbox: &Sandbox) -> usize {
    sandbox.tmux(&["list-panes", "-a"]).lines().count()
}

/// Wait until space-1's current tab has `n` panes.
fn wait_for_panes(paddock: &Paddock, sandbox: &Sandbox, n: usize) {
    paddock.wait_for(&format!("{n} panes"), |_| pane_count(sandbox) == n);
}

/// Wait until the pane at `index` of space-1's current tab has the focus.
fn wait_for_focus(paddock: &Paddock, sandbox: &Sandbox, index: &str) {
    paddock.wait_for(&format!("pane {index} focused"), |_| {
        sandbox.display("space-1", "#{pane_index}") == index
    });
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
    paddock.command(b"C\r");
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
    assert!(help.contains("t      new tab in this space"), "{help}");
    assert!(!help.contains("detach"), "{help}");
}

#[test]
fn an_alt_key_warning_is_printed_again_on_leaving() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.commands]\nnew_tab = \"M-t\"\n");
    let mut paddock = ready(&sandbox);

    paddock.detach_keys();
    assert!(paddock.wait_exit(), "paddock should detach cleanly");
    paddock.wait_for_text("paddock: [keys.commands] new_tab: \"M-t\" uses Alt");
}

#[test]
fn ctrl_does_not_matter_for_sidebar_keys() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
    paddock.wait_for_text(PROMPT);

    paddock.command(b"w");
    paddock.wait_for("space-2 selected", |_| paddock.reversed(3, 5));
    paddock.send(b"\x0b"); // C-k
    paddock.wait_for("space-1 selected", |_| paddock.reversed(1, 5));
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
        "[keys.direct]\nnew_tab = \"C-M-c\"",
        "\nfocus_left = \"C-h\"",
        "\nsplit_right = 'C-\\'",
        "\nsplit_down = \"C--\"",
        "[keys.sidebar]",
        "[keys.dashboard]",
    ] {
        assert!(text.contains(line), "lacks {line:?}:\n{text}");
    }
    assert!(sandbox.sockets().is_empty(), "no tmux server was started");
}

#[test]
fn direct_chords_work_without_the_prefix() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    // `C-\` splits side by side and the new pane gets the focus; `C-h`
    // and `C-l` move it between the two.
    paddock.send(b"\x1c");
    wait_for_panes(&paddock, &sandbox, 2);
    wait_for_focus(&paddock, &sandbox, "1");
    paddock.send(b"\x08");
    wait_for_focus(&paddock, &sandbox, "0");
    paddock.send(b"\x0c");
    wait_for_focus(&paddock, &sandbox, "1");

    // `C--` splits the right pane top and bottom; `C-k` and `C-j` move.
    paddock.send(b"\x1f");
    wait_for_panes(&paddock, &sandbox, 3);
    wait_for_focus(&paddock, &sandbox, "2");
    paddock.send(b"\x0b");
    wait_for_focus(&paddock, &sandbox, "1");
    paddock.send(b"\x0a");
    wait_for_focus(&paddock, &sandbox, "2");

    // `C-M-c` opens a tab; `C-M-]` and `C-M-[` cycle. A terminal without
    // the kitty protocol sends `C-M-]` as Esc and 0x1d; `C-M-[` reaches
    // paddock only under that protocol (else it is two Esc), so it is
    // sent as the protocol does.
    paddock.send(b"\x1b\x03");
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_current_tab("2:home");
    paddock.send(b"\x1b\x1d");
    paddock.wait_for_current_tab("1:home");
    paddock.send(b"\x1b[91;7u");
    paddock.wait_for_current_tab("2:home");

    // Under the kitty protocol `C--` may come as Ctrl+Shift+_: one key.
    paddock.send(b"\x1b[95;6u");
    wait_for_panes(&paddock, &sandbox, 5);
    paddock.send(b"\x1b[45;5u");
    wait_for_panes(&paddock, &sandbox, 6);
}

#[test]
fn an_unbound_direct_chord_reaches_the_terminal_again() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.direct]\nfocus_left = \"\"\n");
    let mut paddock = ready(&sandbox);

    // `cat -v` shows each byte as it comes once the tty stops editing lines.
    paddock.type_line("stty -icanon; cat -v");
    paddock.send(b"\x08");
    paddock.wait_for_text("^H");
    // `C-l` is still a chord: paddock keeps it, even with nowhere to go.
    paddock.send(b"\x0c");
    paddock.send(b"\x08");
    paddock.wait_for_text("^H^H");
    assert!(!paddock.screen().contains("^L"), "{}", paddock.screen());
}

#[test]
fn in_a_float_a_direct_chord_goes_to_the_float() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[floats]\ng = \"cat -v\"\n");
    let mut paddock = ready(&sandbox);

    paddock.command(b"g");
    paddock.wait_for("the float shown", |_| {
        paddock.full_screen().contains(" cat -v ")
    });
    // (`C-\` would quit `cat`: the tty reads 0x1c as SIGQUIT.)
    paddock.send(b"\x0c\r");
    paddock.wait_for("C-l in the float", |_| paddock.full_screen().contains("^L"));
    paddock.send(b"\x08\r");
    paddock.wait_for("C-h in the float", |_| paddock.full_screen().contains("^H"));
    // The float and the shell: no chord moved focus or split anything.
    assert_eq!(pane_count(&sandbox), 2);
}

#[test]
fn the_control_character_notice_shows_at_start_until_direct_is_set() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_text(CONTROL_CHARS_NOTICE);
    paddock.detach_keys();
    assert!(paddock.wait_exit(), "paddock should detach cleanly");
    paddock.wait_for_text(&format!("paddock: {CONTROL_CHARS_NOTICE}"));

    let sandbox = Sandbox::new();
    sandbox.write_config("[keys.direct]\n");
    let mut paddock = ready(&sandbox);
    assert!(
        !paddock.screen().contains("[keys.direct]"),
        "{}",
        paddock.screen()
    );
    paddock.detach_keys();
    assert!(paddock.wait_exit(), "paddock should detach cleanly");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !paddock.full_screen().contains("[keys.direct]"),
        "{}",
        paddock.full_screen()
    );
}
