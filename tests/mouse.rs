//! Ticket 12: the mouse. Clicks focus terminals, jump to sidebar rows and
//! switch tabs; dragging copies text; programs that ask for the mouse get it
//! in their own coordinates unless Shift is held.
//!
//! The screen is 100x30. After one side-by-side split the left terminal is
//! columns 0-49, the border column 50, the right terminal columns 51-99.

mod harness;

use harness::{Paddock, Sandbox, PROMPT, TIMEOUT};
use std::time::{Duration, Instant};

const RIGHT: u16 = 51;
const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

fn split_side_by_side(p: &mut Paddock) {
    p.command(b"|");
    p.wait_for("two prompts", |s| s.matches(PROMPT).count() == 2);
}

/// Wait until `text` shows somewhere in columns `from..to` of the screen.
fn wait_in_columns(p: &Paddock, text: &str, from: usize, to: usize) {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        let found = p.rows().iter().any(|r| {
            r.chars()
                .skip(from)
                .take(to - from)
                .collect::<String>()
                .contains(text)
        });
        if found {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "{text:?} not in columns {from}..{to}; screen:\n{}",
        p.screen()
    );
}

/// The screen row where the output line `line` (whole row, trimmed) is drawn.
fn row_of(p: &Paddock, line: &str) -> u16 {
    p.wait_for(line, |s| s.lines().any(|l| l.trim_end() == line));
    let row = p.rows().iter().position(|r| r == line).expect("line");
    u16::try_from(row).unwrap()
}

#[test]
fn clicking_a_split_focuses_it() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p); // the new right terminal has the focus

    p.click(10, 10);
    p.type_line("echo left-$((1+1))");
    wait_in_columns(&p, "left-2", 0, 50);

    p.click(70, 10);
    p.type_line("echo right-$((2+2))");
    wait_in_columns(&p, "right-4", 51, 100);
}

#[test]
fn clicking_a_tab_label_switches_tabs() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.command(b"c");
    p.wait_for_tabs(&["1:terminal-1", "2:terminal-2"]);
    p.wait_for_current_tab("2:terminal-2");

    // " 1:terminal-1 " takes columns 0-13, " 2:terminal-2 " 14-27.
    p.click(3, 0);
    p.wait_for_current_tab("1:terminal-1");
    p.click(20, 0);
    p.wait_for_current_tab("2:terminal-2");
}

#[test]
fn clicking_a_sidebar_row_jumps_to_its_terminal() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    let start = std::fs::read_to_string(format!("{SPIKE}/SessionStart.json")).expect("payload");
    sandbox.hook("terminal-1", "SessionStart", &start);
    p.wait_for_text("sessions");
    p.command(b"c");
    p.wait_for_current_tab("2:terminal-2");

    // Row 0 is the sidebar's title, row 1 its first session.
    p.click(3, 1);
    p.wait_for_current_tab("1:terminal-1");
}

#[test]
fn wheel_scrolls_the_split_under_the_pointer() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.type_line("seq 1 100");
    p.wait_for("seq", |s| s.lines().any(|l| l.trim() == "100"));
    split_side_by_side(&mut p);

    p.wheel(true, 10, 10);
    wait_in_columns(&p, "[3/", 0, 50);
}

#[test]
fn dragging_copies_exactly_the_selected_text() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.type_line("echo \"alpha $((40+2))\"; echo \"beta $((6*7))\"");
    let row = row_of(&p, "alpha 42");

    // A click alone copies nothing.
    p.click(2, row);
    p.type_line("echo synced-$((1))");
    p.wait_for_text("synced-1");
    assert_eq!(sandbox.clipboard(), None);

    p.drag((0, row), (4, row), false);
    sandbox.wait_for_clipboard("alpha");
    p.drag((2, row), (2, row + 1), false);
    sandbox.wait_for_clipboard("pha 42\nbet");
    p.drag((3, row + 1), (6, row), false);
    sandbox.wait_for_clipboard("42\nbeta");
}

#[test]
fn programs_that_ask_for_the_mouse_get_it_in_their_own_coordinates() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    // The right terminal turns on drag reports (SGR), then shows what it gets.
    p.type_line(
        "printf '\\033[?1002h\\033[?1006h'; echo \"pick-$((1))-me\"; stty raw -echo; cat -v",
    );
    p.wait_for_text("pick-1-me");
    let (col, row) = p.find("pick-1-me").expect("marker");
    assert!(col >= RIGHT, "marker in the right terminal");
    p.command(b"h"); // the left terminal has the focus, not the one clicked

    // Column 60, row 5 of the screen is column 9, row 4 of the right terminal.
    p.click(60, 5);
    wait_in_columns(&p, "^[[<0;10;5M^[[<0;10;5m", 51, 100);

    // Shift keeps the mouse in paddock: dragging copies.
    p.drag((col, row), (col + 5, row), true);
    sandbox.wait_for_clipboard("pick-1");
}
