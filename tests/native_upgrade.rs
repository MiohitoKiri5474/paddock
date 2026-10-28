//! Ticket 50: a running server hands its terminals to a new binary
//! (ADR 0009), so programs keep running across an upgrade. Every test
//! here picks the native backend in its config, so it runs on either run
//! of the suite.

mod harness;

use harness::backend::{alive, server_pid, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
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
    paddock.wait_for("the farewell", |_| {
        paddock
            .full_screen()
            .contains("lost the connection to the paddock server")
    });
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
    let reply = ask_upgrade(&sandbox, &refuser);

    assert!(reply.contains("Refused"), "{reply}");
    assert!(reply.contains("cannot read this record"), "{reply}");
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
    assert_eq!(sandbox.shells(), shells, "the same shells");
    // The paddock attached all along still works.
    paddock.type_line("echo after-refusal");
    paddock.wait_for_text("after-refusal\n");
}

/// Ask the sandbox's server, past its hello, to hand over to `exe`; its
/// answer, kind byte first.
fn ask_upgrade(sandbox: &Sandbox, exe: &Path) -> String {
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    assert!(read_frame(&mut stream).is_some(), "the server's hello");
    let ask = format!(r#"{{"Ask":{{"exe":"{}"}}}}"#, exe.display());
    send_frame(&mut stream, 3, &ask);
    let reply = read_frame(&mut stream).expect("an answer");
    assert_eq!(reply[0], 3, "an upgrade frame");
    String::from_utf8_lossy(&reply[1..]).into_owned()
}

#[test]
fn another_binary_takes_the_terminals_over() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Typed once the shell runs the line, not while it still reads it.
    paddock.type_line("echo CAT''-READY; cat");
    paddock.wait_for_text("CAT-READY\n");
    paddock.type_line("typed-before");
    paddock.wait_for_text("typed-before\ntyped-before\n");
    let server = server_pid(&sandbox.state_dir()).expect("a server");
    let shells = sandbox.shells();
    // A copy, not a link: another binary, as an upgrade installs.
    let copy = sandbox.path("paddock-next");
    std::fs::copy(env!("CARGO_BIN_EXE_paddock"), &copy).unwrap();

    let reply = ask_upgrade(&sandbox, &copy);

    assert_eq!(reply, format!(r#"{{"Done":{{"pid":{server}}}}}"#));
    let ps = Command::new("ps")
        .args(["-o", "command=", "-p", &server.to_string()])
        .output()
        .unwrap();
    let command = String::from_utf8_lossy(&ps.stdout);
    let runs = format!("{} server --take-over", copy.display());
    assert!(command.starts_with(&runs), "{command}");
    assert_eq!(sandbox.shells(), shells, "the same shells");
    let mut again = sandbox.launch();
    again.wait_for_text("typed-before\ntyped-before\n");
    again.type_line("typed-after");
    again.wait_for_text("typed-after\ntyped-after\n");
}

/// Listen as a server of protocol version 999 would: send its hello,
/// read one frame, and answer it with `answer` (a kind-3 body), or close
/// the connection if `None`, as a server from before upgrades does at a
/// frame it does not know.
fn fake_server(sandbox: &Sandbox, answer: Option<&'static str>) {
    std::fs::create_dir_all(sandbox.state_dir()).unwrap();
    let listener = UnixListener::bind(sandbox.state_dir().join("server.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            send_frame(&mut stream, 0, r#"{"Hello":{"version":999,"pid":4242}}"#);
            if read_frame(&mut stream).is_none() {
                continue;
            }
            if let Some(answer) = answer {
                send_frame(&mut stream, 3, answer);
            }
        }
    });
}

#[test]
fn an_upgrade_is_asked_of_a_server_speaking_another_protocol_version() {
    let sandbox = native();
    fake_server(&sandbox, Some(r#"{"Done":{"pid":4242}}"#));

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("pid 4242"), "{said}");
}

#[test]
fn a_server_too_old_to_hand_over_says_to_restart_it() {
    let sandbox = native();
    fake_server(&sandbox, None);

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("(pid 4242) gave no answer"), "{err}");
    assert!(err.contains("restart it"), "{err}");
}

#[test]
fn upgrading_with_no_server_running_says_so() {
    let sandbox = native();

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not running"), "{err}");
}
