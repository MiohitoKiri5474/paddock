//! Sessions that need you: notifications when one starts waiting or its main
//! agent goes idle, `C-a a` to jump to the next one, and tab highlights.
//!
//! Only hook events notify: a `claude agents --json` snapshot can arrive
//! before the hook that reports the same change, and must neither swallow
//! nor repeat its notification.

use super::rows::Row;
use super::sidebar::Pick;
use super::state::{App, Mode};
use super::Event;
use crate::config::Notify;
use crate::hooks::now_ms;
use crate::sessions::{self, is_external, log, state_after, HookRecord, ProgramState, State};
use crate::terminal::program_status::Report;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The AppleScript that shows a notification; its message is the script's
/// first argument, so it needs no AppleScript quoting.
const NOTIFY_SCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 1 of argv) with title \"paddock\"",
    "end run",
];

/// How long a notification may take before it counts as failed.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to look whether the notifier finished.
const NOTIFY_POLL: Duration = Duration::from_millis(50);

/// Wait up to `timeout` for the notifier `child` (stderr piped) to finish.
/// Returns what it said on stderr if it failed; one that hangs is killed,
/// so waiting threads never pile up.
fn wait_notifier(mut child: Child, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(NOTIFY_POLL),
            Ok(None) => {
                // Best effort: it may have ended just now.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the notifier did not finish in {}s",
                    timeout.as_secs()
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut said = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        if let Err(e) = stderr.read_to_string(&mut said) {
            said = format!("{status}; cannot read its stderr: {e}");
        }
    }
    Err(Some(said.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| status.to_string()))
}

/// Overrides the command that shows notifications (tests use a stub).
const OSASCRIPT_VAR: &str = "PADDOCK_OSASCRIPT";

/// Why a session needs you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Waiting,
    Idle,
}

impl Notify {
    /// Whether the config lets `alert` show a notification.
    pub fn allows(self, alert: Alert) -> bool {
        match alert {
            Alert::Waiting => self.waiting,
            Alert::Idle => self.idle,
        }
    }
}

/// How long (ms) a status report from a terminal with no agy session yet
/// waits for one: a few log polls, so the `SessionStart` agy ran before it
/// is folded. Also how much earlier than its session's start a report may
/// be made and still belong to it.
const STATUS_WAIT_MS: u64 = 1_000;

/// agy's latest status report from a terminal with no agy session yet,
/// waiting for one (see [`App::record_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingStatus {
    pub state: ProgramState,
    /// When the report came (ms since the epoch).
    pub ts: u64,
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
            Some(State::Idle) if sessions::alerts_idle(r) => Alert::Idle,
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
        let loose = sessions::matches_any_conversation(r);
        let same =
            |s: &Option<String>| *s == session || (loose && (s.is_none() || session.is_none()));
        let repeat =
            !sessions::alerts_each_turn(r) && last.is_some_and(|(s, a)| same(s) && *a == alert);
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
    /// A failure to notify was shown; later ones are not, to avoid a notice
    /// on every notification.
    notify_failure_shown: bool,
    /// Which kinds of alert show a notification (`[notify]`).
    notify: Notify,
}

impl Attention {
    /// Notify the kinds `notify` allows, through `PADDOCK_OSASCRIPT` if set.
    pub fn from_env(notify: Notify) -> Self {
        Self {
            notify,
            alerts: Alerts::default(),
            started: now_ms(),
            window_focused: true,
            osascript: std::env::var(OSASCRIPT_VAR)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "osascript".into()),
            notify_failure_shown: false,
        }
    }

    /// The notice for a notification that failed with `error`: the first
    /// time only.
    pub fn notify_failed(&mut self, error: &str) -> Option<String> {
        if self.notify_failure_shown {
            return None;
        }
        self.notify_failure_shown = true;
        Some(format!("cannot notify: {error}"))
    }
}

/// The row after the focused terminal's among those waiting or idle, in
/// sidebar order and wrapping around; the first one if it is not among them.
/// External sessions are skipped: there is no terminal to jump to.
pub fn next_needing<'a>(rows: &'a [Row], focused: Option<&str>) -> Option<&'a Row> {
    let needy: Vec<&Row> = rows
        .iter()
        .filter(|r| r.state.needs_you() && !r.external)
        .collect();
    let at = needy.iter().position(|r| Some(r.pane.as_str()) == focused);
    let next = at.map_or(0, |i| (i + 1) % needy.len());
    needy.get(next).copied()
}

impl App {
    /// Apply recorded hook events and notify about the ones that need you.
    /// Events from floats are left out: paddock does not follow agents there.
    pub(super) fn hooks(&mut self, records: &[HookRecord]) {
        let followed: Vec<&HookRecord> = records
            .iter()
            .filter(|r| !self.is_float_pane(&r.pane))
            .collect();
        for r in followed {
            // A status report with no agy session to go to has no row to
            // alert about (agy reports its sign-in screen before any hook).
            if !self.sessions.apply(r) && r.event == sessions::STATUS {
                continue;
            }
            let Some(alert) = self.attention.alerts.on_hook(r) else {
                continue;
            };
            // `[notify]` turns off only the notification: `sessions.apply`
            // above has already updated the state, markers and dashboard.
            // A hidden External group (`[dashboard] external = false`)
            // notifies of nothing either.
            let hidden = is_external(&r.pane) && !self.external.shown;
            if !self.attention.notify.allows(alert)
                || hidden
                || r.ts < self.attention.started
                || self.focused_on(&r.pane)
            {
                continue;
            }
            self.notify(&r.pane, alert);
        }
    }

    /// Status reports `reports` came from pane `pane`: Antigravity's go
    /// to the server's hook log, so they fold in order with its hooks
    /// (whose records reach the log first when they happened first), and
    /// replay with them when paddock opens again. They name no
    /// conversation (agy's reports do not say), so they belong to whichever
    /// the pane runs when folded. Only a report for a live agy session in
    /// the pane, saying something new, is recorded.
    ///
    /// A hook reaches the app up to a log poll after it ran, so agy's
    /// first report may come before its `SessionStart` is folded: the
    /// latest report of a pane with no agy session yet waits for one (see
    /// [`App::retry_status`]).
    pub(super) fn record_status(&mut self, pane: &str, reports: &[Report]) {
        let ts = now_ms();
        for r in reports {
            if r.app.as_deref() != Some(sessions::AGY_APP) {
                continue;
            }
            if self.sessions.agy_live_in(pane) {
                self.pending_status.remove(pane);
                self.write_status(pane, r.state, ts);
            } else {
                let pending = PendingStatus { state: r.state, ts };
                self.pending_status.insert(pane.to_string(), pending);
            }
        }
    }

    /// Go over the reports waiting for their pane's agy session (see
    /// [`App::record_status`]), in this order:
    /// - the pane runs a live agy session that started at most
    ///   [`STATUS_WAIT_MS`] after the report came: it is recorded, after
    ///   the session's latest record;
    /// - the pane runs a live agy session that started later than that:
    ///   the report is about an earlier one, and is dropped;
    /// - no live agy session there yet: it waits on until it is
    ///   [`STATUS_WAIT_MS`] old, then is dropped.
    pub(super) fn retry_status(&mut self) {
        let now = now_ms();
        let pending = std::mem::take(&mut self.pending_status);
        for (pane, p) in pending {
            let ours = p.ts.saturating_add(STATUS_WAIT_MS);
            if self.sessions.agy_live_by(&pane, ours) {
                // Never before a record already folded, so a replay of the
                // log folds it where it was folded now.
                let after = self.sessions.last_heard(&pane).unwrap_or(0);
                self.write_status(&pane, p.state, p.ts.max(after));
            } else if !self.sessions.agy_live_in(&pane)
                && now.saturating_sub(p.ts) <= STATUS_WAIT_MS
            {
                self.pending_status.insert(pane, p);
            }
        }
    }

    /// Append agy's report `state` from `pane`, made at `ts`, to the
    /// server's log if it says something new; it counts as said once
    /// written.
    fn write_status(&mut self, pane: &str, state: ProgramState, ts: u64) {
        let Some(path) = self.events_log.clone() else {
            return;
        };
        if !self.sessions.is_new_status(pane, state) {
            return;
        }
        let record = sessions::status_record(pane, state, ts);
        match log::append(&path, &record) {
            Ok(()) => self.sessions.note_status(pane, state),
            Err(e) => self.notice = Some(format!("{e:#}")),
        }
    }

    /// Tell the watcher whether an Antigravity session runs in a terminal.
    pub(super) fn note_agy_running(&self) {
        let running = self.sessions.agy_in_terminals();
        self.agy_running.store(running, Ordering::Relaxed);
    }

    /// `C-a a`: jump to the next session that needs you.
    pub(super) fn next_attention(&mut self) {
        let rows = self.rows();
        let focused = self.focused_window().map(|w| w.active.as_str());
        match next_needing(&rows, focused) {
            Some(row) => {
                self.jump_to(&row.pane);
                self.mode = Mode::Normal;
            }
            None => self.notice = Some("no session needs you".into()),
        }
    }

    /// Turns done in the tab you are looking at (the current space's
    /// current tab, with the outer window focused and no float over it)
    /// become idle; while the tab is zoomed, only the zoomed split's, as
    /// the others are hidden.
    pub(super) fn see_current(&mut self) {
        if !self.looking_at_tab() {
            return;
        }
        let Some(window) = self.current_space().and_then(|i| i.current_window()) else {
            return;
        };
        let panes: Vec<String> = window.shown_panes().map(|p| p.id.clone()).collect();
        for pane in panes {
            self.sessions.see(&pane);
        }
    }

    /// An external session has no tab to look at: its finished turn turns
    /// idle once you look at its row instead, selected in the dashboard, or
    /// in AGENTS while the sidebar has the keyboard, with the outer window
    /// focused and no float over it (as [`Self::looking_at_tab`] asks).
    pub(super) fn see_selected_external(&mut self) {
        if !self.looking_at_tab() {
            return;
        }
        let key = match &self.mode {
            Mode::Dashboard(d) => self.dashboard_row(d).map(|r| r.pane),
            Mode::Sidebar(s) => match s.anchored(&self.sidebar_picks()).pick {
                Some(Pick::Agent(key)) => Some(key),
                _ => None,
            },
            _ => None,
        };
        if let Some(key) = key.filter(|k| is_external(k)) {
            self.sessions.see(&key);
        }
    }

    /// Whether you are looking at the terminal with pane `pane`: it is
    /// focused, paddock's window has the focus, and no float is over it.
    fn focused_on(&self, pane: &str) -> bool {
        self.looking_at_tab() && self.is_current_pane(pane)
    }

    /// Whether you are looking at the current tab: paddock's window has
    /// the focus and no float is shown (the float has the keyboard then).
    fn looking_at_tab(&self) -> bool {
        self.attention.window_focused && self.float_pane().is_none()
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
        let mut command = Command::new(&self.attention.osascript);
        for line in NOTIFY_SCRIPT {
            command.args(["-e", line]);
        }
        let child = command
            .arg(&message)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        match child {
            // Wait for it without holding up the event loop; a failure comes
            // back as an event.
            Ok(child) => {
                let events = self.events.clone();
                std::thread::spawn(move || {
                    if let Err(error) = wait_notifier(child, NOTIFY_TIMEOUT) {
                        // Nobody listens once paddock quits.
                        let _ = events.send(Event::NotifyFailed(error));
                    }
                });
            }
            Err(e) => {
                if let Some(notice) = self.attention.notify_failed(&e.to_string()) {
                    self.notice = Some(notice);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::AgentKind;

    fn agy_rec(event: &str) -> HookRecord {
        HookRecord {
            kind: AgentKind::Agy,
            ..rec(event, "s")
        }
    }

    fn agy_status(state: &str) -> HookRecord {
        let state = ProgramState::parse(state).unwrap();
        sessions::status_record("%1", state, 0)
    }

    #[test]
    fn agys_idle_reports_never_alert_without_a_finished_turn() {
        // Started, then quit without a prompt: agy reports idle both times.
        let got = alerts(&[
            agy_rec("SessionStart"),
            agy_status("idle"),
            agy_status("idle"),
        ]);
        assert_eq!(got, [None, None, None]);
        let busy = HookRecord {
            fully_idle: Some(false),
            ..agy_rec("Stop")
        };
        assert_eq!(alerts(&[agy_rec("PreInvocation"), busy]), [None, None]);
    }

    #[test]
    fn claude_after_agy_in_a_pane_alerts_as_ever() {
        // agy's turn ends (a report naming no conversation), then Claude
        // runs in the same terminal: its first idle reminder is new.
        let idle_prompt = HookRecord {
            notification_type: Some("idle_prompt".into()),
            ..rec("Notification", "c")
        };
        let got = alerts(&[
            agy_rec("PreInvocation"),
            agy_status("done"),
            rec("SessionStart", "c"),
            idle_prompt.clone(),
            idle_prompt,
        ]);
        assert_eq!(
            got,
            [None, Some(Alert::Idle), None, Some(Alert::Idle), None]
        );
    }

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
    fn the_notification_script_compiles() {
        // `osacompile` checks the AppleScript without running it; skip
        // where there is none (not macOS).
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let out = dir.path().join("notify.scpt");
        let mut cmd = Command::new("osacompile");
        for line in NOTIFY_SCRIPT {
            cmd.args(["-e", line]);
        }
        let Ok(result) = cmd.arg("-o").arg(&out).output() else {
            return;
        };
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn sh(script: &str) -> std::process::Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    #[test]
    fn a_notifier_that_succeeds_is_fine() {
        assert_eq!(wait_notifier(sh("true"), Duration::from_secs(5)), Ok(()));
    }

    #[test]
    fn a_notifier_that_fails_gives_what_it_said() {
        let got = wait_notifier(
            sh("echo 'syntax error' >&2; exit 1"),
            Duration::from_secs(5),
        );
        assert_eq!(got, Err("syntax error".to_string()));
    }

    #[test]
    fn a_notifier_that_hangs_is_killed_and_counts_as_failed() {
        let started = std::time::Instant::now();
        let got = wait_notifier(sh("sleep 30"), Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(5), "it was killed");
        assert!(got.unwrap_err().contains("did not finish"));
    }

    #[test]
    fn a_failing_notifier_is_reported_once() {
        let mut a = Attention::from_env(Notify::default());
        let first = a.notify_failed("osascript: syntax error");
        assert_eq!(
            first.as_deref(),
            Some("cannot notify: osascript: syntax error")
        );
        assert_eq!(a.notify_failed("again"), None);
    }

    #[test]
    fn each_kind_of_notification_follows_its_switch() {
        let only_waiting = Notify {
            waiting: true,
            idle: false,
        };
        assert!(only_waiting.allows(Alert::Waiting));
        assert!(!only_waiting.allows(Alert::Idle));
        let only_idle = Notify {
            waiting: false,
            idle: true,
        };
        assert!(!only_idle.allows(Alert::Waiting));
        assert!(only_idle.allows(Alert::Idle));
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
    fn an_agy_turn_reported_twice_alerts_once() {
        let agy = |event: &str| HookRecord {
            kind: sessions::AgentKind::Agy,
            ..rec(event, "s")
        };
        let stop = HookRecord {
            fully_idle: Some(true),
            ..agy("Stop")
        };
        // A status report names no conversation: it is the pane's.
        let status = |state: &str| {
            let state = ProgramState::parse(state).unwrap();
            sessions::status_record("%1", state, 0)
        };
        let got = alerts(&[
            agy("SessionStart"),
            agy("PreInvocation"),
            status("working"),
            status("blocked"),
            status("working"),
            status("done"),
            stop.clone(),
            status("idle"),
            agy("PreInvocation"),
            stop,
            status("done"),
        ]);
        use Alert::*;
        assert_eq!(
            got,
            [
                None,
                None,
                None,
                Some(Waiting),
                None,
                Some(Idle),
                None,
                None,
                None,
                Some(Idle),
                None
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
            kind: crate::sessions::AgentKind::Claude,
            state,
            tasks: Vec::new(),
            external: false,
        }
    }

    #[test]
    fn next_needing_skips_external_sessions() {
        let ext = Row {
            external: true,
            ..row("e", State::Waiting)
        };
        let rows = [ext.clone(), row("i", State::Idle)];
        let next = |focused| next_needing(&rows, focused).map(|r| r.pane.as_str());
        assert_eq!(next(None), Some("i"));
        assert_eq!(next(Some("i")), Some("i"));
        assert!(next_needing(&[ext], None).is_none());
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
