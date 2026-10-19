//! Claude sessions in paddock's terminals: what hooks recorded and what
//! `claude agents --json` reports, folded into one state per terminal.
//!
//! Terminals are keyed by tmux pane id (`%N`). Each terminal shows its latest
//! conversation only: an event from a new session id replaces the old one.

pub mod agents;
pub mod log;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Waiting,
    Idle,
    Stopped,
    Unknown,
}

impl State {
    /// Sort order of rows: waiting, idle, running, stopped, then unknown.
    pub fn rank(self) -> u8 {
        match self {
            State::Waiting => 0,
            State::Idle => 1,
            State::Running => 2,
            State::Stopped => 3,
            State::Unknown => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Waiting => "waiting",
            State::Idle => "idle",
            State::Stopped => "stopped",
            State::Unknown => "unknown",
        }
    }

    /// A `claude agents --json` status.
    pub fn from_status(status: &str) -> Self {
        match status {
            "busy" => State::Running,
            "waiting" => State::Waiting,
            "idle" => State::Idle,
            _ => State::Unknown,
        }
    }
}

/// One hook event as `paddock hook` records it: only the fields paddock uses,
/// never the whole payload (tool responses can be huge).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRecord {
    pub event: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    /// Set when a subagent, not the main agent, caused the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// The task branch checked out in `cwd` when the hook ran, for events
    /// after which it may have changed (see [`needs_branch`]); `None` for the
    /// default branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
}

/// The session state an event leads to, if it changes it.
pub fn state_after(r: &HookRecord) -> Option<State> {
    let state = match r.event.as_str() {
        "SessionStart" if r.source.as_deref() == Some("compact") => State::Running,
        "SessionStart" | "Stop" => State::Idle,
        "UserPromptSubmit" | "PreCompact" | "PostToolUse" => State::Running,
        "PermissionRequest" => State::Waiting,
        "Notification" => match r.notification_type.as_deref() {
            Some("permission_prompt" | "elicitation_dialog") => State::Waiting,
            Some("idle_prompt") => State::Idle,
            _ => return None,
        },
        "SessionEnd" => State::Stopped,
        _ => return None,
    };
    // A subagent blocked on you blocks its session; its other events say
    // nothing about the main agent.
    (r.agent_id.is_none() || state == State::Waiting).then_some(state)
}

/// Whether the main agent may have switched branch before this event (it
/// started, was prompted, or ran a shell command), so the hook records the
/// branch checked out in its directory. Claude's transcripts carry a stale
/// branch, so git is the only source.
pub fn needs_branch(r: &HookRecord) -> bool {
    let switched = match r.event.as_str() {
        "SessionStart" | "UserPromptSubmit" => true,
        "PostToolUse" => r.tool_name.as_deref() == Some("Bash"),
        _ => false,
    };
    switched && r.agent_id.is_none() && r.cwd.is_some()
}

/// A task: one branch a session worked on, and the directory it was seen in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub branch: String,
    pub cwd: String,
}

/// A session reported by `claude agents --json`, with the terminal it runs in
/// if paddock could find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub session_id: String,
    pub name: Option<String>,
    pub status: String,
    pub cwd: Option<String>,
    pub pane: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub state: State,
    /// When the state was last set (ms since the epoch).
    pub updated: u64,
    /// Branches the main agent worked on, oldest first.
    pub tasks: Vec<Task>,
}

impl Session {
    fn new(session_id: Option<String>) -> Self {
        Self {
            session_id,
            cwd: None,
            name: None,
            state: State::Unknown,
            updated: 0,
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Sessions {
    by_pane: HashMap<String, Session>,
}

impl Sessions {
    pub fn get(&self, pane: &str) -> Option<&Session> {
        self.by_pane.get(pane)
    }

    /// Apply one recorded hook event.
    pub fn apply(&mut self, r: &HookRecord) {
        let s = self
            .by_pane
            .entry(r.pane.clone())
            .or_insert_with(|| Session::new(r.session_id.clone()));
        if r.session_id.is_some() && r.session_id != s.session_id {
            *s = Session::new(r.session_id.clone());
        }
        if r.cwd.is_some() {
            s.cwd.clone_from(&r.cwd);
        }
        if let Some(state) = state_after(r) {
            s.state = state;
            s.updated = r.ts;
        }
        if let (Some(branch), Some(cwd), None) = (&r.branch, &r.cwd, &r.agent_id) {
            match s.tasks.iter_mut().find(|t| t.branch == *branch) {
                Some(t) => t.cwd.clone_from(cwd),
                None => s.tasks.push(Task {
                    branch: branch.clone(),
                    cwd: cwd.clone(),
                }),
            }
        }
    }

    /// Fold in a `claude agents --json` snapshot taken at `taken` (ms). It
    /// catches what hooks miss (an Esc interrupt fires no Stop) and finds
    /// sessions no hook reported, but never overrides a newer hook event.
    pub fn reconcile(&mut self, taken: u64, agents: &[Agent]) {
        for a in agents {
            let known = self
                .by_pane
                .iter()
                .find(|(_, s)| s.session_id.as_deref() == Some(a.session_id.as_str()))
                .map(|(p, _)| p.clone());
            let Some(pane) = known.or_else(|| a.pane.clone()) else {
                continue;
            };
            let s = self
                .by_pane
                .entry(pane)
                .or_insert_with(|| Session::new(Some(a.session_id.clone())));
            if taken <= s.updated {
                if s.session_id.as_deref() == Some(a.session_id.as_str()) {
                    s.name.clone_from(&a.name);
                }
                continue;
            }
            if s.session_id.as_deref() != Some(a.session_id.as_str()) {
                *s = Session::new(Some(a.session_id.clone()));
            }
            if s.cwd.is_none() {
                s.cwd.clone_from(&a.cwd);
            }
            s.name.clone_from(&a.name);
            s.state = State::from_status(&a.status);
            s.updated = taken;
        }
    }
}

/// A session row's label: the Claude session name if set, else the tab name
/// (`None` while it still has its default name), else the directory name.
pub fn label(session: &Session, tab: Option<&str>) -> String {
    let dir = || {
        session
            .cwd
            .as_deref()
            .and_then(|c| Path::new(c).file_name())
            .map(|n| n.to_string_lossy().into_owned())
    };
    session
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| tab.map(str::to_string))
        .or_else(dir)
        .unwrap_or_else(|| "claude".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const S1: &str = "11111111-2222-4333-8444-555555555555";

    fn rec(event: &str, ts: u64) -> HookRecord {
        HookRecord {
            event: event.into(),
            pane: "%1".into(),
            session_id: Some(S1.into()),
            cwd: Some("/tmp/repo".into()),
            ts,
            ..Default::default()
        }
    }

    fn state(s: &Sessions) -> State {
        s.get("%1").unwrap().state
    }

    fn agent(id: &str, status: &str, pane: Option<&str>) -> Agent {
        Agent {
            session_id: id.into(),
            name: None,
            status: status.into(),
            cwd: Some("/tmp/repo".into()),
            pane: pane.map(str::to_string),
        }
    }

    #[test]
    fn states_follow_main_agent_events() {
        let cases = [
            ("SessionStart", None, None, Some(State::Idle)),
            ("SessionStart", Some("compact"), None, Some(State::Running)),
            ("UserPromptSubmit", None, None, Some(State::Running)),
            ("PostToolUse", None, None, Some(State::Running)),
            ("PreCompact", None, None, Some(State::Running)),
            ("PermissionRequest", None, None, Some(State::Waiting)),
            (
                "Notification",
                None,
                Some("permission_prompt"),
                Some(State::Waiting),
            ),
            (
                "Notification",
                None,
                Some("elicitation_dialog"),
                Some(State::Waiting),
            ),
            ("Notification", None, Some("idle_prompt"), Some(State::Idle)),
            ("Notification", None, Some("auth_success"), None),
            ("Stop", None, None, Some(State::Idle)),
            ("SessionEnd", None, None, Some(State::Stopped)),
            ("SomethingNew", None, None, None),
        ];
        for (event, source, kind, want) in cases {
            let r = HookRecord {
                source: source.map(str::to_string),
                notification_type: kind.map(str::to_string),
                ..rec(event, 1)
            };
            assert_eq!(state_after(&r), want, "{event} {source:?} {kind:?}");
        }
    }

    #[test]
    fn subagent_events_only_matter_when_they_wait() {
        let sub = |event| HookRecord {
            agent_id: Some("a1".into()),
            ..rec(event, 1)
        };
        assert_eq!(state_after(&sub("PostToolUse")), None);
        assert_eq!(state_after(&sub("PermissionRequest")), Some(State::Waiting));
    }

    #[test]
    fn a_terminal_shows_its_latest_conversation() {
        let mut s = Sessions::default();
        s.apply(&rec("SessionStart", 1));
        s.apply(&rec("UserPromptSubmit", 2));
        assert_eq!(state(&s), State::Running);
        s.apply(&rec("Stop", 3));
        assert_eq!(state(&s), State::Idle);
        s.apply(&HookRecord {
            session_id: Some("new".into()),
            cwd: None,
            ..rec("SessionStart", 4)
        });
        let now = s.get("%1").unwrap();
        assert_eq!(now.session_id.as_deref(), Some("new"));
        assert_eq!(now.state, State::Idle);
        assert_eq!(now.cwd, None, "nothing kept from the old conversation");
    }

    #[test]
    fn events_without_a_state_still_create_the_row() {
        let mut s = Sessions::default();
        s.apply(&rec("SomethingNew", 1));
        assert_eq!(state(&s), State::Unknown);
    }

    #[test]
    fn a_newer_snapshot_wins_over_hooks() {
        let mut s = Sessions::default();
        s.apply(&rec("UserPromptSubmit", 10));
        // The user pressed Esc: no Stop, but claude reports idle.
        s.reconcile(20, &[agent(S1, "idle", None)]);
        assert_eq!(state(&s), State::Idle);
    }

    #[test]
    fn a_stale_snapshot_never_overrides_a_hook() {
        let mut s = Sessions::default();
        s.apply(&rec("Stop", 30));
        s.reconcile(
            20,
            &[Agent {
                name: Some("named".into()),
                ..agent(S1, "busy", None)
            }],
        );
        assert_eq!(state(&s), State::Idle);
        assert_eq!(s.get("%1").unwrap().name.as_deref(), Some("named"));
    }

    #[test]
    fn snapshots_discover_sessions_in_known_terminals_only() {
        let mut s = Sessions::default();
        s.reconcile(
            5,
            &[agent("x", "waiting", Some("%7")), agent("y", "busy", None)],
        );
        let found = s.get("%7").unwrap();
        assert_eq!(found.session_id.as_deref(), Some("x"));
        assert_eq!(found.state, State::Waiting);
        assert_eq!(found.cwd.as_deref(), Some("/tmp/repo"));
        assert!(s.get("%1").is_none());
    }

    #[test]
    fn a_new_conversation_found_by_snapshot_replaces_an_older_one() {
        let mut s = Sessions::default();
        s.apply(&rec("Stop", 1));
        s.reconcile(5, &[agent("newer", "busy", Some("%1"))]);
        let now = s.get("%1").unwrap();
        assert_eq!(now.session_id.as_deref(), Some("newer"));
        assert_eq!(now.state, State::Running);
    }

    #[test]
    fn branches_are_read_when_the_main_agent_may_have_switched() {
        let bash = |event: &str, tool: &str| HookRecord {
            tool_name: Some(tool.into()),
            ..rec(event, 1)
        };
        assert!(needs_branch(&rec("SessionStart", 1)));
        assert!(needs_branch(&rec("UserPromptSubmit", 1)));
        assert!(needs_branch(&bash("PostToolUse", "Bash")));
        assert!(!needs_branch(&bash("PostToolUse", "Edit")));
        assert!(!needs_branch(&rec("Stop", 1)));
        assert!(!needs_branch(&HookRecord {
            cwd: None,
            ..rec("SessionStart", 1)
        }));
        assert!(!needs_branch(&HookRecord {
            agent_id: Some("a1".into()),
            ..bash("PostToolUse", "Bash")
        }));
    }

    fn on(branch: &str, cwd: &str, ts: u64) -> HookRecord {
        HookRecord {
            branch: Some(branch.into()),
            cwd: Some(cwd.into()),
            ..rec("PostToolUse", ts)
        }
    }

    fn tasks(s: &Sessions) -> Vec<(&str, &str)> {
        s.get("%1")
            .unwrap()
            .tasks
            .iter()
            .map(|t| (t.branch.as_str(), t.cwd.as_str()))
            .collect()
    }

    #[test]
    fn each_branch_the_main_agent_works_on_becomes_one_task() {
        let mut s = Sessions::default();
        s.apply(&rec("SessionStart", 1));
        assert!(tasks(&s).is_empty(), "default branch: no task");
        s.apply(&on("feat/a", "/r", 2));
        s.apply(&on("feat/b", "/r", 3));
        s.apply(&on("feat/a", "/r/sub", 4));
        s.apply(&rec("Stop", 5));
        assert_eq!(tasks(&s), [("feat/a", "/r/sub"), ("feat/b", "/r")]);
    }

    #[test]
    fn subagent_branches_are_not_main_agent_tasks() {
        let mut s = Sessions::default();
        s.apply(&HookRecord {
            agent_id: Some("a1".into()),
            ..on("fix/a", "/r/wt", 1)
        });
        assert!(tasks(&s).is_empty());
    }

    #[test]
    fn a_new_conversation_starts_without_tasks() {
        let mut s = Sessions::default();
        s.apply(&on("feat/a", "/r", 1));
        s.apply(&HookRecord {
            session_id: Some("new".into()),
            ..rec("SessionStart", 2)
        });
        assert!(tasks(&s).is_empty());
    }

    #[test]
    fn rows_sort_waiting_idle_running_stopped_unknown() {
        let mut all = [
            State::Unknown,
            State::Stopped,
            State::Running,
            State::Idle,
            State::Waiting,
        ];
        all.sort_by_key(|s| s.rank());
        let names: Vec<&str> = all.iter().map(|s| s.label()).collect();
        assert_eq!(names, ["waiting", "idle", "running", "stopped", "unknown"]);
        assert_eq!(State::from_status("busy"), State::Running);
        assert_eq!(State::from_status("odd"), State::Unknown);
    }

    #[test]
    fn labels_prefer_session_name_then_tab_then_directory() {
        let mut s = Session::new(None);
        assert_eq!(label(&s, None), "claude");
        s.cwd = Some("/work/api".into());
        assert_eq!(label(&s, None), "api");
        assert_eq!(label(&s, Some("build")), "build");
        s.name = Some("fix-login".into());
        assert_eq!(label(&s, Some("build")), "fix-login");
        s.name = Some(String::new());
        assert_eq!(label(&s, None), "api");
    }
}
