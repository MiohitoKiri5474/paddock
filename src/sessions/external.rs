//! External sessions: Claude running outside paddock's terminals. They
//! have no pane, so they are keyed by [`external_key`] of their session id
//! and fold like any terminal's session. They leave when they end, or once
//! nothing was heard of them for a while (their Claude may have been killed
//! without a `SessionEnd`).

use super::{Agent, HookRecord, Session, Sessions};

/// What an external session's key starts with: never a tmux pane id (`%N`).
const EXTERNAL: &str = "ext:";

/// The key of the external session with id `session_id`.
pub fn external_key(session_id: &str) -> String {
    format!("{EXTERNAL}{session_id}")
}

/// Whether `key` is an external session's (see [`external_key`]), not a
/// pane's.
pub fn is_external(key: &str) -> bool {
    key.starts_with(EXTERNAL)
}

/// Whether `r` ends an external session: any `SessionEnd` of its main
/// agent. It is keyed by its id, and after `/clear` the new conversation is
/// another session, so this one ends.
pub(super) fn ends(r: &HookRecord) -> bool {
    is_external(&r.pane) && r.event == "SessionEnd" && r.agent_id.is_none()
}

impl Sessions {
    /// A session seen in a terminal (resumed in paddock) is not external:
    /// a pane's event drops the external session of the same id.
    pub(super) fn leave_external(&mut self, r: &HookRecord) {
        if let (false, Some(id)) = (is_external(&r.pane), &r.session_id) {
            self.forget_external(id);
        }
    }

    /// The key of the session `claude agents --json` lists as `a`: the pane
    /// already holding it, else the pane it runs in, else (in no terminal of
    /// paddock's) its external key. Found in a pane, it is no longer external.
    pub(super) fn agent_key(&mut self, a: &Agent) -> String {
        let id = a.session_id.as_str();
        let known = self
            .by_pane
            .iter()
            .find(|(p, s)| !is_external(p) && s.session_id.as_deref() == Some(id))
            .map(|(p, _)| p.clone());
        let key = known
            .or_else(|| a.pane.clone())
            .unwrap_or_else(|| external_key(id));
        if !is_external(&key) {
            self.forget_external(id);
        }
        key
    }

    /// Drop the external session with id `session_id`: it runs in a
    /// terminal of paddock's now.
    pub(super) fn forget_external(&mut self, session_id: &str) {
        let key = external_key(session_id);
        if self.by_pane.remove(&key).is_some() {
            self.seen.retain(|(k, _), _| *k != key);
        }
    }

    /// The keys of the external sessions to show at `now`: those
    /// [`Sessions::shown`] shows, heard of (see [`Session::alive`]) at most
    /// `timeout` ms ago. A session killed without a `SessionEnd` leaves
    /// this way.
    pub fn externals(&self, now: u64, timeout: u64) -> Vec<&str> {
        self.by_pane
            .iter()
            .filter(|(k, s)| is_external(k) && now <= s.alive.saturating_add(timeout))
            .filter(|(k, _)| self.shown(k).is_some())
            .map(|(k, _)| k.as_str())
            .collect()
    }

    /// Forget external sessions quiet for longer than `timeout` ms at
    /// `now`, ended or not, so they do not pile up. An ended one is kept
    /// until then, so hooks landing after its `SessionEnd` stay ignored.
    pub fn prune_externals(&mut self, now: u64, timeout: u64) {
        let quiet = |s: &Session| now > s.alive.saturating_add(timeout);
        self.by_pane.retain(|k, s| !(is_external(k) && quiet(s)));
        let kept = &self.by_pane;
        self.seen
            .retain(|(k, _), _| !is_external(k) || kept.contains_key(k));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{agent, rec, state, S1};
    use super::super::{label, State, CLEAR};
    use super::*;

    /// An external session's event, keyed as the app reads it.
    fn ext(event: &str, session: &str, ts: u64) -> HookRecord {
        HookRecord {
            event: event.into(),
            pane: external_key(session),
            session_id: Some(session.into()),
            cwd: Some("/w/api".into()),
            ts,
            ..Default::default()
        }
    }

    const MIN: u64 = 60_000;

    #[test]
    fn external_keys_never_look_like_panes() {
        assert!(is_external(&external_key(S1)));
        assert!(!is_external("%1"));
        assert!(!is_external(""));
    }

    #[test]
    fn external_sessions_fold_like_terminals_keyed_by_session_id() {
        let mut s = Sessions::default();
        s.apply(&ext("SessionStart", "e1", 1));
        s.apply(&ext("UserPromptSubmit", "e2", 2));
        s.apply(&HookRecord {
            branch: Some("feat/x".into()),
            ..ext("PermissionRequest", "e1", 3)
        });
        let e1 = s.get(&external_key("e1")).unwrap();
        assert_eq!(e1.state, State::Waiting);
        assert_eq!(e1.alive, 3);
        assert_eq!(tasks_of(&s, &external_key("e1")), [("feat/x", "/w/api")]);
        assert_eq!(s.get(&external_key("e2")).unwrap().state, State::Running);
        let mut shown = s.externals(3, 10 * MIN);
        shown.sort_unstable();
        assert_eq!(shown, [external_key("e1"), external_key("e2")]);
    }

    fn tasks_of<'a>(s: &'a Sessions, key: &str) -> Vec<(&'a str, &'a str)> {
        s.get(key)
            .unwrap()
            .tasks
            .iter()
            .map(|t| (t.branch.as_str(), t.cwd.as_str()))
            .collect()
    }

    #[test]
    fn an_external_session_ends_on_any_session_end_even_clear() {
        let mut s = Sessions::default();
        s.apply(&ext("SessionStart", "e1", 1));
        s.apply(&HookRecord {
            reason: Some(CLEAR.into()),
            ..ext("SessionEnd", "e1", 2)
        });
        assert!(s.get(&external_key("e1")).unwrap().ended());
        assert!(s.externals(2, 10 * MIN).is_empty());
        // A late hook of the ended session stays ignored.
        s.apply(&ext("PostToolUse", "e1", 3));
        assert!(s.externals(3, 10 * MIN).is_empty());
    }

    #[test]
    fn a_quiet_external_session_leaves_after_the_timeout() {
        let mut s = Sessions::default();
        s.apply(&ext("Stop", "e1", 1));
        assert_eq!(s.externals(1 + 10 * MIN, 10 * MIN).len(), 1);
        assert!(s.externals(2 + 10 * MIN, 10 * MIN).is_empty());
        // Listed by `claude agents --json`, it is still alive.
        s.reconcile(5 * MIN, &[agent("e1", "idle", None)]);
        assert_eq!(s.externals(14 * MIN, 10 * MIN).len(), 1);
        assert!(s.externals(16 * MIN, 10 * MIN).is_empty());
        s.prune_externals(14 * MIN, 10 * MIN);
        assert!(
            s.get(&external_key("e1")).is_some(),
            "not quiet long enough"
        );
        s.prune_externals(16 * MIN, 10 * MIN);
        assert!(s.get(&external_key("e1")).is_none());
    }

    #[test]
    fn snapshots_find_sessions_outside_paddock_as_external() {
        let mut s = Sessions::default();
        s.reconcile(
            10,
            &[Agent {
                name: Some("fix-login".into()),
                ..agent("e1", "busy", None)
            }],
        );
        let e1 = s.get(&external_key("e1")).unwrap();
        assert_eq!((e1.state, e1.alive), (State::Running, 10));
        assert_eq!(label(e1, None), "fix-login");
        assert_eq!(s.externals(10, MIN), [external_key("e1")]);
        // A hook of the same session updates the same row.
        s.apply(&ext("PermissionRequest", "e1", 20));
        assert_eq!(s.get(&external_key("e1")).unwrap().state, State::Waiting);
    }

    #[test]
    fn a_session_seen_in_a_terminal_is_no_longer_external() {
        let mut s = Sessions::default();
        s.apply(&ext("Stop", S1, 1));
        // Resumed in a paddock terminal: its hooks name the pane.
        s.apply(&rec("SessionStart", 2));
        assert!(s.get(&external_key(S1)).is_none());
        assert!(s.externals(2, MIN).is_empty());
        // A snapshot finding it in a pane does the same.
        s.apply(&ext("Stop", "e2", 3));
        s.reconcile(4, &[agent("e2", "busy", Some("%2"))]);
        assert!(s.get(&external_key("e2")).is_none());
        assert_eq!(s.get("%2").unwrap().state, State::Running);
        // And a snapshot without a pane keeps updating the pane's session.
        s.reconcile(10_000, &[agent(S1, "busy", None)]);
        assert_eq!(state(&s), State::Running);
        assert!(s.get(&external_key(S1)).is_none());
    }

    #[test]
    fn a_dismissed_external_session_stays_hidden_until_its_next_event() {
        let mut s = Sessions::default();
        s.apply(&ext("Stop", "e1", 1));
        s.dismiss(&external_key("e1"), None, 2);
        assert!(s.externals(2, MIN).is_empty());
        s.reconcile(3, &[agent("e1", "idle", None)]);
        assert!(s.externals(3, MIN).is_empty(), "still the same session");
        s.apply(&ext("UserPromptSubmit", "e1", 4));
        assert_eq!(s.externals(4, MIN).len(), 1);
    }
}
