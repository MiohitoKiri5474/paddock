//! Agents never move when their state changes: the sidebar's AGENTS and the
//! dashboard (`C-a o`) list them in space, tab and split order, whatever
//! state each is in. Only `C-a a` goes by urgency.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use serde_json::json;

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
/// The start of the dashboard's key line, as drawn while it is open.
const OPEN: &str = "j down  k up";
/// The agents' labels (their directories' names), in tab order.
const LABELS: [&str; 3] = ["alpha", "bravo", "charlie"];

/// The recorded payload of `event`: ours for `PermissionRequest`, else
/// the spike's.
fn payload(event: &str) -> String {
    let dir = if event == "PermissionRequest" {
        OURS
    } else {
        SPIKE
    };
    let path = format!("{dir}/{event}.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Report `event` for the Claude session `session` in terminal `terminal`,
/// working in `/w/<label>`.
fn hook(sandbox: &Sandbox, terminal: &str, event: &str, session: &str, label: &str) {
    let body = payload(event)
        .replace(SESSION, session)
        .replace("\"/tmp/repo\"", &format!("\"/w/{label}\""));
    sandbox.hook(terminal, event, &body);
}

/// The agents' labels in the order `lines` show them.
fn order(lines: &[String]) -> Vec<&'static str> {
    lines
        .iter()
        .filter_map(|l| LABELS.into_iter().find(|label| l.contains(label)))
        .collect()
}

/// The labels in the sidebar's AGENTS, top first.
fn sidebar_order(paddock: &Paddock) -> Vec<&'static str> {
    let lines = paddock.sidebar();
    let agents = lines
        .iter()
        .position(|l| l.contains("AGENTS"))
        .unwrap_or_else(|| panic!("no AGENTS:\n{}", lines.join("\n")));
    order(&lines[agents..])
}

/// The labels in the dashboard's list, top first: the lines above the
/// preview's border.
fn dashboard_order(paddock: &Paddock) -> Vec<&'static str> {
    let rows = paddock.rows();
    let list: Vec<String> = rows
        .iter()
        .take_while(|l| !l.contains("preview"))
        .cloned()
        .collect();
    order(&list)
}

/// Wait until the sidebar shows `states` (each agent's, in tab order).
fn wait_for_states(paddock: &Paddock, states: [&str; 3]) {
    paddock.wait_for("states", |_| {
        let lines = paddock.sidebar();
        LABELS.iter().zip(states).all(|(label, state)| {
            lines
                .iter()
                .any(|l| l.contains(label) && l.trim_end().ends_with(state))
        })
    });
}

#[test]
fn a_state_change_moves_no_agent_in_the_sidebar_or_the_dashboard() {
    let sandbox = Sandbox::new();
    // No `claude agents` snapshots: only the hooks set the states.
    sandbox.set_agents("not json");
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    for tab in ["2:home", "3:home"] {
        paddock.command(b"c");
        paddock.wait_for_current_tab(tab);
        paddock.wait_for_text(PROMPT);
    }
    // alpha done, bravo waiting, charlie running: not in urgency order.
    hook(&sandbox, "space-1:0", "Stop", "s1", "alpha");
    hook(&sandbox, "space-1:1", "PermissionRequest", "s2", "bravo");
    hook(&sandbox, "space-1:2", "UserPromptSubmit", "s3", "charlie");
    wait_for_states(&paddock, ["done", "waiting", "running"]);
    assert_eq!(sidebar_order(&paddock), LABELS);
    paddock.command(b"o");
    paddock.wait_for_text(OPEN);
    assert_eq!(dashboard_order(&paddock), LABELS, "{}", paddock.screen());

    // charlie, last, starts waiting; bravo, waiting, runs again.
    hook(&sandbox, "space-1:2", "PermissionRequest", "s3", "charlie");
    hook(&sandbox, "space-1:1", "PostToolUse", "s2", "bravo");
    wait_for_states(&paddock, ["done", "running", "waiting"]);
    assert_eq!(dashboard_order(&paddock), LABELS, "{}", paddock.screen());
    paddock.send(b"\x1b");
    paddock.wait_for("dashboard closed", |s| !s.contains(OPEN));
    assert_eq!(sidebar_order(&paddock), LABELS);
}

/// What `claude agents --json` lists: alpha, bravo and charlie in the
/// first three tabs' shells, with `states` (its statuses), and two
/// sessions outside paddock, delta and echo, with `outside`.
fn snapshot(sandbox: &Sandbox, states: [&str; 3], outside: [&str; 2]) -> String {
    let mut agents: Vec<serde_json::Value> = LABELS
        .iter()
        .zip(states)
        .enumerate()
        .map(|(tab, (label, status))| {
            let pid = sandbox
                .terminal(&format!("space-1:{tab}"))
                .pid
                .expect("the shell's pid");
            json!({"sessionId": label, "cwd": format!("/w/{label}"), "status": status, "pid": pid})
        })
        .collect();
    agents.extend(["delta", "echo"].into_iter().zip(outside).map(|(label, status)| {
        json!({"sessionId": label, "cwd": format!("/w/{label}"), "status": status, "pid": 1})
    }));
    serde_json::Value::Array(agents).to_string()
}

#[test]
fn a_snapshot_changing_states_moves_no_agent() {
    let sandbox = Sandbox::new();
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    for tab in ["2:home", "3:home"] {
        paddock.command(b"c");
        paddock.wait_for_current_tab(tab);
        paddock.wait_for_text(PROMPT);
    }
    let all = ["alpha", "bravo", "charlie", "delta", "echo"];
    let shown = |p: &Paddock| -> Vec<&str> {
        p.sidebar()
            .iter()
            .filter(|l| l.contains("●"))
            .filter_map(|l| all.into_iter().find(|a| l.contains(a)))
            .collect()
    };

    sandbox.set_agents(&snapshot(
        &sandbox,
        ["idle", "waiting", "busy"],
        ["busy", "waiting"],
    ));
    paddock.wait_for("first snapshot", |_| {
        let lines = paddock.sidebar();
        lines
            .iter()
            .any(|l| l.contains("charlie") && l.ends_with("running"))
            && lines
                .iter()
                .any(|l| l.contains("echo") && l.ends_with("waiting"))
    });
    assert_eq!(shown(&paddock), all, "{}", paddock.sidebar().join("\n"));

    sandbox.set_agents(&snapshot(
        &sandbox,
        ["busy", "busy", "waiting"],
        ["waiting", "busy"],
    ));
    paddock.wait_for("second snapshot", |_| {
        let lines = paddock.sidebar();
        lines
            .iter()
            .any(|l| l.contains("charlie") && l.ends_with("waiting"))
            && lines
                .iter()
                .any(|l| l.contains("echo") && l.ends_with("running"))
    });
    assert_eq!(shown(&paddock), all, "{}", paddock.sidebar().join("\n"));
    paddock.command(b"o");
    paddock.wait_for_text(OPEN);
    let list: Vec<String> = paddock
        .rows()
        .into_iter()
        .take_while(|l| !l.contains("preview"))
        .collect();
    let dashboard: Vec<&str> = list
        .iter()
        .filter_map(|l| all.into_iter().find(|a| l.contains(a)))
        .collect();
    // The external rows are under the list's fold at this size.
    assert_eq!(dashboard, LABELS, "{}", paddock.screen());
}

#[test]
fn agents_across_spaces_and_tabs_stay_put_when_one_starts_waiting() {
    let sandbox = Sandbox::new();
    sandbox.set_agents("not json");
    let mut paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    // Three spaces of two tabs each.
    for first in [false, true, true] {
        if first {
            paddock.command(b"C\r");
            paddock.wait_for_tabs(&["1:home"]);
            paddock.wait_for_text(PROMPT);
        }
        paddock.command(b"c");
        paddock.wait_for_current_tab("2:home");
        paddock.wait_for_text(PROMPT);
    }
    // Running agents in each space's first tab, finished ones in its second:
    // urgency would list b, d, f first.
    let agents = [
        ("space-1:0", "UserPromptSubmit", "a"),
        ("space-1:1", "Stop", "b"),
        ("space-2:0", "UserPromptSubmit", "c"),
        ("space-2:1", "Stop", "d"),
        ("space-3:0", "UserPromptSubmit", "e"),
        ("space-3:1", "Stop", "f"),
    ];
    for (terminal, event, label) in agents {
        hook(&sandbox, terminal, event, label, label);
    }
    let labels: Vec<&str> = agents.iter().map(|(_, _, label)| *label).collect();
    // Each agent's first line: its dot, label and state.
    let shown = |p: &Paddock| -> Vec<String> {
        let lines = p.sidebar();
        let agents = lines.iter().position(|l| l.contains("AGENTS")).unwrap_or(0);
        lines[agents..]
            .iter()
            .filter(|l| l.contains("●"))
            .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
            .collect()
    };
    let states = |p: &Paddock| -> Vec<String> {
        p.sidebar()
            .iter()
            .filter(|l| l.contains("●"))
            .filter_map(|l| l.split_whitespace().last().map(str::to_string))
            .collect()
    };
    paddock.wait_for("every agent", |_| shown(&paddock).len() == 6);
    paddock.wait_for("every state", |_| {
        states(&paddock).iter().filter(|s| *s == "running").count() == 3
    });
    assert_eq!(shown(&paddock), labels, "{}", paddock.sidebar().join("\n"));

    hook(&sandbox, "space-2:0", "PermissionRequest", "c", "c");
    paddock.wait_for("c waiting", |_| {
        states(&paddock).iter().any(|s| s == "waiting")
    });
    assert_eq!(shown(&paddock), labels, "{}", paddock.sidebar().join("\n"));
}
