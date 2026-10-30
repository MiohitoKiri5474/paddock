//! Ticket 64: a paddock attaching to the native server draws a terminal
//! as soon with a full scrollback as with none: its screens come first,
//! its scrollback after, and copy mode and the wheel still reach every
//! line of it. Every test here picks the native backend in its config,
//! so it runs on either run of the suite.

mod harness;

use harness::backend::NATIVE;
use harness::stall::wait_with_progress;
use harness::{Paddock, Sandbox, PROMPT};
use std::time::{Duration, Instant};

/// The copy-mode line's start.
const COPY: &str = " COPY  line";
/// A full scrollback (50,000 lines) and more, each line its number and
/// a colour change every few cells, which take long to write and to
/// read; then a line to wait for.
const FILL: &str = "seq 1 60000 | awk '{s=$1; for (i = 0; i < 12; i++) \
s = s sprintf(\"\\033[3%dm-\\033[4%dm=+\", i % 8, (i + 3) % 8); print s \"\\033[0m\"}'; \
echo FIL''LED-$((1+1))";
/// What [`FILL`] ends with.
const FILLED: &str = "FILLED-2";

fn native() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);
    sandbox
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Wait until `text` shows, as long as the screen still changes: under
/// load, drawing 60,000 lines can take longer than [`harness::TIMEOUT`],
/// which then counts from the last change.
fn wait_while_streaming(paddock: &Paddock, text: &str) {
    wait_with_progress(
        || paddock.screen(),
        |screen| screen.contains(text),
        |_| {
            format!(
                "no {text}, no longer changing; screen:\n{}",
                paddock.full_screen()
            )
        },
    );
}

/// A new paddock attached to the sandbox's server (the last one gone),
/// and how long it took to draw `text`, which its terminal shows.
fn attach_again(sandbox: &Sandbox, last: Paddock, text: &str) -> (Paddock, Duration) {
    drop(last);
    let start = Instant::now();
    let again = sandbox.launch();
    again.wait_for(text, |s| s.contains(text));
    (again, start.elapsed())
}

/// The copy-mode view after going to the top of the history (`g`): the
/// terminal's top row and the copy-mode line (`line 1/M`). `g` is
/// pressed until the top holds the whole history, as the scrollback
/// may come after copy mode starts.
fn top_of_history(p: &mut Paddock, lines: Option<&str>) -> (String, String) {
    p.command(b"[");
    p.wait_for_text(COPY);
    let wanted = |s: &str| {
        let status = s.lines().find(|l| l.contains(COPY));
        status.is_some_and(|l| {
            l.contains("line 1/") && lines.is_none_or(|n| l.contains(&format!("line 1/{n} ")))
        })
    };
    p.wait_for_poking("the top of the history", b"g", wanted);
    let rows = p.rows();
    let status = rows.iter().find(|r| r.contains(COPY)).unwrap();
    let status = status.trim().to_string();
    let top = rows[1].clone();
    p.send(b"q");
    p.wait_for("copy mode left", |s| !s.contains(COPY));
    (top, status)
}

/// `M` of a copy-mode line ` COPY  line N/M ...`.
fn total_lines(status: &str) -> &str {
    let after = status.split("line ").nth(1).unwrap();
    let after = after.split('/').nth(1).unwrap();
    after.split_whitespace().next().unwrap()
}

#[test]
fn the_first_draw_takes_no_longer_with_a_full_scrollback() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo EMP''TY-$((1+1))");
    paddock.wait_for_text("EMPTY-2");
    // The longest of two, for a fair bound on a loaded machine.
    let mut empty = Duration::ZERO;
    for _ in 0..2 {
        let (again, took) = attach_again(&sandbox, paddock, "EMPTY-2");
        empty = empty.max(took);
        paddock = again;
    }
    paddock.type_line(FILL);
    wait_while_streaming(&paddock, FILLED);
    let (_again, full) = attach_again(&sandbox, paddock, FILLED);
    // Before ticket 64, the scrollback came first: about 2 s more on an
    // idle machine, 10 s and more on a loaded one.
    let bound = empty * 3 + Duration::from_secs(1);
    assert!(
        full < bound,
        "{full:?} to draw with a full scrollback, {empty:?} with none"
    );
}

#[test]
fn after_attaching_copy_mode_and_the_wheel_reach_the_whole_scrollback() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("seq 1 60000; echo FIL''LED-$((1+1))");
    wait_while_streaming(&paddock, FILLED);
    // What the paddock that saw it all keeps: 50,000 lines and more.
    let (top, status) = top_of_history(&mut paddock, None);
    let lines = total_lines(&status).to_string();
    assert!(lines.parse::<usize>().unwrap() > 50_000, "{status}");
    assert!(top.parse::<u32>().is_ok(), "a number at the top: {top:?}");

    let (mut again, _) = attach_again(&sandbox, paddock, FILLED);
    assert_eq!(top_of_history(&mut again, Some(&lines)), (top, status));
    // The wheel scrolls back past the screen, deep into the numbers.
    let live: u32 = again
        .rows()
        .iter()
        .filter_map(|r| r.parse().ok())
        .min()
        .unwrap();
    let deep = |s: &str| {
        s.lines()
            .any(|l| l.parse::<u32>().is_ok_and(|n| n + 50 < live))
    };
    let start = Instant::now();
    while !deep(&again.screen()) {
        assert!(start.elapsed() < harness::TIMEOUT, "{}", again.screen());
        again.wheel(true, 10, 10);
        std::thread::sleep(Duration::from_millis(50));
    }
}
