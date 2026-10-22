//! Ticket 30: resizing splits. `C-a H/J/K/L` move the focused split's
//! border by 2 (tmux's `resize-pane`), and repeat without the prefix for
//! `[keys] repeat_ms`; dragging the border between two splits moves it,
//! live, and the layout stays in tmux.
//!
//! The screen is 100x30; the sidebar takes 32 columns and the tab bar a row,
//! so panes get 68x29. Columns here count from the sidebar's right, as the
//! harness does; rows from the top of the screen. One side-by-side split
//! leaves 34 + 1 + 33 columns, the border at column 34.

mod harness;

use harness::{pane_sizes, wait_for_pane_sizes, wait_in_region, Paddock, Sandbox, PROMPT};
use std::time::{Duration, Instant};

const V_BORDER: u16 = 34;
const ALL_ROWS: (usize, usize) = (1, 30);
const RIGHT: (usize, usize) = (V_BORDER as usize + 1, 68);
/// The panes of one side-by-side split, as [`harness::pane_sizes`] lists them.
const SPLIT: [&str; 2] = ["0.0 34x29", "0.1 33x29"];
/// Press, drag and release codes of a left-button SGR report.
const PRESS: u16 = 0;
const DRAG: u16 = 32;

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// A side-by-side split, the right pane focused.
fn split(sandbox: &Sandbox) -> Paddock {
    let mut p = ready(sandbox);
    p.command(b"|");
    wait_in_region(&p, PROMPT, ALL_ROWS, RIGHT);
    wait_for_pane_sizes(sandbox, &SPLIT);
    p
}

/// Whether the pane sizes stay `expected` for a while.
fn sizes_stay(sandbox: &Sandbox, expected: &[&str]) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        assert_eq!(pane_sizes(sandbox), expected, "pane sizes in tmux");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn shift_h_and_l_move_the_border_and_repeat_without_the_prefix() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);

    // One write: the second `H` comes well within the repeat window.
    p.send(b"\x01HH");
    wait_for_pane_sizes(&sandbox, &["0.0 30x29", "0.1 37x29"]);

    p.send(b"\x01LL");
    wait_for_pane_sizes(&sandbox, &SPLIT);

    // After the window, `H` is typed again.
    std::thread::sleep(Duration::from_millis(700));
    p.send(b"H");
    wait_in_region(&p, &format!("{PROMPT} H"), ALL_ROWS, RIGHT);
    sizes_stay(&sandbox, &SPLIT);
}

#[test]
fn shift_k_and_j_move_a_border_between_rows() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.command(b"-");
    wait_for_pane_sizes(&sandbox, &["0.0 68x14", "0.1 68x14"]);

    p.send(b"\x01KK");
    wait_for_pane_sizes(&sandbox, &["0.0 68x10", "0.1 68x18"]);
    p.send(b"\x01J");
    wait_for_pane_sizes(&sandbox, &["0.0 68x12", "0.1 68x16"]);
}

#[test]
fn a_zero_repeat_window_types_the_second_key() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[keys]\nrepeat_ms = 0\n");
    let mut p = split(&sandbox);

    p.send(b"\x01HH");

    wait_for_pane_sizes(&sandbox, &["0.0 32x29", "0.1 35x29"]);
    // The right pane now starts at column 33.
    wait_in_region(&p, &format!("{PROMPT} H"), ALL_ROWS, (33, 68));
    sizes_stay(&sandbox, &["0.0 32x29", "0.1 35x29"]);
}

#[test]
fn a_split_stops_at_one_column_without_a_notice() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);

    for _ in 0..25 {
        p.send(b"\x01H");
    }

    wait_for_pane_sizes(&sandbox, &["0.0 1x29", "0.1 66x29"]);
    p.type_line("echo still-$((1+1))");
    wait_in_region(&p, "still-2", ALL_ROWS, (2, 68));
    assert!(!p.tab_bar().contains("resize"), "{}", p.tab_bar());
}

#[test]
fn dragging_the_border_between_splits_resizes_them_live() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);

    p.mouse(PRESS, V_BORDER, 10, false);
    p.mouse(DRAG, 40, 12, false);
    // Before the release: the drag is live.
    wait_for_pane_sizes(&sandbox, &["0.0 40x29", "0.1 27x29"]);
    p.mouse(DRAG, 42, 12, false);
    p.mouse(DRAG, 44, 12, false);
    p.mouse(PRESS, 44, 12, true);

    // The last place goes to tmux on release, however fast the drag.
    wait_for_pane_sizes(&sandbox, &["0.0 44x29", "0.1 23x29"]);
    // No text was selected, no program got the mouse, nothing was focused.
    assert_eq!(sandbox.clipboard(), None);
    p.type_line("echo focus-$((2+2))");
    wait_in_region(&p, "focus-4", ALL_ROWS, (45, 68));

    // Dragging back past the start works the same.
    p.mouse(PRESS, 44, 3, false);
    p.mouse(DRAG, 20, 3, false);
    p.mouse(PRESS, 20, 3, true);
    wait_for_pane_sizes(&sandbox, &["0.0 20x29", "0.1 47x29"]);
    assert_eq!(sandbox.clipboard(), None);
}

#[test]
fn dragging_a_border_between_rows_resizes_them() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.command(b"-");
    wait_for_pane_sizes(&sandbox, &["0.0 68x14", "0.1 68x14"]);

    // Window row 14 is screen row 15, under the tab bar.
    p.mouse(PRESS, 10, 15, false);
    p.mouse(DRAG, 10, 20, false);
    p.mouse(PRESS, 10, 20, true);

    wait_for_pane_sizes(&sandbox, &["0.0 68x19", "0.1 68x9"]);
    assert_eq!(sandbox.clipboard(), None);
}

#[test]
fn dragging_a_border_beside_a_nested_row_moves_that_border() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);
    // `{A | B}` over C, beside D: B is the last of its row, so resizing B
    // itself would move the border between A and B instead.
    p.command(b"h");
    p.command(b"-");
    wait_for_pane_sizes(&sandbox, &["0.0 34x14", "0.1 34x14", "0.2 33x29"]);
    p.command(b"k");
    p.command(b"|");
    wait_for_pane_sizes(
        &sandbox,
        &["0.0 17x14", "0.1 16x14", "0.2 34x14", "0.3 33x29"],
    );

    p.mouse(PRESS, V_BORDER, 5, false);
    p.mouse(DRAG, 40, 5, false);
    p.mouse(PRESS, 40, 5, true);

    let start = Instant::now();
    loop {
        let sizes = pane_sizes(&sandbox);
        let moved =
            sizes.iter().any(|s| s == "0.2 40x14") && sizes.iter().any(|s| s == "0.3 27x29");
        if moved {
            break;
        }
        assert!(
            start.elapsed() < harness::TIMEOUT,
            "the outer border did not move: {sizes:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_sidebar_border_still_resizes_the_sidebar_beside_splits() {
    let sandbox = Sandbox::new();
    let mut p = split(&sandbox);

    // Screen column 31 is the sidebar's border.
    p.drag_screen(10, 31, 39);

    let start = Instant::now();
    while sandbox.display("space-1:0", "#{window_width}") != "60" {
        assert!(
            start.elapsed() < harness::TIMEOUT,
            "the sidebar did not widen"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(sandbox.clipboard(), None);
}
