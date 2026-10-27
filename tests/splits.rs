//! Tickets 11 and 14: splits are tmux panes, and their layout lives in tmux.
//!
//! The screen is 100x30; the sidebar takes 32 columns and the tab bar a row,
//! so panes get 68x29. Rows and mouse columns in these tests count from the
//! right of the sidebar. A side-by-side split leaves one column for the
//! border (34 + 1 + 33), a top and bottom split one row (14 + 1 + 14).

mod harness;

use harness::{
    cols, in_region, split_sizes, wait_for_split_sizes, wait_in_region, Paddock, Sandbox, PROMPT,
};

/// Column of the border after one side-by-side split.
const V_BORDER: usize = 34;
/// Row of the border after a top and bottom split of the whole height.
const H_BORDER: usize = 15;
/// Width of the session area.
const WIDTH: usize = 68;

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Wait for a whole drawn line. Cells paddock blanked after a split went
/// away read as spaces, so lines are compared trimmed.
fn wait_for_line(p: &Paddock, line: &str) {
    p.wait_for(line, |s| s.lines().any(|l| l.trim_end() == line));
}

const ALL_ROWS: (usize, usize) = (1, 30);
const LEFT: (usize, usize) = (0, V_BORDER);
const RIGHT: (usize, usize) = (V_BORDER + 1, WIDTH);

/// `C-a |` and wait for the new pane's prompt on the right.
fn split_side_by_side(p: &mut Paddock) {
    p.command(b"|");
    wait_in_region(p, PROMPT, ALL_ROWS, RIGHT);
}

#[test]
fn native_needs_41_pipe_splits_side_by_side_into_a_new_pane() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);

    split_side_by_side(&mut p);

    wait_for_split_sizes(&sandbox, &["0.0 34x29", "0.1 33x29"]);
    p.wait_for_tabs(&["1:home"]);
    p.wait_for("vertical border", |_| {
        cols(&p.rows()[1], V_BORDER, V_BORDER + 1) == "│"
    });
    p.type_line("echo typed-into-the-new-split");
    wait_in_region(&p, "typed-into-the-new-split", ALL_ROWS, RIGHT);
    assert!(!in_region(&p, "typed-into-the-new-split", ALL_ROWS, LEFT));
    assert_eq!(sandbox.space_names(), ["space-1"], "one item");
}

#[test]
fn native_needs_41_minus_splits_top_and_bottom() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);

    p.command(b"-");

    wait_for_split_sizes(&sandbox, &["0.0 68x14", "0.1 68x14"]);
    p.wait_for("horizontal border", |_| {
        p.rows()[H_BORDER].starts_with("────")
    });
    p.type_line("echo typed-below");
    wait_in_region(&p, "typed-below", (H_BORDER + 1, 30), (0, WIDTH));
    assert!(!in_region(&p, "typed-below", (1, H_BORDER), (0, WIDTH)));
}

#[test]
fn native_needs_41_hjkl_and_arrows_move_focus() {
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
    wait_for_split_sizes(&sandbox, &["0.0 34x29", "0.1 33x14", "0.2 33x14"]);
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
fn native_needs_41_background_splits_keep_drawing_their_output() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.type_line("sleep 1; echo printed-while-unfocused");
    split_side_by_side(&mut p);

    wait_in_region(&p, "printed-while-unfocused", ALL_ROWS, LEFT);
}

#[test]
fn native_needs_41_x_closes_only_the_focused_split_and_its_neighbour_takes_the_space() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    wait_for_split_sizes(&sandbox, &["0.0 34x29", "0.1 33x29"]);

    p.command(b"x");

    wait_for_split_sizes(&sandbox, &["0.0 68x29"]);
    p.wait_for_tabs(&["1:home"]);
    p.type_line("echo focus-moved-back");
    wait_for_line(&p, "focus-moved-back");
}

#[test]
fn native_needs_41_a_shell_exiting_closes_only_its_split() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);

    p.type_line("exit");

    wait_for_split_sizes(&sandbox, &["0.0 68x29"]);
    p.type_line("echo still-here");
    wait_for_line(&p, "still-here");
}

#[test]
fn native_needs_41_equals_gives_the_focused_split_and_its_neighbours_the_same_size() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    // Splitting the right one again halves it.
    p.command(b"|");
    wait_for_split_sizes(&sandbox, &["0.0 34x29", "0.1 16x29", "0.2 16x29"]);

    p.command(b"=");

    wait_for_split_sizes(&sandbox, &["0.0 22x29", "0.1 22x29", "0.2 22x29"]);
}

#[test]
fn native_needs_41_splits_and_focus_come_back_after_detach_and_relaunch() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    // Tab 1: left | (top over bottom). Tab 2: one pane.
    split_side_by_side(&mut p);
    p.command(b"-");
    wait_for_split_sizes(&sandbox, &["0.0 34x29", "0.1 33x14", "0.2 33x14"]);
    p.command(b"c");
    p.wait_for_tabs(&["1:home", "2:home"]);
    p.command(b"1");
    p.wait_for_current_tab("1:home");
    p.command(b"k"); // focus the top right pane
    let before = split_sizes(&sandbox);
    p.detach_keys();
    assert!(p.wait_exit());

    let mut again = ready(&sandbox);

    again.wait_for_tabs(&["1:home", "2:home"]);
    again.wait_for_current_tab("1:home");
    assert_eq!(
        split_sizes(&sandbox),
        before,
        "same panes at the same sizes"
    );
    wait_in_region(&again, PROMPT, (H_BORDER + 1, 30), RIGHT);
    again.wait_for("vertical border", |_| {
        cols(&again.rows()[1], V_BORDER, V_BORDER + 1) == "│"
    });
    again.type_line("echo focus-restored");
    wait_in_region(&again, "focus-restored", (1, H_BORDER), RIGHT);
}

#[test]
fn native_needs_41_the_layout_lives_in_the_backend_not_in_the_state_file() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    split_side_by_side(&mut p);
    p.detach_keys();
    assert!(p.wait_exit());
    std::fs::remove_file(sandbox.path("state/paddock/layout.toml")).expect("layout file");

    let again = ready(&sandbox);

    again.wait_for_tabs(&["1:home"]);
    wait_for_split_sizes(&sandbox, &["0.0 34x29", "0.1 33x29"]);
    wait_in_region(&again, PROMPT, ALL_ROWS, RIGHT);
}

#[test]
fn native_needs_41_an_unreadable_layout_is_kept_aside_not_overwritten() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.detach_keys();
    assert!(p.wait_exit());
    let layout = sandbox.path("state/paddock/layout.toml");
    std::fs::write(&layout, "not a layout").unwrap();

    let mut again = ready(&sandbox);
    again.wait_for_text("invalid layout");
    again.command(b"c");
    again.wait_for_tabs(&["1:home", "2:home"]);

    let aside = sandbox.path("state/paddock/layout.toml.bad");
    assert_eq!(std::fs::read_to_string(aside).unwrap(), "not a layout");
}
