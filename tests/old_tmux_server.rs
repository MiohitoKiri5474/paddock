//! Ticket 45: on the native backend, a tmux server an older paddock left
//! running is said once (per server) not to be moved, with how to close
//! it. The server here is the sandbox's own (its `TMUX_TMPDIR` and
//! socket), never the user's. Every test picks the native backend in its
//! config, so it runs on either run of the suite.

mod harness;

use harness::backend::NATIVE;
use harness::{Paddock, Sandbox, PROMPT};

/// The start of the notice, as the tab bar shows it (cut to its width,
/// after the default keys' warning).
const NOTICE: &str = "an older paddock";
/// Its end, printed whole on leaving: the sandbox's socket.
const CLOSE: &str = "Close it with: tmux -L paddock-test kill-server";

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

fn leave(mut paddock: Paddock) -> Paddock {
    paddock.detach_keys();
    assert!(paddock.wait_exit(), "paddock should detach cleanly");
    paddock
}

#[test]
fn an_old_tmux_server_is_said_once_with_how_to_close_it() {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);
    sandbox.old_tmux_server(true);

    let paddock = ready(&sandbox);
    paddock.wait_for_text(NOTICE);
    let paddock = leave(paddock);
    // Printed whole, maybe wrapped: the whole screen, its rows joined.
    paddock.wait_for(CLOSE, |_| paddock.full_screen().contains(CLOSE));

    // The same server again: nothing said.
    let paddock = ready(&sandbox);
    let screen = paddock.screen();
    assert!(!screen.contains(NOTICE), "{screen}");
    leave(paddock);

    // Another server where it was: said again.
    sandbox.old_tmux_server(false);
    sandbox.old_tmux_server(true);
    let paddock = ready(&sandbox);
    paddock.wait_for_text(NOTICE);
    leave(paddock);
}

#[test]
fn no_old_tmux_server_no_notice() {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);

    let paddock = ready(&sandbox);
    let screen = paddock.screen();
    assert!(!screen.contains(NOTICE), "{screen}");
}
