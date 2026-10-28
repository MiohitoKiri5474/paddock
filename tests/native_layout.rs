//! Ticket 44: the native server saves its layout to `layout.json` and a
//! new server rebuilds it (ADR 0008), after it was ended or restarted for
//! another protocol version (ADR 0007). Every test here picks the native
//! backend in its config, so it runs on either run of the suite.

mod harness;

use harness::backend::{alive, server_pid, stop_server, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::{Duration, Instant};

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

/// `path` as the server reads a directory: symlinks resolved.
fn real(path: &Path) -> String {
    let real = std::fs::canonicalize(path).unwrap();
    real.to_string_lossy().into_owned()
}

/// Every space's shape, as `debug state` tells it: names, tabs, focus,
/// zoom, each split's place and directory, with `gone` read as `home`.
fn shape(sandbox: &Sandbox, gone: &str, home: &str) -> Vec<String> {
    let state = sandbox.state();
    let mut lines = Vec::new();
    for space in &state.spaces {
        lines.push(format!(
            "space {} current tab {}",
            space.name, space.current
        ));
        for tab in &space.tabs {
            lines.push(format!(
                "  tab {} auto {} zoomed {}",
                tab.name, tab.auto_name, tab.zoomed
            ));
            for s in &tab.splits {
                let dir = s.dir.clone().unwrap_or_default();
                let dir = if dir == gone { home.to_string() } else { dir };
                lines.push(format!(
                    "    {}x{} at {},{} focused {} in {dir}",
                    s.width, s.height, s.x, s.y, s.focused
                ));
            }
        }
    }
    lines
}

/// The saved layout, once it holds `spaces` spaces and names `text`.
fn wait_for_saved(sandbox: &Sandbox, spaces: usize, text: &str) {
    let path = sandbox.state_dir().join("layout.json");
    let start = Instant::now();
    loop {
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&saved).unwrap_or_default();
        let count = json["spaces"].as_array().map_or(0, Vec::len);
        if count == spaces && saved.contains(text) {
            return;
        }
        assert!(start.elapsed() < harness::TIMEOUT, "layout.json: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A space with a renamed tab split in two, its second split in `dir`.
fn split_in(paddock: &mut Paddock, sandbox: &Sandbox, dir: &Path) {
    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "");
    paddock.send(b"\x15logs\r");
    paddock.wait_for_tabs(&["1:logs"]);
    paddock.command(b"|");
    paddock.wait_for("the split", |_| {
        sandbox.state().spaces[0].tabs[0].splits.len() == 2
    });
    paddock.type_line(&format!("cd '{}'", dir.display()));
    let real = real(dir);
    paddock.wait_for("the cd", |_| {
        sandbox.terminal("space-1:0.1").dir.as_deref() == Some(real.as_str())
    });
}

#[test]
fn a_new_server_rebuilds_the_layout_with_each_shell_in_its_directory() {
    let sandbox = native();
    let (proj, gone) = (sandbox.path("home/proj"), sandbox.path("home/gone"));
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&gone).unwrap();
    let mut paddock = ready(&sandbox);
    split_in(&mut paddock, &sandbox, &gone);
    paddock.command(b"c");
    paddock.wait_for("the second tab", |_| {
        sandbox.state().spaces[0].tabs.len() == 2
    });
    paddock.type_line(&format!("cd '{}'", proj.display()));
    paddock.wait_for_tabs(&["1:logs", "2:proj"]);
    paddock.command(b"C\r");
    paddock.wait_for("the second space", |_| sandbox.state().spaces.len() == 2);
    wait_for_saved(&sandbox, 2, "/proj\"");
    let (gone_dir, home) = (real(&gone), real(&sandbox.path("home")));
    let before = shape(&sandbox, &gone_dir, &home);
    let instance = sandbox.state().instance;

    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    std::fs::remove_dir(&gone).unwrap();
    let again = sandbox.launch();

    again.wait_for_text("restored 2 spaces");
    assert_eq!(shape(&sandbox, &gone_dir, &home), before);
    assert_eq!(
        sandbox.terminal("space-1:0.1").dir.as_deref(),
        Some(home.as_str()),
        "a directory gone: $HOME"
    );
    assert_ne!(sandbox.state().instance, instance, "a new server");
    let tabs = again.tab_bar();
    assert!(!tabs.contains("logs"), "space-2 stays current: {tabs}");
}

#[test]
fn a_server_ended_with_its_last_shell_leaves_nothing_to_restore() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    wait_for_saved(&sandbox, 1, "space-1");

    paddock.type_line("exit");
    assert!(paddock.wait_exit());

    let path = sandbox.state_dir().join("layout.json");
    assert!(!path.exists(), "removed with the last space");
    let again = ready(&sandbox);
    again.wait_for_tabs(&["1:home"]);
    assert!(!again.full_screen().contains("restored"));
    assert_eq!(sandbox.space_names(), ["space-1"]);
}

/// Listen as an older server would: it answers every hello with its
/// own, version 999 and pid 4242, and holds no lock (its process gone,
/// its socket left).
fn fake_older_server(sandbox: &Sandbox) {
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
fn restarting_a_server_of_another_version_restores_its_layout() {
    let sandbox = native();
    let dir = sandbox.path("home/proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut paddock = ready(&sandbox);
    split_in(&mut paddock, &sandbox, &dir);
    wait_for_saved(&sandbox, 1, "/proj\"");
    let real_dir = real(&dir);
    let before = shape(&sandbox, "", "");
    let pid = server_pid(&sandbox.state_dir()).expect("a server");
    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit());
    assert!(!alive(pid));
    fake_older_server(&sandbox);

    let mut again = sandbox.launch();
    again.wait_for_text("Restart it?");
    assert!(again.screen().contains("speaks protocol version 999"));
    again.type_line("y");

    again.wait_for_text("restored 1 space");
    assert_eq!(shape(&sandbox, "", ""), before);
    assert_eq!(
        sandbox.terminal("space-1:0.1").dir.as_deref(),
        Some(real_dir.as_str())
    );
    assert!(server_pid(&sandbox.state_dir()).is_some_and(alive));
}
