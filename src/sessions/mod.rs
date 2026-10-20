//! Claude sessions in paddock's terminals: what hooks recorded and what
//! `claude agents --json` reports, folded into one state per terminal.
//!
//! Terminals are keyed by tmux pane id (`%N`). Each terminal shows its latest
//! conversation only: an event from a new session id replaces the old one,
//! keeping only the terminal's task rows.

pub mod agents;
pub mod dismissed;
pub mod log;

pub use dismissed::Dismissal;

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
    /// A subagent's task description, when the hook could find it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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

/// Whether the agent may have switched branch before this event (it started,
/// was prompted, ran a shell command, or a subagent finished), so the hook
/// records the branch checked out in its directory: the session's for the
/// main agent, the worktree's for a subagent. Claude's transcripts carry a
/// stale branch, so git is the only source.
pub fn needs_branch(r: &HookRecord) -> bool {
    let switched = match r.event.as_str() {
        "SessionStart" | "UserPromptSubmit" | "SubagentStart" | "SubagentStop" => true,
        "PostToolUse" => r.tool_name.as_deref() == Some("Bash"),
        _ => false,
    };
    switched && r.cwd.is_some()
}

/// A subagent task's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Waiting,
    Done,
    Stopped,
}

impl TaskState {
    pub fn label(self) -> &'static str {
        match self {
            TaskState::Running => "running",
            TaskState::Waiting => "waiting",
            TaskState::Done => "done",
            TaskState::Stopped => "stopped",
        }
    }
}

/// The state a subagent's own event leads to, if it changes it.
fn subagent_state(r: &HookRecord) -> Option<TaskState> {
    match (r.event.as_str(), state_after(r)) {
        ("SubagentStop", _) => Some(TaskState::Done),
        (_, Some(State::Waiting)) => Some(TaskState::Waiting),
        ("SubagentStart" | "PostToolUse", _) => Some(TaskState::Running),
        _ => None,
    }
}

/// A task: one branch a session worked on, and the directory it was seen in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub branch: String,
    pub cwd: String,
    /// Set when subagents, not the main agent, work on the branch.
    pub subagent: Option<SubagentTask>,
}

/// What a subagent task row shows: the latest subagent on the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentTask {
    pub agent_id: String,
    pub description: Option<String>,
    /// Running or waiting while any subagent on the branch is.
    pub state: TaskState,
}

/// A subagent of a session, as its hook events left it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Subagent {
    id: String,
    cwd: String,
    /// Its worktree's task branch, once read.
    branch: Option<String>,
    description: Option<String>,
    state: TaskState,
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
    /// Branches the main agent and its subagents worked on, oldest first.
    pub tasks: Vec<Task>,
    /// When a hook event last set the state (ms since the epoch).
    pub hooked: Option<u64>,
    /// Subagents seen, in the order they reached their current branch.
    subagents: Vec<Subagent>,
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
            hooked: None,
            subagents: Vec::new(),
        }
    }

    /// Fold in an event of subagent `id`.
    fn apply_subagent(&mut self, id: &str, r: &HookRecord) {
        let i = match self.subagents.iter().position(|a| a.id == id) {
            Some(i) => i,
            None => {
                self.subagents.push(Subagent {
                    id: id.to_string(),
                    cwd: String::new(),
                    branch: None,
                    description: None,
                    state: TaskState::Running,
                });
                self.subagents.len() - 1
            }
        };
        let a = &mut self.subagents[i];
        if let Some(cwd) = &r.cwd {
            a.cwd.clone_from(cwd);
        }
        if r.description.is_some() {
            a.description.clone_from(&r.description);
        }
        if let Some(state) = subagent_state(r) {
            a.state = state;
        }
        let moved = r.branch.is_some() && r.branch != a.branch;
        let old = if moved {
            std::mem::replace(&mut a.branch, r.branch.clone())
        } else {
            None
        };
        if moved {
            // The latest subagent on a branch is the last one to reach it.
            let a = self.subagents.remove(i);
            self.subagents.push(a);
        }
        let branch = self
            .subagents
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.branch.clone());
        if let Some(old) = old {
            // Claude names a worktree's first branch after the agent; the
            // agent's real task branch replaces it, in place.
            if old == format!("worktree-agent-{id}") && !self.on_branch(&old) {
                let new = branch.as_deref().unwrap_or_default();
                let taken = self.tasks.iter().any(|t| t.branch == new);
                let first = |t: &Task| t.branch == old && t.subagent.is_some();
                match self.tasks.iter_mut().find(|t| first(t)) {
                    Some(t) if !taken => t.branch = new.to_string(),
                    _ => self.tasks.retain(|t| !first(t)),
                }
            }
            self.sync(&old);
        }
        if let Some(branch) = branch {
            self.sync(&branch);
        }
    }

    /// The task branch subagent `id` works on, once read.
    fn subagent_branch(&self, id: &str) -> Option<String> {
        self.subagents
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.branch.clone())
    }

    /// The same terminal's next conversation: only the task rows carry over.
    fn next(&mut self, session_id: Option<String>) {
        self.stop_subagents();
        let tasks = std::mem::take(&mut self.tasks);
        *self = Session {
            tasks,
            ..Session::new(session_id)
        };
    }

    fn on_branch(&self, branch: &str) -> bool {
        self.subagents
            .iter()
            .any(|a| a.branch.as_deref() == Some(branch))
    }

    /// The session ended: subagents still at work never finish.
    fn stop_subagents(&mut self) {
        for a in &mut self.subagents {
            if matches!(a.state, TaskState::Running | TaskState::Waiting) {
                a.state = TaskState::Stopped;
            }
        }
        let branches: Vec<String> = self.tasks.iter().map(|t| t.branch.clone()).collect();
        for b in branches {
            self.sync(&b);
        }
    }

    /// Bring `branch`'s task row up to date with the subagents on it. The
    /// main agent's own branch stays its row.
    fn sync(&mut self, branch: &str) {
        let on: Vec<&Subagent> = self
            .subagents
            .iter()
            .filter(|a| a.branch.as_deref() == Some(branch))
            .collect();
        let Some(latest) = on.last() else {
            return;
        };
        let busy = |s: TaskState| on.iter().any(|a| a.state == s);
        let state = [TaskState::Waiting, TaskState::Running]
            .into_iter()
            .find(|&s| busy(s))
            .unwrap_or(latest.state);
        let task = SubagentTask {
            agent_id: latest.id.clone(),
            description: on.iter().rev().find_map(|a| a.description.clone()),
            state,
        };
        let cwd = latest.cwd.clone();
        match self.tasks.iter_mut().find(|t| t.branch == branch) {
            Some(t) if t.subagent.is_none() => {}
            Some(t) => {
                t.cwd = cwd;
                t.subagent = Some(task);
            }
            None => self.tasks.push(Task {
                branch: branch.to_string(),
                cwd,
                subagent: Some(task),
            }),
        }
    }
}

/// How long (ms) a hook event's state holds against `claude agents --json`,
/// which can lag behind it by a poll or so.
pub const HOOK_GRACE: u64 = 4_000;

/// A session row (`None`) or one of its task rows (`Some(branch)`) of a pane.
type RowKey = (String, Option<String>);

#[derive(Debug, Default)]
pub struct Sessions {
    by_pane: HashMap<String, Session>,
    /// When a row last had an event about it (ms since the epoch).
    seen: HashMap<RowKey, u64>,
    /// When the user dismissed a row (ms since the epoch).
    dismissed: HashMap<RowKey, u64>,
}

impl Sessions {
    pub fn get(&self, pane: &str) -> Option<&Session> {
        self.by_pane.get(pane)
    }

    /// Every terminal with a session, closed ones included.
    pub fn panes(&self) -> impl Iterator<Item = &str> {
        self.by_pane.keys().map(String::as_str)
    }

    /// Pane `pane`'s session and task rows, leaving out dismissed ones; `None`
    /// if its session row is dismissed. A dismissed row stays hidden until an
    /// event about it newer than the dismissal (a dismissed session row hides
    /// its task rows too), so replaying the event log keeps it hidden.
    pub fn shown(&self, pane: &str) -> Option<(&Session, Vec<&Task>)> {
        let s = self.by_pane.get(pane)?;
        let key = |branch: Option<&str>| (pane.to_string(), branch.map(str::to_string));
        let seen = |k: &RowKey| self.seen.get(k).copied().unwrap_or(0);
        let hidden =
            |k: &RowKey, by: &RowKey| self.dismissed.get(by).is_some_and(|&at| seen(k) <= at);
        let row = key(None);
        if hidden(&row, &row) {
            return None;
        }
        let tasks = s
            .tasks
            .iter()
            .filter(|t| {
                let k = key(Some(&t.branch));
                !hidden(&k, &row) && !hidden(&k, &k)
            })
            .collect();
        Some((s, tasks))
    }

    /// Dismiss pane `pane`'s session row, or its task row on `branch`, at
    /// `at` (ms since the epoch).
    pub fn dismiss(&mut self, pane: &str, branch: Option<&str>, at: u64) {
        let key = (pane.to_string(), branch.map(str::to_string));
        let old = self.dismissed.entry(key).or_insert(at);
        *old = (*old).max(at);
    }

    /// Every dismissal, in a stable order.
    pub fn dismissals(&self) -> Vec<Dismissal> {
        let mut list: Vec<Dismissal> = self
            .dismissed
            .iter()
            .map(|((pane, branch), &at)| Dismissal {
                pane: pane.clone(),
                branch: branch.clone(),
                at,
            })
            .collect();
        list.sort();
        list
    }

    /// Note an event about a row at `ts`.
    fn touch(&mut self, pane: &str, branch: Option<String>, ts: u64) {
        let seen = self.seen.entry((pane.to_string(), branch)).or_insert(ts);
        *seen = (*seen).max(ts);
    }

    /// Apply one recorded hook event.
    pub fn apply(&mut self, r: &HookRecord) {
        self.fold(r);
        self.touch(&r.pane, None, r.ts);
        let branch = match &r.agent_id {
            Some(id) => self
                .by_pane
                .get(&r.pane)
                .and_then(|s| s.subagent_branch(id)),
            None => r.branch.clone().filter(|_| r.cwd.is_some()),
        };
        if branch.is_some() {
            self.touch(&r.pane, branch, r.ts);
        }
    }

    fn fold(&mut self, r: &HookRecord) {
        let s = self
            .by_pane
            .entry(r.pane.clone())
            .or_insert_with(|| Session::new(r.session_id.clone()));
        if r.session_id.is_some() && r.session_id != s.session_id {
            s.next(r.session_id.clone());
        }
        // A subagent's directory is its worktree, not the session's.
        if r.cwd.is_some() && r.agent_id.is_none() {
            s.cwd.clone_from(&r.cwd);
        }
        if let Some(state) = state_after(r) {
            s.state = state;
            s.updated = r.ts;
            s.hooked = Some(r.ts);
        }
        if let Some(id) = &r.agent_id {
            s.apply_subagent(id, r);
            return;
        }
        if r.event == "SessionEnd" {
            s.stop_subagents();
        }
        if let (Some(branch), Some(cwd)) = (&r.branch, &r.cwd) {
            match s.tasks.iter_mut().find(|t| t.branch == *branch) {
                Some(t) => t.cwd.clone_from(cwd),
                None => s.tasks.push(Task {
                    branch: branch.clone(),
                    cwd: cwd.clone(),
                    subagent: None,
                }),
            }
        }
    }

    /// Fold in a `claude agents --json` snapshot taken at `taken` (ms). It
    /// catches what hooks miss (an Esc interrupt fires no Stop) and finds
    /// sessions no hook reported, but never overrides a newer hook event, nor
    /// one less than [`HOOK_GRACE`] older.
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
            let found = !self.by_pane.contains_key(&pane);
            let s = self
                .by_pane
                .entry(pane.clone())
                .or_insert_with(|| Session::new(Some(a.session_id.clone())));
            let same = s.session_id.as_deref() == Some(a.session_id.as_str());
            let fresh_hook = same && s.hooked.is_some_and(|h| taken <= h + HOOK_GRACE);
            if taken <= s.updated || fresh_hook {
                if same {
                    s.name.clone_from(&a.name);
                }
                continue;
            }
            if !same {
                s.next(Some(a.session_id.clone()));
            }
            if s.cwd.is_none() {
                s.cwd.clone_from(&a.cwd);
            }
            s.name.clone_from(&a.name);
            s.state = State::from_status(&a.status);
            s.updated = taken;
            // A conversation no hook reported counts as an event of its row.
            if found || !same {
                self.touch(&pane, None, taken);
            }
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
        assert_eq!(
            now.cwd, None,
            "only tasks are kept from the old conversation"
        );
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
        s.reconcile(10 + HOOK_GRACE + 1, &[agent(S1, "idle", None)]);
        assert_eq!(state(&s), State::Idle);
    }

    #[test]
    fn a_snapshot_right_after_a_hook_does_not_override_it() {
        let mut s = Sessions::default();
        s.apply(&rec("Stop", 10_000));
        // `claude agents` still says busy a poll after Stop.
        s.reconcile(13_000, &[agent(S1, "busy", None)]);
        assert_eq!(state(&s), State::Idle);
        // Once the hook is old, snapshots correct each other every poll.
        s.reconcile(10_000 + HOOK_GRACE + 1, &[agent(S1, "busy", None)]);
        assert_eq!(state(&s), State::Running);
        s.reconcile(10_000 + HOOK_GRACE + 2, &[agent(S1, "idle", None)]);
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
        // A subagent's worktree branch is read too, from its own directory.
        let sub = |event: &str| HookRecord {
            agent_id: Some("a1".into()),
            tool_name: Some("Bash".into()),
            ..rec(event, 1)
        };
        assert!(needs_branch(&sub("PostToolUse")));
        assert!(needs_branch(&sub("SubagentStart")));
        assert!(needs_branch(&sub("SubagentStop")));
        assert!(!needs_branch(&sub("PermissionRequest")));
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

    const WT_A: &str = "/r/.claude/worktrees/agent-a1";
    const WT_B: &str = "/r/.claude/worktrees/agent-b2";

    /// An event of subagent `id` working in `cwd`, on `branch` if read.
    fn sub(event: &str, id: &str, cwd: &str, branch: Option<&str>, ts: u64) -> HookRecord {
        HookRecord {
            agent_id: Some(id.into()),
            cwd: Some(cwd.into()),
            branch: branch.map(str::to_string),
            tool_name: Some("Bash".into()),
            ..rec(event, ts)
        }
    }

    fn described(r: HookRecord, description: &str) -> HookRecord {
        HookRecord {
            description: Some(description.into()),
            ..r
        }
    }

    type Line = (String, Option<String>, Option<TaskState>);

    /// Task rows as `(branch, label, state)`; main-agent rows have neither.
    fn subtasks(s: &Sessions) -> Vec<Line> {
        s.get("%1")
            .unwrap()
            .tasks
            .iter()
            .map(|t| match &t.subagent {
                Some(a) => (t.branch.clone(), a.description.clone(), Some(a.state)),
                None => (t.branch.clone(), None, None),
            })
            .collect()
    }

    fn line(branch: &str, label: Option<&str>, state: Option<TaskState>) -> Line {
        (branch.into(), label.map(str::to_string), state)
    }

    #[test]
    fn parallel_subagents_get_a_task_row_per_worktree_branch() {
        use TaskState::{Done, Running, Stopped};
        let mut s = Sessions::default();
        s.apply(&rec("UserPromptSubmit", 1));
        s.apply(&described(
            sub("SubagentStart", "a1", WT_A, Some("worktree-agent-a1"), 2),
            "Fix a",
        ));
        s.apply(&sub(
            "SubagentStart",
            "b2",
            WT_B,
            Some("worktree-agent-b2"),
            3,
        ));
        assert_eq!(
            subtasks(&s),
            [
                line("worktree-agent-a1", Some("Fix a"), Some(Running)),
                line("worktree-agent-b2", None, Some(Running)),
            ]
        );
        // Leaving the branch Claude made for the worktree moves the row.
        s.apply(&sub("PostToolUse", "a1", WT_A, Some("fix/a"), 4));
        s.apply(&sub("PostToolUse", "b2", WT_B, Some("fix/b"), 5));
        // The main agent's Stop says nothing about its subagents.
        s.apply(&rec("Stop", 6));
        assert_eq!(state(&s), State::Idle);
        s.apply(&sub("SubagentStop", "a1", WT_A, Some("fix/a"), 7));
        assert_eq!(
            subtasks(&s),
            [
                line("fix/a", Some("Fix a"), Some(Done)),
                line("fix/b", None, Some(Running)),
            ]
        );
        assert_eq!(s.get("%1").unwrap().tasks[1].cwd, WT_B);
        // The session ends with b still at work.
        s.apply(&rec("SessionEnd", 8));
        assert_eq!(
            subtasks(&s),
            [
                line("fix/a", Some("Fix a"), Some(Done)),
                line("fix/b", None, Some(Stopped)),
            ]
        );
        let session = s.get("%1").unwrap();
        assert_eq!(
            session.cwd.as_deref(),
            Some("/tmp/repo"),
            "subagents never move the session"
        );
    }

    #[test]
    fn rows_keep_their_place_when_subagents_switch_branch() {
        let mut s = Sessions::default();
        s.apply(&sub(
            "SubagentStart",
            "a1",
            WT_A,
            Some("worktree-agent-a1"),
            1,
        ));
        s.apply(&sub(
            "SubagentStart",
            "b2",
            WT_B,
            Some("worktree-agent-b2"),
            2,
        ));
        s.apply(&sub("PostToolUse", "b2", WT_B, Some("fix/b"), 3));
        s.apply(&sub("PostToolUse", "a1", WT_A, Some("fix/a"), 4));
        let branches: Vec<&str> = tasks(&s).into_iter().map(|(b, _)| b).collect();
        assert_eq!(branches, ["fix/a", "fix/b"]);
    }

    #[test]
    fn a_subagent_waits_then_runs_again() {
        let mut s = Sessions::default();
        s.apply(&sub("SubagentStart", "a1", WT_A, Some("fix/a"), 1));
        s.apply(&sub("PermissionRequest", "a1", WT_A, None, 2));
        assert_eq!(
            subtasks(&s),
            [line("fix/a", None, Some(TaskState::Waiting))]
        );
        assert_eq!(state(&s), State::Waiting, "it blocks its session");
        s.apply(&sub("PostToolUse", "a1", WT_A, Some("fix/a"), 3));
        assert_eq!(
            subtasks(&s),
            [line("fix/a", None, Some(TaskState::Running))]
        );
    }

    #[test]
    fn a_description_seen_later_labels_the_row() {
        let mut s = Sessions::default();
        s.apply(&sub("SubagentStart", "a1", WT_A, Some("fix/a"), 1));
        s.apply(&described(
            sub("SubagentStop", "a1", WT_A, Some("fix/a"), 2),
            "Fix a",
        ));
        assert_eq!(
            subtasks(&s),
            [line("fix/a", Some("Fix a"), Some(TaskState::Done))]
        );
    }

    #[test]
    fn subagents_sharing_a_branch_share_one_row_labelled_by_the_latest() {
        use TaskState::{Done, Running};
        let mut s = Sessions::default();
        let start = |id, cwd, ts| sub("SubagentStart", id, cwd, Some("fix/x"), ts);
        s.apply(&described(start("a1", WT_A, 1), "First"));
        s.apply(&described(start("b2", WT_B, 2), "Second"));
        assert_eq!(subtasks(&s), [line("fix/x", Some("Second"), Some(Running))]);
        // Still running while any of them is.
        s.apply(&sub("SubagentStop", "b2", WT_B, Some("fix/x"), 3));
        assert_eq!(subtasks(&s), [line("fix/x", Some("Second"), Some(Running))]);
        s.apply(&sub("SubagentStop", "a1", WT_A, Some("fix/x"), 4));
        assert_eq!(subtasks(&s), [line("fix/x", Some("Second"), Some(Done))]);
    }

    #[test]
    fn subagents_on_the_main_agents_branch_or_the_default_get_no_row() {
        let mut s = Sessions::default();
        s.apply(&on("feat/main", "/tmp/repo", 1));
        // No worktree: the subagent works in the main agent's directory.
        s.apply(&described(
            sub("SubagentStart", "a1", "/tmp/repo", Some("feat/main"), 2),
            "Helper",
        ));
        s.apply(&sub("SubagentStart", "b2", WT_B, None, 3));
        s.apply(&sub("SubagentStop", "b2", WT_B, None, 4));
        assert_eq!(subtasks(&s), [line("feat/main", None, None)]);
    }

    #[test]
    fn a_new_conversation_keeps_the_terminals_tasks() {
        let mut s = Sessions::default();
        s.apply(&on("feat/a", "/r", 1));
        s.apply(&HookRecord {
            session_id: Some("new".into()),
            ..rec("SessionStart", 2)
        });
        assert_eq!(tasks(&s), [("feat/a", "/r")]);
        // Subagents of the old conversation never finish.
        s.apply(&sub("SubagentStart", "a1", WT_A, Some("fix/a"), 3));
        s.reconcile(10_000, &[agent("newer", "busy", Some("%1"))]);
        assert_eq!(
            subtasks(&s)[1],
            line("fix/a", None, Some(TaskState::Stopped))
        );
        assert_eq!(s.get("%1").unwrap().session_id.as_deref(), Some("newer"));
        assert_eq!(tasks(&s), [("feat/a", "/r"), ("fix/a", WT_A)]);
    }

    /// `%1`'s task rows' branches, if its session row is shown.
    fn shown(s: &Sessions) -> Option<Vec<&str>> {
        let (_, tasks) = s.shown("%1")?;
        Some(tasks.iter().map(|t| t.branch.as_str()).collect())
    }

    #[test]
    fn a_dismissed_session_row_stays_hidden_until_its_next_hook_event() {
        let mut s = Sessions::default();
        s.apply(&on("feat/a", "/r", 1));
        s.apply(&rec("Stop", 2));
        assert_eq!(shown(&s), Some(vec!["feat/a"]));
        s.dismiss("%1", None, 5);
        assert_eq!(shown(&s), None);
        // Snapshots of the same conversation do not bring it back.
        s.reconcile(10_000, &[agent(S1, "busy", Some("%1"))]);
        assert_eq!(shown(&s), None);
        // A hook event does, without the tasks dismissed with it.
        s.apply(&rec("UserPromptSubmit", 10_001));
        assert_eq!(shown(&s), Some(vec![]));
        s.apply(&on("feat/b", "/r", 10_002));
        s.apply(&on("feat/a", "/r", 10_003));
        assert_eq!(shown(&s), Some(vec!["feat/a", "feat/b"]));
    }

    #[test]
    fn a_new_conversation_found_by_snapshot_brings_a_dismissed_row_back() {
        let mut s = Sessions::default();
        s.apply(&rec("Stop", 1));
        s.dismiss("%1", None, 5);
        s.reconcile(10, &[agent("newer", "idle", Some("%1"))]);
        assert_eq!(shown(&s), Some(vec![]));
        // A terminal first seen after a dismissal of its pane id (from
        // before tmux reused it) is shown.
        s.dismiss("%7", None, 20);
        s.reconcile(30, &[agent("x", "idle", Some("%7"))]);
        assert!(s.shown("%7").is_some());
    }

    #[test]
    fn dismissing_a_task_row_hides_only_that_task() {
        let mut s = Sessions::default();
        s.apply(&on("feat/a", "/r", 1));
        s.apply(&on("feat/b", "/r", 2));
        s.dismiss("%1", Some("feat/a"), 5);
        assert_eq!(shown(&s), Some(vec!["feat/b"]));
        s.apply(&rec("Stop", 6));
        assert_eq!(shown(&s), Some(vec!["feat/b"]), "not an event of feat/a");
        s.apply(&on("feat/a", "/r", 7));
        assert_eq!(shown(&s), Some(vec!["feat/a", "feat/b"]));
    }

    #[test]
    fn a_dismissed_subagent_task_comes_back_only_with_that_subagents_events() {
        let mut s = Sessions::default();
        s.apply(&sub("SubagentStart", "a1", WT_A, Some("fix/a"), 1));
        s.apply(&sub("SubagentStop", "a1", WT_A, Some("fix/a"), 2));
        s.dismiss("%1", Some("fix/a"), 5);
        s.apply(&sub("SubagentStart", "b2", WT_B, Some("fix/b"), 6));
        assert_eq!(shown(&s), Some(vec!["fix/b"]));
        s.apply(&sub("PostToolUse", "a1", WT_A, None, 7));
        assert_eq!(shown(&s), Some(vec!["fix/a", "fix/b"]));
    }

    #[test]
    fn events_older_than_a_dismissal_stay_hidden_in_any_order() {
        // On reopen the dismissals are loaded before the log is replayed.
        let mut s = Sessions::default();
        s.dismiss("%1", Some("feat/a"), 5);
        s.dismiss("%1", Some("feat/a"), 3);
        s.apply(&on("feat/a", "/r", 1));
        s.apply(&on("feat/b", "/r", 2));
        assert_eq!(shown(&s), Some(vec!["feat/b"]));
        let want = Dismissal {
            pane: "%1".into(),
            branch: Some("feat/a".into()),
            at: 5,
        };
        assert_eq!(s.dismissals(), [want]);
        assert_eq!(s.panes().collect::<Vec<_>>(), ["%1"]);
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
