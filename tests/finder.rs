//! Ticket 23: the finder (`C-a g`), a box that lists spaces, tabs and
//! agents matching what is typed and shows the one chosen.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::time::Duration;

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
/// The finder's title, as drawn in its top border.
const TITLE: &str = " Finder ";

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

/// Two tabs, the first named `logs`, the second current.
fn two_tabs(sandbox: &Sandbox) -> Paddock {
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock.wait_for_tabs(&["1:home"]);
    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    paddock.send(b"\x15logs\r");
    paddock.wait_for_tabs(&["1:logs"]);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Wait until the finder shows `text` anywhere on the screen, sidebar
/// included (the box is drawn over it).
fn wait_in_finder(paddock: &Paddock, text: &str) {
    paddock.wait_for(&format!("the finder with {text:?}"), |_| {
        let s = paddock.full_screen();
        s.contains(TITLE) && s.contains(text)
    });
}

fn open(paddock: &mut Paddock) {
    paddock.command(b"g");
    wait_in_finder(paddock, "tab   space-1 › logs");
}

#[test]
fn g_opens_the_finder_and_typing_a_tabs_name_then_enter_shows_it() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);

    open(&mut paddock);
    // Every space, tab and agent is listed before anything is typed.
    wait_in_finder(&paddock, "space space-1");
    wait_in_finder(&paddock, "tab   space-1 › home");
    paddock.send(b"logs");
    paddock.wait_for("only the tab left", |_| {
        let s = paddock.full_screen();
        s.contains("tab   space-1 › logs") && !s.contains("space-1 › home")
    });
    paddock.send(b"\r");

    paddock.wait_for_no_popup();
    paddock.wait_for_current_tab("1:logs");
    paddock.type_line("echo in-logs");
    paddock.wait_for_text("in-logs\n");
}

#[test]
fn an_agent_row_opens_its_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "space-1:0", "Stop", "s1", "/w/alpha");
    paddock.wait_for_tabs(&["1:● logs", "2:home"]);

    open(&mut paddock);
    // The agent is labelled after its tab; its directory matches too.
    paddock.send(b"alpha");
    wait_in_finder(&paddock, "agent logs");
    paddock.send(b"\r");

    paddock.wait_for_no_popup();
    paddock.wait_for_current_tab("1:● logs");
}

#[test]
fn state_letters_filter_agents_on_an_empty_query() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    // The agent in `logs` waits; bravo finished its turn in the tab
    // looked at: idle.
    claude(&sandbox, "space-1:0", "PermissionRequest", "s1", "/w/alpha");
    claude(&sandbox, "space-1:1", "Stop", "s2", "/w/bravo");
    paddock.wait_for_tabs(&["1:● logs", "2:● claude"]);

    open(&mut paddock);
    wait_in_finder(&paddock, "agent logs");
    paddock.send(b"b");
    paddock.wait_for("only waiting agents", |_| {
        let s = paddock.full_screen();
        s.contains(" Finder · waiting ") && s.contains("agent logs") && !s.contains("agent bravo")
    });
    paddock.send(b"i");
    paddock.wait_for("only idle agents", |_| {
        let s = paddock.full_screen();
        s.contains(" Finder · idle ") && s.contains("agent bravo") && !s.contains("agent logs")
    });
    paddock.send(b"a");
    paddock.wait_for("everything again", |_| {
        let s = paddock.full_screen();
        s.contains(TITLE) && s.contains("agent logs") && s.contains("space space-1")
    });
    // With text in the field, `b` is a character.
    paddock.send(b"alb");
    wait_in_finder(&paddock, "no matches");
    paddock.send(b"\x1b");
    paddock.wait_for_no_popup();
}

#[test]
fn esc_and_a_click_outside_cancel_and_change_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);

    open(&mut paddock);
    paddock.send(b"logs\x1b");
    paddock.wait_for_no_popup();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(paddock.current_tab(), "2:home");
    paddock.type_line("echo after-esc");
    paddock.wait_for_text("after-esc\n");

    open(&mut paddock);
    paddock.send(b"logs");
    // The 64x15 box is centred on the 100x30 screen: columns 18-81, rows
    // 7-21. A click inside, off the rows, does nothing.
    paddock.click_screen(40, 8);
    std::thread::sleep(Duration::from_millis(200));
    wait_in_finder(&paddock, "tab   space-1 › logs");
    paddock.click_screen(90, 27);
    paddock.wait_for_no_popup();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(paddock.current_tab(), "2:home");
}

#[test]
fn a_click_on_a_row_opens_it() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);

    open(&mut paddock);
    paddock.send(b"logs");
    paddock.wait_for("only the tab left", |_| {
        !paddock.full_screen().contains("space-1 › home")
    });
    // The first row: under the border and the line typed.
    paddock.click_screen(40, 9);

    paddock.wait_for_no_popup();
    paddock.wait_for_current_tab("1:logs");
}

#[test]
fn the_finder_opened_from_the_dashboard_returns_to_it_on_esc() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    paddock.command(b"o");
    paddock.wait_for_text("j down  k up");

    open(&mut paddock);
    paddock.send(b"\x1b");
    paddock.wait_for_no_popup();
    paddock.wait_for_text("j down  k up");
}
