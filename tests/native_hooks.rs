//! Ticket 43: the native server tells a process in its terminals which
//! terminal it runs in and which event log is its server's. Every test here
//! picks the native backend in its config, so it runs on either run of the
//! suite.

mod harness;

use harness::backend::NATIVE;
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;

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

/// Ticket 41: a split's shell and a new tab's carry their own terminal,
/// not the one they were made from.
#[test]
fn splits_and_new_tabs_carry_their_own_terminal_for_hooks() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let first = sandbox.terminal("space-1").hook_env["PADDOCK_PANE"].clone();

    paddock.command(b"|");
    paddock.wait_for("the split", |_| {
        sandbox.state().spaces[0].tabs[0].splits.len() == 2
    });
    let split = sandbox.terminal("space-1:0.1");
    assert_eq!(split.id, split.hook_env["PADDOCK_PANE"]);
    assert_ne!(split.hook_env["PADDOCK_PANE"], first);
    paddock.type_line("echo split-pane=$PADDOCK_PANE");
    paddock.wait_for_text(&format!("split-pane={}\n", split.id));

    paddock.command(b"c");
    paddock.wait_for("the new tab", |_| sandbox.state().spaces[0].tabs.len() == 2);
    let tab = sandbox.terminal("space-1:1");
    assert_eq!(tab.id, tab.hook_env["PADDOCK_PANE"]);
    paddock.type_line("echo tab-pane=$PADDOCK_PANE");
    paddock.wait_for_text(&format!("tab-pane={}\n", tab.id));
}

/// Ticket 42: a float's program and a second space's shell carry their
/// own terminal too.
#[test]
fn floats_and_a_second_space_carry_their_own_terminal_for_hooks() {
    let sandbox = Sandbox::new();
    sandbox.write_config(&format!("{NATIVE}[floats]\nG = \"sh\"\n"));
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    paddock.command(b"G");
    paddock.wait_for("the float", |_| sandbox.state().spaces[0].floats.len() == 1);
    let float = sandbox.state().spaces[0].floats[0].splits[0].clone();
    let float = sandbox.terminal(&float.id);
    assert_eq!(float.id, float.hook_env["PADDOCK_PANE"]);
    paddock.type_line("echo float-pane=$PADDOCK_PANE");
    let text = format!("float-pane={}", float.id);
    paddock.wait_for(&text, |_| paddock.full_screen().contains(&text));
    paddock.command(b"G");

    paddock.command(b"C\r");
    paddock.wait_for("the second space", |_| sandbox.state().spaces.len() == 2);
    let second = sandbox.terminal("space-2");
    assert_eq!(second.id, second.hook_env["PADDOCK_PANE"]);
    assert_ne!(second.id, float.id);
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo space-pane=$PADDOCK_PANE");
    paddock.wait_for_text(&format!("space-pane={}\n", second.id));
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

/// Listen as a server whose hello gives no instance id would: it answers
/// every hello with its own, version 1, pid 4242 and no `instance`.
fn server_without_an_instance(sandbox: &Sandbox) {
    std::fs::create_dir_all(sandbox.state_dir()).unwrap();
    let socket = sandbox.state_dir().join("server.sock");
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut len = [0; 4];
            if stream.read_exact(&mut len).is_err() {
                continue;
            }
            let mut body = vec![0; u32::from_be_bytes(len) as usize];
            let _ = stream.read_exact(&mut body);
            let hello = br#"{"Hello":{"version":1,"pid":4242}}"#;
            let len = u32::try_from(hello.len() + 1).unwrap().to_be_bytes();
            let _ = stream.write_all(&len);
            let _ = stream.write_all(&[0]);
            let _ = stream.write_all(hello);
        }
    });
}

/// No event log is named after an instance id of 0: debug state fails
/// saying so, and prints no state.
#[test]
fn debug_state_fails_at_a_server_that_gives_no_instance_id() {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);
    server_without_an_instance(&sandbox);

    let out = sandbox.run(&["debug", "state", "--json"], &[], "");

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("gave no instance id"), "{err}");
    assert!(err.contains("pid 4242"), "{err}");
    assert!(out.stdout.is_empty(), "no state printed");
}
