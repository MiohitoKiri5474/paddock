//! Ticket 05: one paddock at a time; a second launch offers to take over.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};

const TAKE_OVER: &str = "Take over?";

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

#[test]
fn accepting_take_over_closes_the_first_and_keeps_its_terminals() {
    let sandbox = Sandbox::new();
    let mut first = ready(&sandbox);
    first.command(b"c");
    first.wait_for_tabs(&["1:home", "2:home"]);
    first.wait_for_text(PROMPT);
    first.type_line("echo marker-before-take-over");
    first.wait_for_text("marker-before-take-over\n");
    let before = sandbox.shells();

    let mut second = sandbox.launch();
    second.wait_for_text(TAKE_OVER);
    second.type_line("y");

    assert!(first.wait_exit(), "the first paddock detaches cleanly");
    second.wait_for_tabs(&["1:home", "2:home"]);
    assert_eq!(
        sandbox.shells(),
        before,
        "the same shells are still running"
    );
    second.command(b"2");
    second.wait_for_text("marker-before-take-over\n");
    second.type_line("echo after-take-over");
    second.wait_for_text("after-take-over\n");
}

#[test]
fn declining_take_over_leaves_the_first_running() {
    let sandbox = Sandbox::new();
    let mut first = ready(&sandbox);

    let mut second = sandbox.launch();
    second.wait_for_text(TAKE_OVER);
    second.type_line("n");

    assert!(second.wait_exit(), "declining is not an error");
    first.type_line("echo still-here");
    first.wait_for_text("still-here\n");
    first.wait_for_tabs(&["1:home"]);
}

#[test]
fn a_lock_left_by_a_dead_paddock_is_ignored() {
    let sandbox = Sandbox::new();
    let mut gone = std::process::Command::new("true").spawn().expect("spawn");
    gone.wait().expect("wait");
    let dir = sandbox.path("state/paddock");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(dir.join("paddock.pid"), format!("{}\n", gone.id())).expect("lock");

    let paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]);
    assert!(!paddock.screen().contains(TAKE_OVER));
}
