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
    // The same process, so the same lock, socket, instance and shells.
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
    assert!(alive(server));
    assert_eq!(sandbox.shells(), shells, "the same shells");
    assert_eq!(sandbox.state().instance, instance, "the same instance");
    let terminal = sandbox.terminal("space-1");
    assert_eq!(terminal.command.as_deref(), Some("cat"));
    assert_eq!(terminal.scrollback, scrollback, "the same scrollback");
    // The attached paddock attached again: the same screen, and the
    // program still reads.
    paddock.wait_for_text("programs kept running");
    paddock.wait_for_text("typed-before\ntyped-before\n");
    paddock.type_line("typed-after");
    paddock.wait_for_text("typed-after\ntyped-after\n");
}

#[test]
fn an_attached_paddock_attaches_again_where_it_was() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo CAT''-READY; cat");
    paddock.wait_for_text("CAT-READY\n");
    paddock.type_line("typed-before");
    paddock.wait_for_text("typed-before\ntyped-before\n");
    // A second space, holding a second tab, which is current.
    paddock.command(b"C\r");
    paddock.wait_for_text(PROMPT);
    paddock.command(b"c");
    paddock.wait_for_tabs(&["1:home", "2:home"]);
    paddock.wait_for_current_tab("2:home");
    paddock.type_line("echo in-second-tab");
    paddock.wait_for_text("in-second-tab\n");

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(out.status.success(), "{out:?}");
    paddock.wait_for_text("programs kept running");
    // The same space and tab, and its shell takes input.
    assert_eq!(paddock.current_tab(), "2:home");
    paddock.wait_for_text("in-second-tab\n");
    paddock.type_line("echo after-upgrade");
    paddock.wait_for_text("after-upgrade\n");
    assert_eq!(sandbox.space("space-2").current, 1);
    // The first space's program, there too.
    let first = sandbox.terminal("space-1");
    assert_eq!(first.command.as_deref(), Some("cat"));
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

    let refuser = refuser.to_string_lossy().into_owned();

    let out = sandbox.run(&["server", "upgrade", &refuser], &[], "");

    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!(
            "{refuser} cannot take over: cannot read this record"
        )),
        "{err}"
    );
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
    assert_eq!(sandbox.shells(), shells, "the same shells");
    // The paddock attached all along still works.
    paddock.type_line("echo after-refusal");
    paddock.wait_for_text("after-refusal\n");
}

/// Ask the sandbox's server, past its hello but sending none, to hand
/// over to `exe`; its answers, each an upgrade frame (kind 3).
fn ask_upgrade(sandbox: &Sandbox, exe: &Path) -> Vec<String> {
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    assert!(read_frame(&mut stream).is_some(), "the server's hello");
    let ask = format!(r#"{{"Ask":{{"exe":"{}"}}}}"#, exe.display());
    send_frame(&mut stream, 3, &ask);
    let mut answers = Vec::new();
    while let Some(reply) = read_frame(&mut stream) {
        assert_eq!(reply[0], 3, "an upgrade frame");
        let reply = String::from_utf8_lossy(&reply[1..]).into_owned();
        let last = reply != r#""Probed""#;
        answers.push(reply);
        if last {
            break;
        }
    }
    answers
}

#[test]
fn a_binary_named_by_a_relative_path_is_refused() {
    let sandbox = native();
    let _paddock = ready(&sandbox);
    let server = server_pid(&sandbox.state_dir()).expect("a server");

    let answers = ask_upgrade(&sandbox, Path::new("paddock"));

    assert_eq!(answers.len(), 1, "{answers:?}");
    assert!(answers[0].contains("not an absolute path"), "{answers:?}");
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
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

    let answers = ask_upgrade(&sandbox, &copy);

    let done = format!(r#"{{"Done":{{"pid":{server}}}}}"#);
    assert_eq!(answers, [r#""Probed""#.to_string(), done]);
    let ps = Command::new("ps")
        .args(["-o", "command=", "-p", &server.to_string()])
        .output()
        .unwrap();
    let command = String::from_utf8_lossy(&ps.stdout);
    let runs = format!("{} server --take-over", copy.display());
    assert!(command.starts_with(&runs), "{command}");
    assert_eq!(sandbox.shells(), shells, "the same shells");
    paddock.wait_for_text("programs kept running");
    paddock.wait_for_text("typed-before\ntyped-before\n");
    paddock.type_line("typed-after");
    paddock.wait_for_text("typed-after\ntyped-after\n");
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
fn a_server_that_ends_as_it_takes_over_is_not_called_too_old() {
    let sandbox = native();
    fake_server(&sandbox, Some(r#""Probed""#));

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("ended as it took over"), "{err}");
    assert!(!err.contains("too old"), "{err}");
}

#[test]
fn upgrading_with_no_server_running_says_so() {
    let sandbox = native();

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not running"), "{err}");
}

#[test]
fn a_paddock_with_a_busy_space_attaches_again_once_and_stays() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Bounded, and stopped below: nothing is left running.
    paddock.type_line("for i in $(seq 1 6000); do echo tick-$i; sleep 0.005; done");
    paddock.wait_for_text("tick-5\n");
    paddock.command(b"C\r");
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo in-quiet-space");
    paddock.wait_for_text("in-quiet-space\n");

    let out = sandbox.run(&["server", "upgrade"], &[], "");

    assert!(out.status.success(), "{out:?}");
    paddock.wait_for_text("programs kept running");
    paddock.type_line("echo after-upgrade");
    paddock.wait_for_text("after-upgrade\n");
    // The busy space, attached again, still streams.
    paddock.command(b"\t");
    let ticks = |screen: &str| {
        screen
            .lines()
            .filter_map(|l| l.trim().strip_prefix("tick-")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
    };
    let seen = ticks(&paddock.screen());
    paddock.wait_for("more ticks", |s| ticks(s) > seen + 20);
    paddock.send(b"\x03");
    paddock.type_line("echo loop''-stopped");
    paddock.wait_for_text("loop-stopped\n");
    // Still attached, and only once: a second attach, or a lost
    // connection, would have ended it.
    paddock.type_line("echo still-here");
    paddock.wait_for_text("still-here\n");
    let shells = sandbox.shells();
    assert_eq!(shells.len(), 2, "{shells:?}");
}
