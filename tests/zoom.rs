//! Ticket 29: `C-a z` zooms the focused split to fill its tab, through
//! tmux's `resize-pane -Z`, and again gives the others their places back.
//!
//! The screen is 100x30; the sidebar takes 32 columns and the tab bar a row,
//! so panes get 68x29. One side-by-side split leaves 34 + 1 + 33 columns.

mod harness;

use harness::{Paddock, Sandbox, PROMPT, TIMEOUT};
use std::time::{Duration, Instant};

/// Column of the border after one side-by-side split.
const V_BORDER: usize = 34;
/// Width of the session area.
const WIDTH: usize = 68;
const ALL_ROWS: (usize, usize) = (1, 30);
const LEFT: (usize, usize) = (0, V_BORDER);
const RIGHT: (usize, usize) = (V_BORDER + 1, WIDTH);
/// The panes of one side-by-side split, as [`sizes`] lists them.
const SPLIT: [&str; 2] = ["0.0 34x29", "0.1 33x29"];

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Every pane's size as tmux has it, as `window.pane WxH`, in order.
fn sizes(sandbox: &Sandbox) -> Vec<String> {
    let out = sandbox.tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{window_index}.#{pane_index} #{pane_width}x#{pane_height}",
    ]);
    out.lines().map(str::to_string).collect()
}

fn wait_for_sizes(sandbox: &Sandbox, expected: &[&str]) {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        if sizes(sandbox) == expected {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(sizes(sandbox), expected, "pane sizes in tmux");
}

/// Whether the first tab is zoomed, as tmux has it.
fn zoomed(sandbox: &Sandbox) -> bool {
    sandbox.display("space-1:0", "#{window_zoomed_flag}") == "1"
}

/// The part of a drawn row from column `from` (inclusive) to `to` (exclusive).
fn cols(row: &str, from: usize, to: usize) -> String {
    row.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

fn in_region(p: &Paddock, text: &str, rows: (usize, usize), columns: (usize, usize)) -> bool {
    p.rows()[rows.0..rows.1]
        .iter()
        .any(|r| cols(r, columns.0, columns.1).contains(text))
}

/// Wait until `text` shows in the given region (rows and columns of the screen).
fn wait_in_region(p: &Paddock, text: &str, rows: (usize, usize), columns: (usize, usize)) {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        if in_region(p, text, rows, columns) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "{text:?} not in rows {rows:?}, columns {columns:?}; screen:\n{}",
        p.full_screen()
    );
}

fn border_shown(p: &Paddock) -> bool {
    cols(&p.rows()[1], V_BORDER, V_BORDER + 1) == "│"
}

/// A side-by-side split with `left-pane` printed on the left, the right
/// pane focused.
fn split(sandbox: &Sandbox) -> Paddock {
    let mut p = ready(sandbox);
    p.type_line("echo left-pane");
    wait_in_region(&p, "left-pane", ALL_ROWS, LEFT);
    p.command(b"|");
    wait_in_region(&p, PROMPT, ALL_ROWS, RIGHT);
    wait_for_sizes(sandbox, &SPLIT);
    p
}

/// `C-a z` and wait until the right pane fills the tab, alone.
fn zoom(sandbox: &Sandbox, p: &mut Paddock) {
    p.command(b"z");
    wait_for_sizes(sandbox, &["0.0 34x29", "0.1 68x29"]);
    p.wait_for_tabs(&["1:home Z"]);
    p.wait_for("the left pane hidden", |_| {
        !in_region(p, "left-pane", ALL_ROWS, (0, WIDTH)) && !border_shown(p)
    });
}

#[test]
fn z_zooms_the_focused_split_and_again_gives_the_layout_back() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);

    zoom(&sandbox, &mut p);
    assert!(zoomed(&sandbox));
    p.type_line("echo typed-while-zoomed");
    wait_in_region(&p, "typed-while-zoomed", ALL_ROWS, LEFT);

    p.command(b"z");

    wait_for_sizes(&sandbox, &SPLIT);
    p.wait_for_tabs(&["1:home"]);
    p.wait_for("the border back", |_| border_shown(&p));
    wait_in_region(&p, "left-pane", ALL_ROWS, LEFT);
    wait_in_region(&p, "typed-while-zoomed", ALL_ROWS, RIGHT);
    p.type_line("echo focus-kept");
    wait_in_region(&p, "focus-kept", ALL_ROWS, RIGHT);
}

#[test]
fn a_click_in_the_zoomed_split_keeps_it_zoomed_and_focused() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);
    zoom(&sandbox, &mut p);

    // Where the hidden left pane would be.
    p.click(2, 5);
    p.type_line("echo clicked-while-zoomed");

    wait_in_region(&p, "clicked-while-zoomed", ALL_ROWS, LEFT);
    assert!(zoomed(&sandbox));
    p.wait_for_tabs(&["1:home Z"]);
}

#[test]
fn moving_focus_while_zoomed_unzooms_then_moves() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);
    zoom(&sandbox, &mut p);

    p.command(b"h");

    wait_for_sizes(&sandbox, &SPLIT);
    p.wait_for_tabs(&["1:home"]);
    p.type_line("echo went-left");
    wait_in_region(&p, "went-left", ALL_ROWS, LEFT);

    // The direct chord too: `C-l` from the left pane, zoomed.
    zoom_left(&sandbox, &mut p);
    p.send(b"\x0c");
    wait_for_sizes(&sandbox, &SPLIT);
    p.wait_for_tabs(&["1:home"]);
    p.type_line("echo went-right");
    wait_in_region(&p, "went-right", ALL_ROWS, RIGHT);
}

/// `C-a z` with the left pane focused.
fn zoom_left(sandbox: &Sandbox, p: &mut Paddock) {
    p.command(b"z");
    wait_for_sizes(sandbox, &["0.0 68x29", "0.1 33x29"]);
    p.wait_for_tabs(&["1:home Z"]);
}

#[test]
fn splitting_while_zoomed_unzooms() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);
    zoom(&sandbox, &mut p);

    p.command(b"-");

    wait_for_sizes(&sandbox, &["0.0 34x29", "0.1 33x14", "0.2 33x14"]);
    p.wait_for_tabs(&["1:home"]);
    assert!(!zoomed(&sandbox));
    wait_in_region(&p, "left-pane", ALL_ROWS, LEFT);
}

#[test]
fn closing_the_zoomed_split_unzooms() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);
    p.command(b"|");
    wait_for_sizes(&sandbox, &["0.0 34x29", "0.1 16x29", "0.2 16x29"]);
    p.command(b"z");
    wait_for_sizes(&sandbox, &["0.0 34x29", "0.1 16x29", "0.2 68x29"]);
    p.wait_for_tabs(&["1:home Z"]);

    p.command(b"x");

    wait_for_sizes(&sandbox, &["0.0 34x29", "0.1 33x29"]);
    p.wait_for_tabs(&["1:home"]);
    assert!(!zoomed(&sandbox));
    p.wait_for("the border back", |_| border_shown(&p));
    wait_in_region(&p, "left-pane", ALL_ROWS, LEFT);
}

#[test]
fn a_zoom_made_outside_paddock_shows() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);

    sandbox.tmux(&["resize-pane", "-Z", "-t", "space-1:0.0"]);

    p.wait_for_tabs(&["1:home Z"]);
    wait_for_sizes(&sandbox, &["0.0 68x29", "0.1 33x29"]);
    p.type_line("echo outside-zoom");
    wait_in_region(&p, "outside-zoom", ALL_ROWS, LEFT);
    assert!(!border_shown(&p));

    sandbox.tmux(&["resize-pane", "-Z", "-t", "space-1:0.0"]);

    p.wait_for_tabs(&["1:home"]);
    p.wait_for("the border back", |_| border_shown(&p));
}

#[test]
fn a_zoom_survives_detach_and_relaunch() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);
    zoom(&sandbox, &mut p);
    p.detach_keys();
    assert!(p.wait_exit());

    let mut again = ready(&sandbox);

    again.wait_for_tabs(&["1:home Z"]);
    assert!(zoomed(&sandbox));
    assert!(!in_region(&again, "left-pane", ALL_ROWS, (0, WIDTH)));
    again.command(b"z");
    wait_for_sizes(&sandbox, &SPLIT);
    again.wait_for_tabs(&["1:home"]);
}
