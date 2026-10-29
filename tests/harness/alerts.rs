//! Helpers for tests that make sessions need you: recorded hook payloads
//! (`tests/attention.rs`, `tests/desktop.rs`).

use super::{Paddock, Sandbox, PROMPT};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");

/// The recorded payload `name`: ours for `PermissionRequest` and
/// `Notification` variants, else the spike's.
pub fn payload(name: &str) -> String {
    let path = if name.starts_with("PermissionRequest") || name.starts_with("Notification") {
        format!("{OURS}/{name}.json")
    } else {
        format!("{SPIKE}/{name}.json")
    };
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Run hooks in order in terminal `terminal`; `-` separates an event from
/// its payload variant (`Notification-permission_prompt`).
pub fn hooks(sandbox: &Sandbox, terminal: &str, names: &[&str]) {
    for name in names {
        let event = name.split('-').next().unwrap();
        sandbox.hook(terminal, event, &payload(name));
    }
}

/// Launch paddock and wait for the shell's prompt.
pub fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}
