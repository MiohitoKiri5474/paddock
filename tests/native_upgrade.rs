//! Ticket 50: a running server hands its terminals to a new binary
//! (ADR 0009), so programs keep running across an upgrade. Every test
//! here picks the native backend in its config, so it runs on either run
//! of the suite.

mod harness;

use harness::backend::{alive, server_pid, NATIVE};
use harness::older_server::{read_frame, send_frame};
use harness::{Paddock, Sandbox, PROMPT};
use std::io::Write;
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    send_frame(&mut stream, 3, &ask).unwrap();
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
    let copy = sandbox.copy_paddock("paddock-next");

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
            let _ = send_frame(&mut stream, 0, r#"{"Hello":{"version":999,"pid":4242}}"#);
            if read_frame(&mut stream).is_none() {
                continue;
            }
            // A client that only checked the hello may be gone already.
            if let Some(answer) = answer {
                let _ = send_frame(&mut stream, 3, answer);
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
    // Bounded, and stopped below: nothing is left running. Stopped by a
    // file, which a builtin checks, not by Ctrl-C: while the terminal's
    // output waits to be read (a loaded machine), zsh now and then lets a
    // Ctrl-C go by and runs this loop on, in a bare terminal too.
    let stop = sandbox.path("stop-ticking");
    paddock.type_line(&format!(
        "for i in $(seq 1 6000); do [ -e '{}' ] && break; echo tick-$i; sleep 0.005; done; \
         echo loop''-stopped",
        stop.display()
    ));
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
    std::fs::write(&stop, "").unwrap();
    paddock.wait_for_text("loop-stopped\n");
    // Still attached, and only once: a second attach, or a lost
    // connection, would have ended it.
    paddock.type_line("echo still-here");
    paddock.wait_for_text("still-here\n");
    let shells = sandbox.shells();
    assert_eq!(shells.len(), 2, "{shells:?}");
}

// Ticket 51: a paddock that finds a server of another protocol version
// offers to upgrade it in place, and restarting it is the fallback.

/// What the proxy of [`disguise_as_another_version`] changes, besides the
/// version in the server's hello before the upgrade.
#[derive(Clone, Copy, Default)]
struct Disguise {
    /// Close the asking connection in place of passing `Done` on, as if
    /// the answer were lost.
    lose_done: bool,
    /// After the upgrade, the pid the server's hello says, as if another
    /// server had started meanwhile.
    pid_after: Option<u32>,
}

/// Stand between paddock and the sandbox's real server as if it spoke
/// another protocol version: move its socket aside, listen in its place,
/// and pass every byte on both ways, except that the server's hello says
/// version 999 until the server answers an upgrade with `Done`, and what
/// `disguise` says. Only that is faked: the server, its programs and the
/// handover to this build's binary are real. (One build speaks one
/// version, so two real versions cannot meet here.)
fn disguise_as_another_version(sandbox: &Sandbox, disguise: Disguise) {
    let socket = sandbox.state_dir().join("server.sock");
    let real = sandbox.state_dir().join("real.sock");
    // The server's listener keeps its socket, whatever it is named.
    std::fs::rename(&socket, &real).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    let upgraded = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let Ok(server) = UnixStream::connect(&real) else {
                return;
            };
            let (mut from_client, mut to_server) =
                (client.try_clone().unwrap(), server.try_clone().unwrap());
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut from_client, &mut to_server);
                let _ = to_server.shutdown(Shutdown::Write);
            });
            let upgraded = Arc::clone(&upgraded);
            std::thread::spawn(move || pass_on(server, client, &upgraded, disguise));
        }
    });
}

/// Pass the server's frames on to the client, its hello changed as
/// [`disguise_as_another_version`] says, `upgraded` set by a `Done`.
fn pass_on(
    mut server: UnixStream,
    mut client: UnixStream,
    upgraded: &AtomicBool,
    disguise: Disguise,
) {
    let mut first = true;
    while let Some(mut body) = read_frame(&mut server) {
        if first {
            body = disguised_hello(&body, upgraded.load(Ordering::SeqCst), disguise);
        }
        first = false;
        if body[0] == 3 && body[1..].starts_with(br#"{"Done""#) {
            upgraded.store(true, Ordering::SeqCst);
            if disguise.lose_done {
                break;
            }
        }
        let len = u32::try_from(body.len()).unwrap().to_be_bytes();
        if client
            .write_all(&len)
            .and_then(|()| client.write_all(&body))
            .is_err()
        {
            break;
        }
    }
    let _ = client.shutdown(Shutdown::Both);
}

/// A hello frame's body (kind 0): version 999 before the upgrade, and
/// after it the pid `disguise` says, if any.
fn disguised_hello(body: &[u8], upgraded: bool, disguise: Disguise) -> Vec<u8> {
    assert_eq!(body[0], 0, "a hello first");
    let mut hello: serde_json::Value = serde_json::from_slice(&body[1..]).unwrap();
    match (upgraded, disguise.pid_after) {
        (false, _) => hello["Hello"]["version"] = 999.into(),
        (true, Some(pid)) => hello["Hello"]["pid"] = pid.into(),
        (true, None) => return body.to_vec(),
    }
    let mut out = vec![0];
    out.extend(serde_json::to_vec(&hello).unwrap());
    out
}

/// A paddock running `cat`, which has read a line, detached from its
/// server, which then seems to speak another protocol version as
/// `disguise` says: the server's pid and shells.
fn cat_behind_another_version(sandbox: &Sandbox, disguise: Disguise) -> (i32, Vec<String>) {
    let mut first = ready(sandbox);
    // Typed once the shell runs the line, not while it still reads it.
    first.type_line("echo CAT''-READY; cat");
    first.wait_for_text("CAT-READY\n");
    first.type_line("typed-before");
    first.wait_for_text("typed-before\ntyped-before\n");
    let server = server_pid(&sandbox.state_dir()).expect("a server");
    let shells = sandbox.shells();
    first.detach_keys();
    assert!(first.wait_exit(), "detached");
    disguise_as_another_version(sandbox, disguise);
    (server, shells)
}

/// The upgraded server's programs run on: the same pid, shells and
/// `cat`, which still reads, on the screen it had.
fn assert_programs_kept(sandbox: &Sandbox, paddock: &mut Paddock, server: i32, shells: &[String]) {
    paddock.wait_for_text("typed-before\ntyped-before\n");
    paddock.type_line("typed-after");
    paddock.wait_for_text("typed-after\ntyped-after\n");
    let log = std::fs::read_to_string(sandbox.state_dir().join("server.log")).unwrap();
    assert!(log.contains(&format!("took over as pid {server}")), "{log}");
    assert_eq!(server_pid(&sandbox.state_dir()), Some(server));
    assert!(alive(server));
    assert_eq!(sandbox.shells(), shells, "the same shells");
    let terminal = sandbox.terminal("space-1");
    assert_eq!(terminal.command.as_deref(), Some("cat"));
}

#[test]
fn a_server_of_another_version_is_upgraded_and_its_programs_keep_running() {
    let sandbox = native();
    let (server, shells) = cat_behind_another_version(&sandbox, Disguise::default());

    let mut paddock = sandbox.launch();
    paddock.wait_for_text("speaks protocol version 999");
    paddock.wait_for_text("Upgrade it? Its programs keep running. [Y/n]");
    // Yes is the default.
    paddock.type_line("");

    paddock.wait_for_text("programs kept running");
    assert_programs_kept(&sandbox, &mut paddock, server, &shells);
}

/// The switch happened, but paddock never read `Done`: the server, at
/// this version with the same pid, tells it.
#[test]
fn an_upgrade_whose_done_is_lost_is_done_if_the_same_server_answers() {
    let sandbox = native();
    let lost = Disguise {
        lose_done: true,
        ..Disguise::default()
    };
    let (server, shells) = cat_behind_another_version(&sandbox, lost);

    let mut paddock = sandbox.launch();
    paddock.wait_for_text("Upgrade it?");
    paddock.type_line("y");

    paddock.wait_for_text("programs kept running");
    assert!(!paddock.full_screen().contains("Restart it?"));
    assert_programs_kept(&sandbox, &mut paddock, server, &shells);
}

/// The answer lost, and another server answers (one started from
/// `layout.json` meanwhile): the old one ended, its programs with it.
#[test]
fn an_upgrade_whose_done_is_lost_has_ended_if_another_server_answers() {
    let sandbox = native();
    let elsewhere = Disguise {
        lose_done: true,
        pid_after: Some(4243),
    };
    cat_behind_another_version(&sandbox, elsewhere);

    let mut paddock = sandbox.launch();
    paddock.wait_for_text("Upgrade it?");
    paddock.type_line("y");

    paddock.wait_for_text("its programs ended");
    assert!(!paddock.full_screen().contains("Restart it?"));
}

#[test]
fn a_server_too_old_to_hand_over_falls_back_to_the_restart_prompt() {
    let sandbox = native();
    fake_server(&sandbox, None);

    let mut paddock = sandbox.launch();
    paddock.wait_for_text("Upgrade it? Its programs keep running. [Y/n]");
    paddock.type_line("y");

    paddock.wait_for_text("too old to hand its terminals over");
    paddock.wait_for_text("Restart it? Its programs end; its layout comes back. [y/N]");
    paddock.type_line("n");
    assert!(!paddock.wait_exit(), "declining fails");
}

#[test]
fn a_server_refusing_before_the_switch_falls_back_to_the_restart_prompt() {
    let sandbox = native();
    fake_server(&sandbox, Some(r#"{"Refused":"cannot read this record"}"#));

    let mut paddock = sandbox.launch();
    paddock.wait_for_text("Upgrade it?");
    paddock.type_line("y");

    paddock.wait_for_text("cannot read this record");
    paddock.wait_for_text("Restart it?");
    paddock.type_line("n");
    assert!(!paddock.wait_exit(), "declining fails");
}
