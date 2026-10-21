//! Ticket 19: floats, a program in a box over the screen that the prefix
//! and its key show and hide, kept running while hidden. The tests run
//! `cat`, which echoes what it is sent and exits on `C-d`, instead of
//! lazygit.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};

/// The float's title, as drawn in its top border.
const TITLE: &str = " cat ";
/// `[floats]` for these tests: `C-a g` runs `cat` instead of lazygit.
const CONFIG: &str = "[floats]\ng = \"cat\"\n";
/// The box's top-left corner on the 100x30 test screen: 80% of each side,
/// centred, over the sidebar.
const CORNER: (usize, usize) = (10, 3);
/// The float's pane: the box less its border.
const INSIDE: &str = "78x22";

fn ready(sandbox: &Sandbox) -> Paddock {
    sandbox.write_config(CONFIG);
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Float panes on the server, as `(space name, pane id, key)`.
fn floats(sandbox: &Sandbox) -> Vec<(String, String, String)> {
    let format = "#{session_name} #{pane_id} #{@paddock-float}";
    sandbox
        .tmux(&["list-panes", "-a", "-F", format])
        .lines()
        .filter_map(|l| {
            let mut f = l.split(' ');
            let (space, pane, key) = (f.next()?, f.next()?, f.next()?);
            (!key.is_empty()).then(|| (space.into(), pane.into(), key.into()))
        })
        .collect()
}

/// The panes of space `space` that are not floats.
fn tab_panes(sandbox: &Sandbox, space: &str) -> Vec<String> {
    let format = "#{pane_id} #{@paddock-float}";
    sandbox
        .tmux(&["list-panes", "-s", "-t", space, "-F", format])
        .lines()
        .filter(|l| l.trim_end().split(' ').nth(1).is_none_or(str::is_empty))
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// The one float pane on the server, once it runs.
fn wait_for_float(paddock: &Paddock, sandbox: &Sandbox) -> String {
    paddock.wait_for("a float on the server", |_| floats(sandbox).len() == 1);
    floats(sandbox).remove(0).1
}

/// Wait until `text` is drawn anywhere, sidebar included: the float's
/// box starts left of the session area.
fn wait_for_full_text(paddock: &Paddock, text: &str) {
    paddock.wait_for(text, |_| paddock.full_screen().contains(text));
}

fn wait_shown(paddock: &Paddock) {
    paddock.wait_for("the float shown", |_| paddock.full_screen().contains(TITLE));
}

fn wait_hidden(paddock: &Paddock) {
    paddock.wait_for("the float hidden", |_| {
        !paddock.full_screen().contains(TITLE)
    });
}

#[test]
fn the_key_shows_a_float_over_everything_and_hides_it_with_its_program_kept() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let work = sandbox.path("work");
    std::fs::create_dir(&work).unwrap();
    paddock.type_line(&format!("cd {}", work.display()));
    paddock.wait_for_tabs(&["1:work"]);

    paddock.command(b"g");
    wait_shown(&paddock);
    let pane = wait_for_float(&paddock, &sandbox);
    let corner = paddock.full_rows()[CORNER.1].chars().nth(CORNER.0);
    assert_eq!(corner, Some('╭'), "{}", paddock.full_screen());
    paddock.wait_for("the float's pane sized to its box", |_| {
        sandbox.display(&pane, "#{pane_width}x#{pane_height}") == INSIDE
    });
    // Started where the focused terminal is.
    let dir = sandbox.display(&pane, "#{pane_current_path}");
    assert_eq!(
        std::fs::canonicalize(dir).unwrap(),
        std::fs::canonicalize(&work).unwrap()
    );
    // Never a tab.
    paddock.wait_for_tabs(&["1:work"]);

    paddock.type_line("kept-state");
    wait_for_full_text(&paddock, "kept-state");
    paddock.command(b"g");
    wait_hidden(&paddock);
    paddock.type_line("echo in-the-shell");
    paddock.wait_for_text("in-the-shell\n");

    paddock.command(b"g");
    wait_shown(&paddock);
    wait_for_full_text(&paddock, "kept-state");
    assert_eq!(wait_for_float(&paddock, &sandbox), pane, "the same program");
    paddock.wait_for_tabs(&["1:work"]);
}

#[test]
fn keys_go_to_the_float_and_not_to_the_terminal_under_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let shell = tab_panes(&sandbox, "space-1").remove(0);

    paddock.command(b"g");
    wait_shown(&paddock);
    let pane = wait_for_float(&paddock, &sandbox);
    paddock.type_line("typed-into-the-float");
    wait_for_full_text(&paddock, "typed-into-the-float");

    let float_screen = sandbox.tmux(&["capture-pane", "-p", "-t", &pane]);
    assert!(
        float_screen.contains("typed-into-the-float"),
        "{float_screen}"
    );
    let shell_screen = sandbox.tmux(&["capture-pane", "-p", "-t", &shell]);
    assert!(!shell_screen.contains("typed-into"), "{shell_screen}");
}

#[test]
fn when_its_program_exits_the_float_closes_and_the_next_toggle_starts_it_again() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"g");
    wait_shown(&paddock);
    let first = wait_for_float(&paddock, &sandbox);
    // `C-d` on an empty line ends `cat`.
    paddock.send(b"\x04");
    wait_hidden(&paddock);
    paddock.wait_for("the float gone from tmux", |_| floats(&sandbox).is_empty());
    paddock.type_line("echo back-in-the-shell");
    paddock.wait_for_text("back-in-the-shell\n");
    paddock.wait_for_tabs(&["1:home"]);

    paddock.command(b"g");
    wait_shown(&paddock);
    let second = wait_for_float(&paddock, &sandbox);
    assert_ne!(first, second, "a new program");
}

#[test]
fn a_command_that_changes_the_page_hides_the_float_and_tabs_never_include_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"g");
    wait_shown(&paddock);
    wait_for_float(&paddock, &sandbox);
    paddock.command(b"c");
    wait_hidden(&paddock);
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_current_tab("2:home");
    // Cycling and the last page go round the tabs only.
    for _ in 0..3 {
        paddock.command(b"n");
    }
    paddock.wait_for_current_tab("1:home");
    paddock.command(b"\t");
    paddock.wait_for_current_tab("2:home");
    assert_eq!(floats(&sandbox).len(), 1, "still running");
    assert_eq!(tab_panes(&sandbox, "space-1").len(), 2);
}

#[test]
fn each_space_has_its_own_float() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"g");
    wait_shown(&paddock);
    paddock.type_line("first-space");
    wait_for_full_text(&paddock, "first-space");
    paddock.command(b"g");
    wait_hidden(&paddock);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\r");
    paddock.wait_for("space-2 shown", |_| {
        sandbox.session_names() == ["space-1", "space-2"]
    });
    paddock.wait_for_text(PROMPT);
    paddock.command(b"g");
    wait_shown(&paddock);
    paddock.wait_for("a float in each space", |_| floats(&sandbox).len() == 2);
    assert!(!paddock.full_screen().contains("first-space"));
    let spaces: Vec<String> = floats(&sandbox).into_iter().map(|f| f.0).collect();
    assert_eq!(spaces, ["space-1", "space-2"]);
}

#[test]
fn closing_the_last_tab_closes_its_space_with_the_float() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"g");
    wait_shown(&paddock);
    wait_for_float(&paddock, &sandbox);
    paddock.command(b"g");
    wait_hidden(&paddock);
    // The only tab's shell exits: paddock has nothing left and quits.
    paddock.type_line("exit");
    assert!(paddock.wait_exit());
    assert!(sandbox.session_names().is_empty(), "the float went too");
}

#[test]
fn pasted_text_goes_to_the_float() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let shell = tab_panes(&sandbox, "space-1").remove(0);

    paddock.command(b"g");
    wait_shown(&paddock);
    let pane = wait_for_float(&paddock, &sandbox);
    paddock.send(b"\x1b[200~pasted-in\n\x1b[201~");
    wait_for_full_text(&paddock, "pasted-in");

    let float_screen = sandbox.tmux(&["capture-pane", "-p", "-t", &pane]);
    assert!(float_screen.contains("pasted-in"), "{float_screen}");
    let shell_screen = sandbox.tmux(&["capture-pane", "-p", "-t", &shell]);
    assert!(!shell_screen.contains("pasted-in"), "{shell_screen}");
}

#[test]
fn a_click_outside_the_float_does_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]);
    let (plus, bar) = paddock.find("+").expect("the + after the tabs");

    paddock.command(b"g");
    wait_shown(&paddock);
    wait_for_float(&paddock, &sandbox);
    // `+` would open a tab, and the sidebar's space row would take the
    // keyboard: neither does while the float is shown.
    paddock.click(plus, bar);
    paddock.click_screen(2, 1);
    paddock.type_line("still-typing-into-it");
    wait_for_full_text(&paddock, "still-typing-into-it");
    assert!(paddock.full_screen().contains(TITLE));
    assert_eq!(tab_panes(&sandbox, "space-1").len(), 1);
    paddock.wait_for_tabs(&["1:home"]);
}

/// A `PermissionRequest` hook payload for session `id`, run in `cwd`.
fn waiting(id: &str, cwd: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hooks/PermissionRequest.json"
    );
    std::fs::read_to_string(path)
        .unwrap()
        .replace("11111111-2222-4333-8444-555555555555", id)
        .replace("\"/tmp/repo\"", &format!("\"{cwd}\""))
}

#[test]
fn claude_in_a_float_is_not_followed() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let shell = tab_panes(&sandbox, "space-1").remove(0);
    paddock.command(b"g");
    wait_shown(&paddock);
    let pane = wait_for_float(&paddock, &sandbox);
    paddock.command(b"g");
    wait_hidden(&paddock);
    // Looking at another tab, so that the first one's wait notifies.
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");

    let in_float = waiting("aaaaaaaa-2222-4333-8444-555555555555", "/w/floaty");
    sandbox.hook(&pane, "PermissionRequest", &in_float);
    let in_tab = waiting("bbbbbbbb-2222-4333-8444-555555555555", "/w/tabby");
    sandbox.hook(&shell, "PermissionRequest", &in_tab);

    paddock.wait_for("the tab's agent", |_| {
        paddock.sidebar().iter().any(|l| l.contains("tabby"))
    });
    assert_eq!(sandbox.wait_for_notifications(1).len(), 1);
    let sidebar = paddock.sidebar();
    assert!(!sidebar.iter().any(|l| l.contains("floaty")), "{sidebar:?}");
    let sent = sandbox.notifications();
    assert_eq!(sent.len(), 1, "{sent:?}");
}
