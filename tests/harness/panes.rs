//! Splits as tests see them: every split's size as the backend has it,
//! and text in a region of the drawn screen.

use super::{Paddock, Sandbox, TIMEOUT};
use std::time::{Duration, Instant};

/// Every tab's splits' sizes as the backend has them, as `tab.split WxH`
/// (counting from 0), space by space, in order.
pub fn split_sizes(sandbox: &Sandbox) -> Vec<String> {
    let state = sandbox.state();
    let tabs = state.spaces.iter().flat_map(|s| s.tabs.iter().enumerate());
    tabs.flat_map(|(i, t)| {
        let splits = t.splits.iter().enumerate();
        splits.map(move |(j, s)| format!("{i}.{j} {}", s.size()))
    })
    .collect()
}

/// Wait until [`split_sizes`] is `expected`.
pub fn wait_for_split_sizes(sandbox: &Sandbox, expected: &[&str]) {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        if split_sizes(sandbox) == expected {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(split_sizes(sandbox), expected, "split sizes");
}

/// The part of a drawn row from column `from` (inclusive) to `to` (exclusive).
pub fn cols(row: &str, from: usize, to: usize) -> String {
    row.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// Whether `text` shows in the given region (rows and columns of the
/// screen right of the sidebar).
pub fn in_region(p: &Paddock, text: &str, rows: (usize, usize), columns: (usize, usize)) -> bool {
    p.rows()[rows.0..rows.1]
        .iter()
        .any(|r| cols(r, columns.0, columns.1).contains(text))
}

/// Wait until `text` shows in the given region (see [`in_region`]).
pub fn wait_in_region(p: &Paddock, text: &str, rows: (usize, usize), columns: (usize, usize)) {
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
