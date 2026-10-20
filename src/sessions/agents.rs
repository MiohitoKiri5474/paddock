//! `claude agents --json`: the Claude sessions running right now, each
//! matched to the paddock terminal its process runs in.

use super::Agent;
use crate::tmux::Server;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::{Command, Stdio};

/// How many parents to climb from a Claude process to a terminal's shell.
const MAX_DEPTH: usize = 32;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    session_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
}

/// Parse `claude agents --json`, finding each session's pane from its pid.
pub fn parse(
    json: &str,
    parents: &HashMap<u32, u32>,
    panes: &HashMap<u32, String>,
) -> Option<Vec<Agent>> {
    let raw: Vec<Raw> = serde_json::from_str(json).ok()?;
    Some(
        raw.into_iter()
            .map(|r| Agent {
                pane: r.pid.and_then(|pid| pane_of(pid, parents, panes)),
                session_id: r.session_id,
                name: r.name,
                status: r.status,
                cwd: r.cwd,
            })
            .collect(),
    )
}

/// The pane whose shell `pid` descends from (or is).
pub fn pane_of(
    mut pid: u32,
    parents: &HashMap<u32, u32>,
    panes: &HashMap<u32, String>,
) -> Option<String> {
    for _ in 0..MAX_DEPTH {
        if let Some(pane) = panes.get(&pid) {
            return Some(pane.clone());
        }
        pid = *parents.get(&pid).filter(|&&p| p != pid)?;
    }
    None
}

/// Pairs of numbers, one pair per line (`ps -o pid=,ppid=`).
pub fn parse_pairs(out: &str) -> HashMap<u32, u32> {
    out.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        })
        .collect()
}

/// `pane_pid pane_id` lines from tmux.
pub fn parse_panes(out: &str) -> HashMap<u32, String> {
    out.lines()
        .filter_map(|l| {
            let (pid, pane) = l.split_once(' ')?;
            Some((pid.parse().ok()?, pane.to_string()))
        })
        .collect()
}

fn output(cmd: &mut Command) -> Option<String> {
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Ask Claude for its sessions. `None` if that failed: nothing is known then.
pub fn fetch(server: &Server) -> Option<Vec<Agent>> {
    let json = output(Command::new("claude").args(["agents", "--json"]))?;
    let parents = output(Command::new("ps").args(["-A", "-o", "pid=,ppid="]))
        .map(|o| parse_pairs(&o))
        .unwrap_or_default();
    let panes = server
        .run(&["list-panes", "-a", "-F", "#{pane_pid} #{pane_id}"])
        .map(|o| parse_panes(&o))
        .unwrap_or_default();
    parse(&json, &parents, &panes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_pane_through_parent_processes() {
        let parents = parse_pairs("  300   200\n200 100\n100 1\n 1 0\n");
        let panes = parse_panes("100 %4\n500 %5\n");
        assert_eq!(pane_of(300, &parents, &panes), Some("%4".into()));
        assert_eq!(pane_of(100, &parents, &panes), Some("%4".into()));
        assert_eq!(pane_of(999, &parents, &panes), None);
        let lonely = parse_pairs("7 7\n");
        assert_eq!(pane_of(7, &lonely, &panes), None, "no loop forever");
    }

    #[test]
    fn parses_claude_agents_output() {
        let json = r#"[{"pid":300,"cwd":"/r","kind":"interactive","startedAt":1,
            "sessionId":"s1","name":"fix-login","status":"busy"},
            {"sessionId":"s2","status":"idle"}]"#;
        let parents = parse_pairs("300 100\n");
        let panes = parse_panes("100 %4\n");
        let got = parse(json, &parents, &panes).unwrap();
        assert_eq!(
            got[0],
            Agent {
                session_id: "s1".into(),
                name: Some("fix-login".into()),
                status: "busy".into(),
                cwd: Some("/r".into()),
                pane: Some("%4".into()),
            }
        );
        assert_eq!(got[1].pane, None);
        assert!(parse("oops", &parents, &panes).is_none());
    }
}
