//! Sessions that need you: notifications when one starts waiting or its main
//! agent goes idle, `C-a a` to jump to the next one, and tab highlights.
//!
//! Only hook events notify: a `claude agents --json` snapshot can arrive
//! before the hook that reports the same change, and must neither swallow
//! nor repeat its notification.

use super::rows::Row;
use super::state::{App, Mode};
use crate::hooks::now_ms;
use crate::sessions::{self, state_after, HookRecord, State};
use std::collections::HashMap;
use std::process::{Command, Stdio};

/// Overrides the command that shows notifications (tests use a stub).
const OSASCRIPT_VAR: &str = "PADDOCK_OSASCRIPT";

/// Why a session needs you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Waiting,
    Idle,
}

/// The last alert of each terminal's session, so that one wait or one
/// finished turn notifies once however many hooks report it.
#[derive(Debug, Default)]
pub struct Alerts {
    last: HashMap<String, (Option<String>, Alert)>,
}

impl Alerts {
    /// The alert a hook event raises, if it is new.
    pub fn on_hook(&mut self, r: &HookRecord) -> Option<Alert> {
        let alert = match state_after(r) {
            Some(State::Waiting) => Alert::Waiting,
            // A new session starting idle has not finished anything.
            Some(State::Idle) if r.event != "SessionStart" => Alert::Idle,
            _ => {
                // Work went on: the next alert is new. A subagent working on
                // ends its wait, but not the main agent's finished turn.
                let main = HookRecord {
                    agent_id: None,
                    ..r.clone()
                };
                if state_after(&main) == Some(State::Running)
                    && (r.agent_id.is_none()
                        || self
                            .last
                            .get(&r.pane)
                            .is_some_and(|l| l.1 == Alert::Waiting))
                {
                    self.last.remove(&r.pane);
                }
                return None;
            }
        };
        let last = self.last.get(&r.pane);
        let session = r
            .session_id
            .clone()
            .or_else(|| last.and_then(|(s, _)| s.clone()));
        // Stop ends one turn each time; other idle events only remind of it.
        let repeat = r.event != "Stop" && last.is_some_and(|(s, a)| *s == session && *a == alert);
        self.last.insert(r.pane.clone(), (session, alert));
        (!repeat).then_some(alert)
    }
}

/// What the app knows to decide whether to notify.
pub struct Attention {
    alerts: Alerts,
    /// When paddock started (ms): earlier events happened while it was closed.
    started: u64,
    /// Whether the outer window has focus, as far as the terminal reports
    /// focus changes (CSI ?1004); assumed until it says otherwise.
    pub window_focused: bool,
    osascript: String,
}

impl Attention {
    pub fn from_env() -> Self {
        Self {
            alerts: Alerts::default(),
            started: now_ms(),
            window_focused: true,
            osascript: std::env::var(OSASCRIPT_VAR)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "osascript".into()),
        }
    }
}

/// The row after the focused terminal's among those waiting or idle, in
/// sidebar order and wrapping around; the first one if it is not among them.
pub fn next_needing<'a>(rows: &'a [Row], focused: Option<&str>) -> Option<&'a Row> {
    let needy: Vec<&Row> = rows.iter().filter(|r| r.state.needs_you()).collect();
    let at = needy.iter().position(|r| Some(r.pane.as_str()) == focused);
    let next = at.map_or(0, |i| (i + 1) % needy.len());
    needy.get(next).copied()
}

impl App {
    /// Apply recorded hook events and notify about the ones that need you.
    pub(super) fn hooks(&mut self, records: &[HookRecord]) {
        for r in records {
            self.sessions.apply(r);
            let Some(alert) = self.attention.alerts.on_hook(r) else {
                continue;
            };
            if r.ts < self.attention.started || self.focused_on(&r.pane) {
                continue;
            }
            self.notify(&r.pane, alert);
        }
    }

    /// `C-a a`: jump to the next session that needs you.
    pub(super) fn next_attention(&mut self) {
        let rows = self.rows();
        let focused = self.current_pane();
        match next_needing(&rows, focused.as_deref()) {
            Some(row) => {
                self.jump_to(&row.pane);
                self.mode = Mode::Normal;
            }
            None => self.notice = Some("no session needs you".into()),
        }
    }

    /// Turns done in the tab you are looking at (the current space's
    /// current tab, with the outer window focused) become idle.
    pub(super) fn see_current(&mut self) {
        if !self.attention.window_focused {
            return;
        }
        let Some(window) = self.current_space().and_then(|i| i.current_window()) else {
            return;
        };
        let panes: Vec<String> = window.panes.iter().map(|p| p.id.clone()).collect();
        for pane in panes {
            self.sessions.see(&pane);
        }
    }

    /// Whether you are looking at the terminal with pane `pane`.
    fn focused_on(&self, pane: &str) -> bool {
        self.attention.window_focused && self.current_pane().is_some_and(|p| p == pane)
    }

    fn notify(&mut self, pane: &str, alert: Alert) {
        let label = match self.rows().into_iter().find(|r| r.pane == pane) {
            Some(row) => row.label,
            None => match self.sessions.get(pane) {
                Some(s) => sessions::label(s, None),
                None => return,
            },
        };
        let what = match alert {
            Alert::Waiting => "waiting",
            Alert::Idle => "idle",
        };
        let message = format!("{label} is {what}");
        // The message goes in as an argument: no AppleScript quoting needed.
        let child = Command::new(&self.attention.osascript)
            .args(["-e", "on run argv"])
            .args([
                "-e",
                "display notification (space 1 of argv) with title \"paddock\"",
            ])
            .args(["-e", "end run", &message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            // Reap it without holding up the event loop.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => self.notice = Some(format!("cannot notify: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(event: &str, session: &str) -> HookRecord {
        HookRecord {
            event: event.into(),
            pane: "%1".into(),
            session_id: Some(session.into()),
            ..Default::default()
        }
    }

    fn alerts(events: &[HookRecord]) -> Vec<Option<Alert>> {
        let mut a = Alerts::default();
        events.iter().map(|r| a.on_hook(r)).collect()
    }

    fn notification(kind: &str) -> HookRecord {
        HookRecord {
            notification_type: Some(kind.into()),
            ..rec("Notification", "s")
        }
    }

    #[test]
    fn a_wait_and_a_finished_turn_alert_once_each() {
        let got = alerts(&[
            rec("SessionStart", "s"),
            rec("UserPromptSubmit", "s"),
            rec("PermissionRequest", "s"),
            notification("permission_prompt"),
            rec("PostToolUse", "s"),
            rec("Stop", "s"),
            notification("idle_prompt"),
            rec("UserPromptSubmit", "s"),
            rec("Stop", "s"),
        ]);
        use Alert::*;
        assert_eq!(
            got,
            [
                None,
                None,
                Some(Waiting),
                None,
                None,
                Some(Idle),
                None,
                None,
                Some(Idle)
            ]
        );
    }

    #[test]
    fn a_denied_request_ends_in_idle() {
        let got = alerts(&[rec("PermissionRequest", "s"), rec("Stop", "s")]);
        assert_eq!(got, [Some(Alert::Waiting), Some(Alert::Idle)]);
    }

    #[test]
    fn subagents_alert_when_they_wait_and_reset_when_they_work_on() {
        let sub = |event| HookRecord {
            agent_id: Some("a1".into()),
            ..rec(event, "s")
        };
        let got = alerts(&[
            sub("PermissionRequest"),
            sub("PostToolUse"),
            sub("PermissionRequest"),
            sub("Stop"),
        ]);
        assert_eq!(
            got,
            [Some(Alert::Waiting), None, Some(Alert::Waiting), None]
        );
    }

    #[test]
    fn a_subagent_working_on_after_the_turn_does_not_repeat_idle() {
        let sub = HookRecord {
            agent_id: Some("a1".into()),
            ..rec("PostToolUse", "s")
        };
        let got = alerts(&[
            rec("Stop", "s"),
            sub.clone(),
            notification("idle_prompt"),
            sub,
            // A new turn, e.g. started by a finished background agent.
            rec("Stop", "s"),
        ]);
        assert_eq!(
            got,
            [Some(Alert::Idle), None, None, None, Some(Alert::Idle)]
        );
    }

    #[test]
    fn a_new_conversation_starts_over() {
        let got = alerts(&[
            rec("Stop", "old"),
            rec("SessionStart", "new"),
            rec("Stop", "new"),
        ]);
        assert_eq!(got, [Some(Alert::Idle), None, Some(Alert::Idle)]);
    }

    fn row(pane: &str, state: State) -> Row {
        Row {
            pane: pane.into(),
            space: "$1".into(),
            window: "@1".into(),
            label: pane.into(),
            state,
            tasks: Vec::new(),
            closed: false,
        }
    }

    #[test]
    fn next_needing_cycles_waiting_and_idle_rows() {
        let rows = [
            row("w", State::Waiting),
            row("i", State::Idle),
            row("r", State::Running),
        ];
        let next = |focused| next_needing(&rows, focused).map(|r| r.pane.as_str());
        assert_eq!(next(Some("w")), Some("i"));
        assert_eq!(next(Some("i")), Some("w"));
        assert_eq!(next(Some("r")), Some("w"));
        assert_eq!(next(None), Some("w"));
        let running = [row("r", State::Running)];
        assert!(next_needing(&running, None).is_none());
    }
}
