//! Ticket 47: the native server saves each terminal's scrollback beside
//! `layout.json` as it ends, and a new server shows it above each
//! restored terminal's fresh shell (ADR 0008). Every test here picks the
//! native backend in its config, so it runs on either run of the suite.

mod harness;

use harness::backend::{stop_server, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The dim line under a restored terminal's saved scrollback.
const MARK: &str = "scrollback saved";

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

/// The saved scrollback files, by name.
fn saved_files(sandbox: &Sandbox) -> Vec<PathBuf> {
    let dir = sandbox.state_dir().join("scrollback");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries.map(|e| e.unwrap().path()).collect();
    files.sort();
    files
}

/// Wait until `scrollback/` holds `n` files.
fn wait_for_files(sandbox: &Sandbox, n: usize) {
    let start = Instant::now();
    while saved_files(sandbox).len() != n {
        let files = saved_files(sandbox);
        assert!(start.elapsed() < harness::TIMEOUT, "scrollback/: {files:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until `layout.json` names `n` scrollback files.
fn wait_for_saved_layout(sandbox: &Sandbox, n: usize) {
    let path = sandbox.state_dir().join("layout.json");
    let start = Instant::now();
    loop {
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        if saved.matches("\"scrollback\"").count() == n {
            return;
        }
        assert!(start.elapsed() < harness::TIMEOUT, "layout.json: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// End the server (as an upgrade's restart does) and the app with it.
fn restart(sandbox: &Sandbox, mut paddock: Paddock) -> Paddock {
    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    let again = sandbox.launch();
    again.wait_for_text("restored 1 space");
    again
}

/// The row of the screen holding `text`, the first from the top.
fn row_of(rows: &[String], text: &str) -> Option<usize> {
    rows.iter().position(|r| r.contains(text))
}

#[test]
fn a_restored_terminal_shows_its_scrollback_above_its_new_prompt() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // The output differs from the command line that made it.
    paddock.type_line("echo hist-$((6*7))");
    paddock.wait_for_text("hist-42");
    wait_for_saved_layout(&sandbox, 1);

    let again = restart(&sandbox, paddock);

    again.wait_for_text(MARK);
    again.wait_for("the new prompt below the mark", |_| {
        let rows = again.full_rows();
        let mark = row_of(&rows, MARK);
        let prompt = rows.iter().rposition(|r| r.contains(PROMPT));
        mark.is_some_and(|m| prompt.is_some_and(|p| p > m))
    });
    let rows = again.full_rows();
    let (output, mark) = (row_of(&rows, "hist-42"), row_of(&rows, MARK));
    assert!(
        output.is_some_and(|o| mark.is_some_and(|m| o < m)),
        "the output above the mark: {rows:#?}"
    );
    let (row, col) = again.find(MARK).unwrap();
    assert!(again.dim(row, col), "the mark is dim");
}

#[test]
fn a_bad_scrollback_file_never_stops_the_restore() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.command(b"|");
    paddock.wait_for("the split", |_| {
        sandbox.state().spaces[0].tabs[0].splits.len() == 2
    });
    paddock.type_line("echo right-$((6*7))");
    paddock.wait_for_text("right-42");
    wait_for_saved_layout(&sandbox, 2);
    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    let files = saved_files(&sandbox);
    assert_eq!(files.len(), 2, "{files:?}");
    // Not UTF-8, and queries the server's emulator would answer into
    // the new shell; the other file is unreadable (a directory).
    let mut junk = b"\xff\xfe junk-line \x1b[c\x1b[6n\x1b]0;title\x07 end\r\n".to_vec();
    junk.extend_from_slice(&[0u8, 0x9b, b'c']);
    std::fs::write(&files[0], &junk).unwrap();
    std::fs::remove_file(&files[1]).unwrap();
    std::fs::create_dir(&files[1]).unwrap();

    let again = sandbox.launch();

    again.wait_for_text("restored 1 space");
    again.wait_for("both splits", |_| {
        sandbox.state().spaces[0].tabs[0].splits.len() == 2
    });
    again.wait_for_text(PROMPT);
    std::thread::sleep(Duration::from_millis(500));
    let screen = again.full_screen();
    assert!(!screen.contains("[?"), "no reply typed: {screen}");
    assert!(!screen.contains("title"), "no OSC shown: {screen}");
}

#[test]
fn closing_a_terminal_removes_its_saved_scrollback() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.command(b"|");
    paddock.wait_for("the split", |_| {
        sandbox.state().spaces[0].tabs[0].splits.len() == 2
    });
    paddock.type_line("echo split");
    wait_for_saved_layout(&sandbox, 2);
    let mut again = restart(&sandbox, paddock);
    wait_for_files(&sandbox, 2);

    again.type_line("exit");

    wait_for_files(&sandbox, 1);
    again.type_line("exit");
    assert!(again.wait_exit());
    let dir = sandbox.state_dir().join("scrollback");
    assert!(!dir.exists(), "removed with the last space");
}
