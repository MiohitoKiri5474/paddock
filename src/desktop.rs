//! The desktop tools paddock runs: the one that shows a notification and
//! the one that copies to the clipboard. This is their platform seam:
//! macOS runs `osascript` and `pbcopy`; Linux runs `notify-send`, and
//! `wl-copy` on Wayland or `xclip` on X11.

use std::ffi::OsString;
use std::process::Command;

/// Names the program run in place of the platform's notifier, with the
/// same arguments (tests use it to make the notifier missing).
const NOTIFY_VAR: &str = "PADDOCK_NOTIFY_COMMAND";
/// Names the program that copies its stdin, in place of the platform's
/// (tests use it to keep off the real clipboard).
const COPY_VAR: &str = "PADDOCK_COPY_COMMAND";

/// The value of `var`, unless it is unset or empty.
fn program_from(var: &str) -> Option<OsString> {
    std::env::var_os(var).filter(|v| !v.is_empty())
}

/// The command that shows a notification titled `paddock` saying `message`.
pub fn notifier(message: &str) -> Command {
    let program = program_from(NOTIFY_VAR).unwrap_or_else(|| NOTIFIER.into());
    let mut command = Command::new(program);
    command.args(notify_args(message));
    command
}

/// The command that puts what it reads on stdin on the clipboard.
pub fn copier() -> Command {
    match program_from(COPY_VAR) {
        Some(program) => Command::new(program),
        None => {
            let (program, args) = default_copier();
            let mut command = Command::new(program);
            command.args(args);
            command
        }
    }
}

#[cfg(target_os = "macos")]
const NOTIFIER: &str = "osascript";

/// The AppleScript that shows a notification; its message is the script's
/// first argument, so it needs no AppleScript quoting.
#[cfg(target_os = "macos")]
const NOTIFY_SCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 1 of argv) with title \"paddock\"",
    "end run",
];

#[cfg(target_os = "macos")]
fn notify_args(message: &str) -> Vec<String> {
    NOTIFY_SCRIPT
        .iter()
        .flat_map(|line| ["-e".to_string(), (*line).to_string()])
        .chain([message.to_string()])
        .collect()
}

#[cfg(target_os = "macos")]
fn default_copier() -> (&'static str, &'static [&'static str]) {
    ("pbcopy", &[])
}

#[cfg(target_os = "linux")]
const NOTIFIER: &str = "notify-send";

/// `--` ends the options, so a message starting with `-` is not one.
#[cfg(target_os = "linux")]
fn notify_args(message: &str) -> Vec<String> {
    vec!["--".into(), "paddock".into(), message.into()]
}

#[cfg(target_os = "linux")]
fn default_copier() -> (&'static str, &'static [&'static str]) {
    linux_copier(std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()))
}

/// `wl-copy` under a Wayland compositor, else X11's `xclip`.
#[cfg(target_os = "linux")]
fn linux_copier(wayland: bool) -> (&'static str, &'static [&'static str]) {
    if wayland {
        ("wl-copy", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_notification_script_compiles() {
        // `osacompile` checks the AppleScript without running it.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("notify.scpt");
        let mut cmd = Command::new("osacompile");
        for line in NOTIFY_SCRIPT {
            cmd.args(["-e", line]);
        }
        let result = cmd.arg("-o").arg(&out).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_message_is_the_scripts_argument() {
        let args = notify_args("repo is idle");
        assert_eq!(args.last().map(String::as_str), Some("repo is idle"));
        assert_eq!(args.len(), 2 * NOTIFY_SCRIPT.len() + 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_send_is_given_a_title_and_the_message_after_its_options() {
        assert_eq!(notify_args("-x is idle"), ["--", "paddock", "-x is idle"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_copies_with_wl_copy_and_x11_with_xclip() {
        assert_eq!(linux_copier(true), ("wl-copy", &[][..]));
        assert_eq!(
            linux_copier(false),
            ("xclip", &["-selection", "clipboard"][..])
        );
    }
}
