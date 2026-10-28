//! Ticket 43: the native server tells a process in its terminals which
//! terminal it runs in and which event log is its server's. Every test here
//! picks the native backend in its config, so it runs on either run of the
//! suite.

mod harness;

use harness::backend::NATIVE;
use harness::{Paddock, Sandbox, PROMPT};

fn ready(sandbox: &Sandbox) -> Paddock {
    sandbox.write_config(NATIVE);
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// A hook in a terminal finds it, and the server's event log, from the
/// variables the shell really has, which debug state reports.
#[test]
fn shells_carry_their_terminal_and_server_for_hooks() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let state = sandbox.state();
    let instance = state.instance.expect("the server's instance id");
    let env = &sandbox.terminal("space-1").hook_env;
    let socket = sandbox.state_dir().join("server.sock");
    assert_eq!(env["PADDOCK_SOCKET"], socket.to_string_lossy());
    assert_eq!(env["PADDOCK_INSTANCE"], instance.to_string());
    paddock.type_line(&format!(
        "[ \"$PADDOCK_SOCKET\" = '{}' ] && echo pane=$PADDOCK_PANE-instance=$PADDOCK_INSTANCE",
        env["PADDOCK_SOCKET"]
    ));
    paddock.wait_for_text(&format!(
        "pane={}-instance={instance}\n",
        env["PADDOCK_PANE"]
    ));
}

/// The server's event log is named apart from tmux's, so a tmux server
/// whose pid equals its instance id never reads it.
#[test]
fn the_servers_event_log_is_apart_from_tmuxs() {
    let sandbox = Sandbox::new();
    let _paddock = ready(&sandbox);
    let state = sandbox.state();
    let instance = state.instance.expect("the server's instance id");
    assert_ne!(instance, 0, "0 names no server");
    let log = format!("events/native-{instance}.jsonl");
    let log = sandbox.state_dir().join(log).to_string_lossy().into_owned();
    assert_eq!(state.event_log, Some(log));
}
