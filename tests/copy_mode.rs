//! Ticket 28: keyboard copy mode and scrollback search. `C-a [` freezes the
//! terminal typing goes to and moves a cursor over its history; `v`/`V`
//! and `y` copy through the same clipboard command as dragging (faked
//! here); `/` and `?` search the history.
//!
//! The screen is 100x30 and the sidebar takes 32 columns, so a terminal is
//! 68 columns by 29 rows; columns here count from the sidebar's right.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::time::Duration;

/// The copy-mode line's start.
const COPY: &str = " COPY  line";
/// A search match's background: the terminal's yellow.
const MATCH_BG: vt100::Color = vt100::Color::Idx(3);
/// Columns of the left terminal after a side-by-side split.
const LEFT: usize = 34;

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

fn enter_copy_mode(p: &mut Paddock) {
    p.command(b"[");
    p.wait_for_text(COPY);
}

fn wait_left(p: &Paddock) {
    p.wait_for("copy mode left", |s| !s.contains(COPY));
}

/// Whether some row of the left terminal is exactly `line`.
fn shows_on_the_left(p: &Paddock, line: &str) -> bool {
    p.rows().iter().any(|r| on_the_left(r) == line)
}

/// The part of a screen row in the left terminal of a side-by-side split.
fn on_the_left(row: &str) -> String {
    row.chars()
        .take(LEFT)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn v_and_y_copy_the_selection_through_the_clipboard_and_leave() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.type_line("echo \"alpha $((40+2))\"; echo \"beta $((6*7))\"");
    p.wait_for("output", |s| s.lines().any(|l| l == "beta 42"));

    enter_copy_mode(&mut p);
    // From the prompt: up twice to `alpha 42`, to its start, select to the
    // end of its second word.
    p.send(b"kk0vwe");
    p.send(b"y");
    sandbox.wait_for_clipboard("alpha 42");
    wait_left(&p);

    // A line selection, upwards over two lines.
    enter_copy_mode(&mut p);
    p.send(b"kVk");
    p.send(b"\r");
    sandbox.wait_for_clipboard("alpha 42\nbeta 42");
    wait_left(&p);

    // Leaving without copying copies nothing.
    enter_copy_mode(&mut p);
    p.send(b"kv0q");
    wait_left(&p);
    assert_eq!(sandbox.clipboard().as_deref(), Some("alpha 42\nbeta 42"));
    // Keys go to the terminal again.
    p.type_line("echo back-$((1+1))");
    p.wait_for_text("back-2");
}

#[test]
fn a_line_selection_copies_a_wrapped_line_as_one() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    // 100 characters: 68 on one row, 32 wrapped onto the next.
    p.type_line("printf 'w%.0s' $(seq 1 100); echo");
    p.wait_for("wrapped output", |s| s.lines().any(|l| l == "w".repeat(32)));

    enter_copy_mode(&mut p);
    p.send(b"kVy");
    sandbox.wait_for_clipboard(&"w".repeat(100));
}

#[test]
fn search_jumps_through_the_history_wrapping_and_highlights_matches() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    // Three `x0y` lines 41 apart: the first two end up in the history.
    // The command line itself reads `x$((i*0))y`, which does not match.
    p.type_line("for i in 1 2 3; do echo \"x$((i*0))y\"; seq 1 40; done");
    p.wait_for("output", |s| s.lines().any(|l| l == "40"));

    p.command(b"/");
    p.wait_for("search prompt", |s| s.lines().any(|l| l.trim() == "/"));
    p.send(b"X0Y\r");
    p.wait_for_text("no match for \"X0Y\"");

    // Lowercase ignores case; `/` goes back up from the cursor.
    p.send(b"/x0y\r");
    p.wait_for_text("match 3/3");
    let (col, row) = p.find("x0y").expect("the match on screen");
    let left = p.left();
    assert_eq!(p.colors(row, col + left).1, MATCH_BG);
    assert_eq!(p.colors(row, col + left + 2).1, MATCH_BG);
    p.send(b"n");
    p.wait_for_text("match 2/3");
    p.send(b"n");
    p.wait_for_text("match 1/3");
    p.send(b"n");
    p.wait_for_text("match 3/3");
    p.send(b"N");
    p.wait_for_text("match 1/3");
    // `?` searches down from the cursor.
    p.send(b"?x0y\r");
    p.wait_for_text("match 2/3");

    p.send(b"\x1b");
    wait_left(&p);
    p.wait_for("the live view", |s| s.lines().any(|l| l == PROMPT));
}

#[test]
fn output_while_in_copy_mode_shows_only_after_leaving() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    let done = sandbox.path("done");
    p.type_line(&format!(
        "sleep 1; echo late-$((5+5)); touch '{}'",
        done.display()
    ));
    enter_copy_mode(&mut p);
    let start = std::time::Instant::now();
    while !done.exists() {
        assert!(start.elapsed() < harness::TIMEOUT, "the command never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Time for the output to reach paddock, which holds it back.
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !p.screen().contains("late-10"),
        "the view moved:\n{}",
        p.screen()
    );
    assert!(p.screen().contains(COPY));

    p.send(b"q");
    p.wait_for_text("late-10");
    wait_left(&p);
}

#[test]
fn the_wheel_scrolls_the_copy_view_and_direct_chords_do_not_fire() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    p.type_line("seq 1 100");
    p.wait_for("seq", |s| s.lines().any(|l| l == "100"));
    p.command(b"|");
    p.wait_for("two prompts", |s| s.matches(PROMPT).count() == 2);
    // The left terminal, with the numbers.
    p.command(b"h");

    enter_copy_mode(&mut p);
    assert!(!shows_on_the_left(&p, "71"));
    p.wheel(true, 10, 10);
    // Three lines: 70 to 98 show, 70 under the copy-mode line.
    p.wait_for("71 scrolled into view", |s| {
        s.lines().any(|l| on_the_left(l) == "71")
    });
    // `C-l` would focus the right terminal; in copy mode it does nothing.
    p.send(b"\x0c");
    p.send(b"q");
    wait_left(&p);
    p.type_line("echo still-$((2+1))");
    p.wait_for("typed on the left", |s| {
        s.lines().any(|l| on_the_left(l).contains("still-3"))
    });
}

#[test]
fn a_command_or_the_pane_closing_leaves_copy_mode() {
    let sandbox = Sandbox::new();
    let mut p = ready(&sandbox);
    enter_copy_mode(&mut p);
    // A command leaves copy mode first.
    p.command(b"c");
    p.wait_for_tabs(&["1:home", "2:home"]);
    wait_left(&p);

    p.command(b"|");
    p.wait_for("two prompts", |s| s.matches(PROMPT).count() == 2);
    enter_copy_mode(&mut p);
    // The focused (right) terminal closes under copy mode.
    let focused = sandbox
        .tmux(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_active} #{window_active} #{pane_id}",
        ])
        .lines()
        .find(|l| l.starts_with("1 1 "))
        .map(|l| l[4..].to_string())
        .expect("the focused pane");
    sandbox.tmux(&["kill-pane", "-t", &focused]);
    wait_left(&p);
    p.wait_for("one prompt", |s| s.matches(PROMPT).count() == 1);
    p.type_line("echo after-$((3+4))");
    p.wait_for_text("after-7");
}

#[test]
fn copy_mode_works_in_the_float_shown() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[floats]\nG = \"cat\"\n");
    let mut p = ready(&sandbox);
    p.command(b"G");
    p.wait_for("the float", |_| p.full_screen().contains(" cat "));
    p.type_line("float-text");
    p.wait_for("cat's echo", |_| {
        p.full_screen().matches("float-text").count() == 2
    });

    p.command(b"[");
    p.wait_for("copy mode in the float", |_| p.full_screen().contains(COPY));
    // The cursor starts on cat's empty line: up to its copy of the text.
    p.send(b"kVy");
    sandbox.wait_for_clipboard("float-text");
    p.wait_for("copy mode left", |_| !p.full_screen().contains(COPY));
    assert!(p.full_screen().contains(" cat "), "the float stays shown");
}
