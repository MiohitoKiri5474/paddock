//! Ticket 53: the desktop tools. Notifications go through `osascript` on
//! macOS and `notify-send` on Linux; copying through `pbcopy` on macOS,
//! `wl-copy` on Wayland and `xclip` on X11, unless `PADDOCK_COPY_COMMAND`
//! names another. The sandbox's stubs stand in for all of them (first on
//! `PATH`), so no test notifies or copies for real. A tool that cannot be
//! run is said once in the tab bar, and paddock goes on.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const CANNOT_NOTIFY: &str = "cannot notify: cannot run";
const CANNOT_COPY: &str = "cannot copy: cannot run";
const COPY: &str = " COPY  line";

fn payload(name: &str) -> String {
    let path = if name.starts_with("PermissionRequest") {
        format!("{OURS}/{name}.json")
    } else {
        format!("{SPIKE}/{name}.json")
    };
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Run hooks in order in the first tab's terminal.
fn hooks(sandbox: &Sandbox, names: &[&str]) {
    for name in names {
        sandbox.hook("space-1:0", name, &payload(name));
    }
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// A second tab, so the first one's session notifies.
fn look_away(paddock: &mut Paddock) {
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
}

/// Print a word and copy it in copy mode.
fn copy_word(p: &mut Paddock, word: &str) {
    p.type_line(&format!("echo {word}"));
    p.wait_for("output", |s| s.lines().any(|l| l == word));
    p.command(b"[");
    p.wait_for_text(COPY);
    // From the prompt: up to the word, select it, copy.
    p.send(b"k0ve");
    p.send(b"y");
    p.wait_for("copy mode left", |s| !s.contains(COPY));
}

/// Show that paddock still runs and handles keys: the shell answers
/// `echo` with `alive-<n>` (computed, so the typed line never matches).
fn still_runs(p: &mut Paddock, n: u32) {
    p.type_line(&format!("echo alive-$(({n}+0))"));
    p.wait_for_text(&format!("alive-{n}"));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_notifies_through_notify_send() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    look_away(&mut paddock);

    hooks(&sandbox, &["UserPromptSubmit", "PermissionRequest"]);

    assert_eq!(sandbox.wait_for_notifications(1), ["repo is waiting"]);
    assert_eq!(
        sandbox.desktop_calls(),
        ["notify-send -- paddock repo is waiting"]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_copies_with_wl_copy_on_wayland() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    sandbox.set_env("WAYLAND_DISPLAY", Some("wayland-test"));
    let mut p = ready(&sandbox);

    copy_word(&mut p, "wayland");

    sandbox.wait_for_clipboard("wayland");
    assert_eq!(sandbox.desktop_calls(), ["wl-copy "]);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_copies_with_xclip_without_wayland() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    let mut p = ready(&sandbox);

    copy_word(&mut p, "x11");

    sandbox.wait_for_clipboard("x11");
    assert_eq!(sandbox.desktop_calls(), ["xclip -selection clipboard"]);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_copies_with_pbcopy() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    let mut p = ready(&sandbox);

    copy_word(&mut p, "mac");

    sandbox.wait_for_clipboard("mac");
    assert_eq!(sandbox.desktop_calls(), ["pbcopy "]);
}

#[test]
fn the_copy_command_overrides_the_platform_tool() {
    let sandbox = Sandbox::new();
    sandbox.set_env("WAYLAND_DISPLAY", Some("wayland-test"));
    let mut p = ready(&sandbox);

    copy_word(&mut p, "override");

    // The harness's `PADDOCK_COPY_COMMAND`, which logs no call.
    sandbox.wait_for_clipboard("override");
    assert_eq!(sandbox.desktop_calls(), Vec::<String>::new());
}

#[test]
fn a_missing_notifier_is_said_once_and_paddock_goes_on() {
    let sandbox = Sandbox::new();
    let missing = sandbox.path("bin/missing");
    sandbox.set_env("PADDOCK_NOTIFY_COMMAND", Some(&missing.to_string_lossy()));
    let mut paddock = ready(&sandbox);
    look_away(&mut paddock);

    hooks(&sandbox, &["UserPromptSubmit", "PermissionRequest"]);
    paddock.wait_for_text(CANNOT_NOTIFY);
    // Any key clears the notice; this one goes to the shell.
    still_runs(&mut paddock, 1);
    assert!(!paddock.full_screen().contains(CANNOT_NOTIFY));

    // The finished turn notifies, and fails, as it is marked done: a
    // notice would be drawn in that frame, the tab bar before the sidebar.
    hooks(&sandbox, &["UserPromptSubmit", "Stop"]);
    paddock.wait_for("done", |_| paddock.full_screen().contains("done"));
    assert!(!paddock.full_screen().contains(CANNOT_NOTIFY));
    still_runs(&mut paddock, 2);
    assert!(sandbox.notifications().is_empty());
}

#[test]
fn a_missing_copy_tool_is_said_once_and_paddock_goes_on() {
    let sandbox = Sandbox::new();
    let missing = sandbox.path("bin/missing");
    sandbox.set_env("PADDOCK_COPY_COMMAND", Some(&missing.to_string_lossy()));
    let mut p = ready(&sandbox);

    copy_word(&mut p, "first");
    p.wait_for_text(CANNOT_COPY);
    still_runs(&mut p, 3);
    assert!(!p.full_screen().contains(CANNOT_COPY));

    // A notice would be drawn in the frame that leaves copy mode, the tab
    // bar before the copy-mode line.
    copy_word(&mut p, "second");
    assert!(!p.full_screen().contains(CANNOT_COPY));
    still_runs(&mut p, 4);
    assert_eq!(sandbox.clipboard(), None);
}
