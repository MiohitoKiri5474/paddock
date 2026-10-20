//! Ticket 11: splits and layout restore.
//!
//! The screen is 100x30; the terminals get the 100x29 below the tab bar. A
//! side-by-side split leaves one column for the border (50 + 1 + 49), a top
//! and bottom split one row (14 + 1 + 14).

mod harness;

use harness::{Paddock, Sandbox, PROMPT, TIMEOUT};
use std::time::{Duration, Instant};

/// Column of the border after one side-by-side split.
const V_BORDER: usize = 50;
/// Row of the border after a top and bottom split of the whole height.
const H_BORDER: usize = 15;

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Every terminal's size as tmux has it, as `name WxH`, sorted by name.
fn sizes(sandbox: &Sandbox) -> Vec<String> {
    let out = sandbox.tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name} #{pane_width}x#{pane_height}",
    ]);
    let mut rows: Vec<String> = out.lines().map(str::to_string).collect();
    rows.sort();
    rows
}

fn wait_for_sizes(sandbox: &Sandbox, expected: &[&str]) {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        if sizes(sandbox) == expected {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(sizes(sandbox), expected, "terminal sizes in tmux");
}

/// The part of a drawn row from column `from` (inclusive) to `to` (exclusive).
fn cols(row: &str, from: usize, to: usize) -> String {
    row.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// Wait until `text` shows in the given region (rows and columns of the screen).
fn wait_in_region(p: &Paddock, text: &str, rows: (usize, usize), columns: (usize, usize)) {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        let found = p.rows()[rows.0..rows.1]
            .iter()
            .any(|r| cols(r, columns.0, columns.1).contains(text));
        if found {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "{text:?} not in rows {rows:?}, columns {columns:?}; screen:\n{}",
        p.screen()
    );
}

/// Wait for a whole drawn line. Cells paddock blanked after a split went
/// away read as spaces, so lines are compared trimmed.
fn wait_for_line(p: &Paddock, line: &str) {
    p.wait_for(line, |s| s.lines().any(|l| l.trim_end() == line));
}

fn in_region(p: &Paddock, text: &str, rows: (usize, usize), columns: (usize, usize)) -> bool {
    p.rows()[rows.0..rows.1]
        .iter()
        .any(|r| cols(r, columns.0, columns.1).contains(text))
}

const ALL_ROWS: (usize, usize) = (1, 30);
const LEFT: (usize, usize) = (0, V_BORDER);
const RIGHT: (usize, usize) = (V_BORDER + 1, 100);

/// `C-a |` and wait for the new terminal's prompt on the right.
fn split_side_by_side(p: &mut Paddock) {
    p.command(b"|");
    wait_in_region(p, PROMPT, ALL_ROWS, RIGHT);
}

#[test]
fn pipe_splits_side_by_side_into_a_new_terminal() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);

    split_side_by_side(&mut p);

    wait_for_sizes(&sandbox, &["terminal-1 50x29", "terminal-2 49x29"]);
    p.wait_for_tabs(&["1:terminal-1"]);
    p.wait_for("vertical border", |_| {
        cols(&p.rows()[1], V_BORDER, V_BORDER + 1) == "│"
    });
    p.type_line("echo typed-into-the-new-split");
    wait_in_region(&p, "typed-into-the-new-split", ALL_ROWS, RIGHT);
    assert!(!in_region(&p, "typed-into-the-new-split", ALL_ROWS, LEFT));
}

#[test]
fn minus_splits_top_and_bottom() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);

    p.command(b"-");

    wait_for_sizes(&sandbox, &["terminal-1 100x14", "terminal-2 100x14"]);
    p.wait_for("horizontal border", |_| {
        p.rows()[H_BORDER].starts_with("────")
    });
    p.type_line("echo typed-below");
    wait_in_region(&p, "typed-below", (H_BORDER + 1, 30), (0, 100));
    assert!(!in_region(&p, "typed-below", (1, H_BORDER), (0, 100)));
}

#[test]
fn hjkl_and_arrows_move_focus() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);

    p.command(b"h");
    p.type_line("echo went-left");
    wait_in_region(&p, "went-left", ALL_ROWS, LEFT);

    p.command(b"\x1b[C"); // right arrow
    p.type_line("echo went-right");
    wait_in_region(&p, "went-right", ALL_ROWS, RIGHT);

    // Split the right one top and bottom, then move up and down in it.
    p.command(b"-");
    wait_for_sizes(
        &sandbox,
        &["terminal-1 50x29", "terminal-2 49x14", "terminal-3 49x14"],
    );
    p.command(b"k");
    p.type_line("echo went-up");
    wait_in_region(&p, "went-up", (1, H_BORDER), RIGHT);
    p.command(b"j");
    p.type_line("echo went-down");
    wait_in_region(&p, "went-down", (H_BORDER + 1, 30), RIGHT);
    p.command(b"\x1b[D"); // left arrow
    p.type_line("echo left-again");
    wait_in_region(&p, "left-again", ALL_ROWS, LEFT);
}

#[test]
fn background_splits_keep_drawing_their_output() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.type_line("sleep 1; echo printed-while-unfocused");
    split_side_by_side(&mut p);

    wait_in_region(&p, "printed-while-unfocused", ALL_ROWS, LEFT);
}

#[test]
fn x_closes_only_the_focused_split_and_its_neighbour_takes_the_space() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    wait_for_sizes(&sandbox, &["terminal-1 50x29", "terminal-2 49x29"]);

    p.command(b"x");

    wait_for_sizes(&sandbox, &["terminal-1 100x29"]);
    assert_eq!(sandbox.session_names(), ["terminal-1"]);
    p.wait_for_tabs(&["1:terminal-1"]);
    p.type_line("echo focus-moved-back");
    wait_for_line(&p, "focus-moved-back");
}

#[test]
fn a_shell_exiting_closes_only_its_split() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);

    p.type_line("exit");

    wait_for_sizes(&sandbox, &["terminal-1 100x29"]);
    p.type_line("echo still-here");
    wait_for_line(&p, "still-here");
}

#[test]
fn equals_gives_every_split_the_same_size() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    // Splitting the right one again halves it.
    p.command(b"|");
    wait_for_sizes(
        &sandbox,
        &["terminal-1 49x29", "terminal-2 25x29", "terminal-3 24x29"],
    );

    p.command(b"=");

    wait_for_sizes(
        &sandbox,
        &["terminal-1 33x29", "terminal-2 32x29", "terminal-3 33x29"],
    );
}

#[test]
fn the_layout_is_restored_after_detach_and_relaunch() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    // Tab 1: terminal-1 | (terminal-2 over terminal-3). Tab 2: terminal-4.
    split_side_by_side(&mut p);
    p.command(b"-");
    wait_for_sizes(
        &sandbox,
        &["terminal-1 50x29", "terminal-2 49x14", "terminal-3 49x14"],
    );
    p.command(b"c");
    p.wait_for_tabs(&["1:terminal-1", "2:terminal-4"]);
    p.command(b"1");
    p.wait_for_current_tab("1:terminal-1");
    p.command(b"k"); // focus terminal-2, top right
    p.command(b",");
    p.wait_for_text("rename tab: terminal-1");
    p.send(b"\x15work\r");
    p.wait_for_tabs(&["1:work", "2:terminal-4"]);
    let before = sizes(&sandbox);
    p.detach_keys();
    assert!(p.wait_exit());

    let mut again = ready(&sandbox);

    again.wait_for_tabs(&["1:work", "2:terminal-4"]);
    again.wait_for_current_tab("1:work");
    assert_eq!(sizes(&sandbox), before, "same terminals at the same sizes");
    wait_in_region(&again, PROMPT, (H_BORDER + 1, 30), RIGHT);
    again.wait_for("vertical border", |_| {
        cols(&again.rows()[1], V_BORDER, V_BORDER + 1) == "│"
    });
    again.type_line("echo focus-restored");
    wait_in_region(&again, "focus-restored", (1, H_BORDER), RIGHT);
}

#[test]
fn without_a_saved_layout_every_terminal_comes_back_as_a_tab() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    p.detach_keys();
    assert!(p.wait_exit());
    std::fs::remove_file(sandbox.path("state/paddock/layout.toml")).expect("layout file");

    let again = ready(&sandbox);

    again.wait_for_tabs(&["1:terminal-1", "2:terminal-2"]);
    wait_for_sizes(&sandbox, &["terminal-1 100x29", "terminal-2 100x29"]);
}

#[test]
fn an_unreadable_layout_is_kept_aside_not_overwritten() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.detach_keys();
    assert!(p.wait_exit());
    let layout = sandbox.path("state/paddock/layout.toml");
    std::fs::write(&layout, "not a layout").unwrap();

    let mut again = ready(&sandbox);
    again.wait_for_text("invalid layout");
    again.command(b"c");
    again.wait_for_tabs(&["1:terminal-1", "2:terminal-2"]);

    let aside = sandbox.path("state/paddock/layout.toml.bad");
    assert_eq!(std::fs::read_to_string(aside).unwrap(), "not a layout");
}
