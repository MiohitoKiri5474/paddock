//! Ticket 45: on the native backend, a tmux server an older paddock left
//! running is said once (per server) not to be moved, with how to close
//! it. The server here is the sandbox's own (its `TMUX_TMPDIR` and
//! socket), never the user's. Every test picks the native backend in its
//! config, so it runs on either run of the suite.

mod harness;

use harness::backend::NATIVE;
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;

/// The start of the notice, as the tab bar shows it first (cut to its
/// width, before the default keys' warning).
const NOTICE: &str = "an older paddock's tmux server still runs";
/// Its end, printed whole on leaving: the sandbox's socket.
const CLOSE: &str = "Close it with: tmux -L paddock-test kill-server";

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Wait until the notice's end is printed, on leaving.
fn wait_printed(paddock: &Paddock) {
    // Printed whole, maybe wrapped: the whole screen, its rows joined.
    paddock.wait_for(CLOSE, |_| paddock.full_screen().contains(CLOSE));
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
    sandbox.start_old_tmux_server();

    let paddock = ready(&sandbox);
    paddock.wait_for_text(NOTICE);
    let paddock = leave(paddock);
    wait_printed(&paddock);

    // The same server again: nothing said.
    let paddock = ready(&sandbox);
    let screen = paddock.screen();
    assert!(!screen.contains(NOTICE), "{screen}");
    leave(paddock);

    // Another server where it was: said again.
    sandbox.kill_old_tmux_server();
    sandbox.start_old_tmux_server();
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

/// Listen as a native server of another protocol version would: it reads
/// paddock's hello and answers with its own, version 999 (as
/// `tests/native_server.rs` does).
fn fake_older_server(sandbox: &Sandbox) {
    std::fs::create_dir_all(sandbox.state_dir()).unwrap();
    let listener = UnixListener::bind(sandbox.state_dir().join("server.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut len = [0; 4];
            if stream.read_exact(&mut len).is_err() {
                continue;
            }
            let mut body = vec![0; u32::from_be_bytes(len) as usize];
            let _ = stream.read_exact(&mut body);
            let hello = br#"{"Hello":{"version":999,"pid":4242}}"#;
            let len = u32::try_from(hello.len() + 1).unwrap().to_be_bytes();
            let _ = stream.write_all(&len);
            let _ = stream.write_all(&[0]);
            let _ = stream.write_all(hello);
        }
    });
}

#[test]
fn a_start_that_fails_after_finding_the_old_server_still_says_so() {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);
    sandbox.start_old_tmux_server();
    fake_older_server(&sandbox);

    // Declining the restart stops paddock before it draws anything.
    let mut paddock = sandbox.launch();
    paddock.wait_for_text("Restart it?");
    paddock.type_line("n");
    assert!(!paddock.wait_exit(), "declining fails");
    wait_printed(&paddock);
}
