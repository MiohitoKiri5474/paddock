//! Ticket 50: a running server hands its terminals to a new binary
//! (ADR 0009), so programs keep running across an upgrade. Every test
//! here picks the native backend in its config, so it runs on either run
//! of the suite.

mod harness;

use harness::backend::{alive, server_pid, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::time::Duration;

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

/// Write one frame of `kind` holding `json`, as the protocol does.
fn send_frame(stream: &mut UnixStream, kind: u8, json: &str) {
    let len = u32::try_from(json.len() + 1).unwrap().to_be_bytes();
    stream.write_all(&len).unwrap();
    stream.write_all(&[kind]).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
}

/// The next frame's body, its kind first; `None` once the server closed
/// the connection.
fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0; 4];
    stream.read_exact(&mut len).ok()?;
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

#[test]
fn a_program_started_before_an_upgrade_is_still_running_with_its_screen_after_it() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Enough lines to leave some in the scrollback, then a program that
    // is not a shell, waiting for input.
    paddock.type_line("seq 1 60; cat");
    paddock.wait_for_text("\n60\n");
    paddock.type_line("typed-before");
    paddock.wait_for_text("typed-before\ntyped-before\n");
    let server = server_pid(&sandbox.state_dir()).expect("a server");
    let shells = sandbox.shells();
    let scrollback = sandbox.terminal("space-1").scrollback;
    assert!(scrollback.is_some_and(|n| n > 0), "{scrollback:?}");
    let instance = sandbox.state().instance;

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains(&format!("pid {server}")), "{said}");
    let log = std::fs::read_to_string(sandbox.state_dir().join("server.log")).unwrap();
    assert!(log.contains(&format!("took over as pid {server}")), "{log}");
    // The attached paddock lost its connection at the switch.
    assert!(!paddock.wait_exit(), "{}", paddock.full_screen());
    // The same process, so the same lock, socket, instance and shells.
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
    assert!(alive(server));
    assert_eq!(sandbox.shells(), shells, "the same shells");
    assert_eq!(sandbox.state().instance, instance, "the same instance");
    let terminal = sandbox.terminal("space-1");
    assert_eq!(terminal.command.as_deref(), Some("cat"));
    assert_eq!(terminal.scrollback, scrollback, "the same scrollback");
    // A new paddock sees the same screen, and the program still reads.
    let mut again = sandbox.launch();
    again.wait_for_text("typed-before\ntyped-before\n");
    again.type_line("typed-after");
    again.wait_for_text("typed-after\ntyped-after\n");
}

#[test]
fn a_handover_the_new_binary_refuses_leaves_the_old_server_serving() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo before-refusal");
    paddock.wait_for_text("before-refusal\n");
    let server = server_pid(&sandbox.state_dir()).expect("a server");
    let shells = sandbox.shells();
    // A new binary that cannot read the record, as one of another format.
    let refuser = sandbox.path("refuser");
    std::fs::write(
        &refuser,
        "#!/bin/sh\ncat >/dev/null\necho 'cannot read this record' >&2\nexit 3\n",
    )
    .unwrap();
    std::fs::set_permissions(&refuser, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Asked whatever protocol version the server speaks: no hello sent.
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    assert!(read_frame(&mut stream).is_some(), "the server's hello");
    let ask = format!(r#"{{"Ask":{{"exe":"{}"}}}}"#, refuser.display());
    send_frame(&mut stream, 3, &ask);
    let reply = read_frame(&mut stream).expect("an answer");

    assert_eq!(reply[0], 3, "an upgrade frame");
    let reply = String::from_utf8_lossy(&reply[1..]).into_owned();
    assert!(reply.contains("Refused"), "{reply}");
    assert!(reply.contains("cannot read this record"), "{reply}");
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
    assert_eq!(sandbox.shells(), shells, "the same shells");
    // The paddock attached all along still works.
    paddock.type_line("echo after-refusal");
    paddock.wait_for_text("after-refusal\n");
}

#[test]
fn upgrading_with_no_server_running_says_so() {
    let sandbox = native();

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not running"), "{err}");
}
