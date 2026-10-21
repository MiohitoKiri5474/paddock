//! The hook event log: `<state>/events/<tmux server pid>.jsonl`, one JSON
//! record per line. Hooks append with a single `write` each, so parallel
//! hooks never interleave. One file per server, because tmux reuses pane ids
//! after the server restarts.
//!
//! Events of external sessions (Claude outside paddock's terminals) have no
//! server: they go to `<state>/events/external.jsonl`, without a pane. The
//! app compacts that file when it opens, since every Claude on the machine
//! appends to it.

use super::{external_key, HookRecord};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub fn path(state_dir: &Path, server_pid: u32) -> PathBuf {
    state_dir.join("events").join(format!("{server_pid}.jsonl"))
}

/// The log of external sessions' events.
pub fn external_path(state_dir: &Path) -> PathBuf {
    state_dir.join("events").join("external.jsonl")
}

/// An external session's record as the app folds it: keyed by the
/// session's [`external_key`]. `None` for one without a session id, or
/// with a pane (it belongs in a server's log).
pub fn keyed_external(r: HookRecord) -> Option<HookRecord> {
    if !r.pane.is_empty() {
        return None;
    }
    let pane = external_key(r.session_id.as_deref()?);
    Some(HookRecord { pane, ..r })
}

/// A server log's new records and the external log's, in the order they
/// happened (by `ts`; each log's own order breaks ties), the external ones
/// keyed (see [`keyed_external`]). Replaying one log after the other would
/// let an old external record bring back a session since seen in a pane.
pub fn merged(server: Vec<HookRecord>, external: Vec<HookRecord>) -> Vec<HookRecord> {
    let mut all = server;
    all.extend(external.into_iter().filter_map(keyed_external));
    // Stable: records with the same `ts` keep their log's order.
    all.sort_by_key(|r| r.ts);
    all
}

/// How big the external log may grow before [`compact`] trims it.
pub const EXTERNAL_MAX_BYTES: u64 = 4 << 20;

/// How long (ms) [`compact`] keeps records.
pub const EXTERNAL_KEEP_MS: u64 = 24 * 60 * 60 * 1000;

/// Keep only the records of the last `keep` ms before `now` in the log at
/// `path`, once it is bigger than `max` bytes. The kept lines replace the
/// file in one rename; a hook appending meanwhile may lose its line, which
/// the next hook or `claude agents --json` makes up for.
pub fn compact(path: &Path, max: u64, now: u64, keep: u64) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > max => {}
        _ => return Ok(()),
    }
    let text = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let since = now.saturating_sub(keep);
    let mut kept = Vec::new();
    for line in text.split(|&b| b == b'\n') {
        let recent = serde_json::from_slice::<HookRecord>(line).is_ok_and(|r| r.ts >= since);
        if recent {
            kept.extend_from_slice(line);
            kept.push(b'\n');
        }
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, kept).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// The terminal a hook runs in, from `$TMUX` (`socket,pid,index`) and
/// `$TMUX_PANE`: `(server pid, pane id)`, or `None` unless the socket is
/// paddock's (`socket` is its name).
pub fn terminal(tmux: &str, pane: &str, socket: &str) -> Option<(u32, String)> {
    let mut parts = tmux.rsplitn(3, ',');
    let _index = parts.next()?;
    let pid = parts.next()?.parse().ok()?;
    let path = parts.next()?;
    let ours = Path::new(path).file_name()? == socket;
    let pane_ok = pane.starts_with('%') && pane.len() > 1;
    (ours && pane_ok).then(|| (pid, pane.to_string()))
}

/// The record for a hook payload (JSON on stdin). An unreadable payload still
/// records the event, without its details.
pub fn record(event: &str, pane: &str, payload: &str, ts: u64) -> HookRecord {
    let v: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    HookRecord {
        event: event.to_string(),
        pane: pane.to_string(),
        session_id: field("session_id"),
        cwd: field("cwd"),
        source: field("source"),
        notification_type: field("notification_type"),
        reason: field("reason"),
        agent_id: field("agent_id"),
        tool_name: field("tool_name"),
        branch: None,
        description: description(&v),
        ts,
    }
}

/// A subagent's description as its payload gives it: `SubagentStop` lists
/// the session's background tasks, the subagent among them.
fn description(v: &Value) -> Option<String> {
    let id = v.get("agent_id")?;
    v.get("background_tasks")?
        .as_array()?
        .iter()
        .find(|t| t.get("id") == Some(id))?
        .get("description")?
        .as_str()
        .map(str::to_string)
}

/// The description in a subagent's `agent-<id>.meta.json`, which Claude
/// keeps next to the session's transcript (`<transcript>.jsonl` →
/// `<transcript>/subagents/`).
pub fn meta_description(transcript: &str, agent_id: &str) -> Option<String> {
    if agent_id.is_empty() || !agent_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let dir = Path::new(transcript.strip_suffix(".jsonl")?).join("subagents");
    let text = std::fs::read_to_string(dir.join(format!("agent-{agent_id}.meta.json"))).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("description")?.as_str().map(str::to_string)
}

/// Append one record as one line, in a single write.
pub fn append(path: &Path, r: &HookRecord) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut line = serde_json::to_vec(r)?;
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let n = f.write(&line)?;
    if n != line.len() {
        bail!("short write to {} ({n} of {})", path.display(), line.len());
    }
    Ok(())
}

/// Reads records as hooks add them, keeping a partial last line until it ends.
pub struct Reader {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Reader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
        }
    }

    /// Records written since the last call. A missing file has none.
    pub fn read_new(&mut self) -> Vec<HookRecord> {
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(self.offset)).is_err() || f.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        self.offset += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let complete: Vec<u8> = self.partial.drain(..=end).collect();
        complete
            .split(|&b| b == b'\n')
            .filter_map(|l| serde_json::from_slice(l).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_paddocks_socket_counts() {
        let t = "/private/tmp/tmux-501/paddock,4242,0";
        assert_eq!(terminal(t, "%3", "paddock"), Some((4242, "%3".into())));
        assert_eq!(terminal("/tmp/tmux-501/default,1,0", "%3", "paddock"), None);
        assert_eq!(terminal(t, "", "paddock"), None);
        assert_eq!(terminal("", "%3", "paddock"), None);
        assert_eq!(terminal("garbage", "%3", "paddock"), None);
    }

    #[test]
    fn an_external_record_has_no_pane_and_is_keyed_by_its_session_once_read() {
        let r = record("Stop", "", r#"{"session_id":"e1","cwd":"/w"}"#, 5);
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("pane"), "{line}");
        let back: HookRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(back, r);
        let keyed = keyed_external(back).unwrap();
        assert_eq!(keyed.pane, external_key("e1"));
        assert_eq!(keyed.cwd.as_deref(), Some("/w"));
        assert_eq!(keyed_external(record("Stop", "", "{}", 5)), None, "no id");
        assert_eq!(
            keyed_external(record("Stop", "%1", r#"{"session_id":"e1"}"#, 5)),
            None,
            "a terminal's"
        );
    }

    #[test]
    fn old_log_lines_read_as_before() {
        let line = r#"{"event":"Stop","pane":"%3","session_id":"s","cwd":"/r","ts":9}"#;
        let r: HookRecord = serde_json::from_str(line).unwrap();
        assert_eq!(r.pane, "%3");
        assert_eq!(serde_json::to_string(&r).unwrap(), line, "written the same");
    }

    #[test]
    fn external_events_are_appended_and_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let p = external_path(dir.path());
        assert_eq!(p, dir.path().join("events/external.jsonl"));
        append(&p, &record("SessionStart", "", r#"{"session_id":"e1"}"#, 1)).unwrap();
        append(&p, &record("Stop", "", r#"{"session_id":"e1"}"#, 2)).unwrap();
        let got: Vec<(String, String)> = Reader::new(p)
            .read_new()
            .into_iter()
            .filter_map(keyed_external)
            .map(|r| (r.event, r.pane))
            .collect();
        let key = external_key("e1");
        assert_eq!(
            got,
            [("SessionStart".into(), key.clone()), ("Stop".into(), key)]
        );
    }

    #[test]
    fn compacting_keeps_recent_records_once_the_log_is_big() {
        let dir = tempfile::tempdir().unwrap();
        let p = external_path(dir.path());
        for ts in [1, 50, 100] {
            append(&p, &record("Stop", "", r#"{"session_id":"e1"}"#, ts)).unwrap();
        }
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"garbage\n").unwrap();
        let before = std::fs::read(&p).unwrap();
        compact(&p, 1 << 20, 100, 60).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), before, "small: untouched");
        compact(&p, 10, 100, 60).unwrap();
        let ts: Vec<u64> = Reader::new(p.clone())
            .read_new()
            .iter()
            .map(|r| r.ts)
            .collect();
        assert_eq!(ts, [50, 100]);
        compact(&dir.path().join("none.jsonl"), 0, 1, 1).unwrap();
    }

    #[test]
    fn records_keep_only_what_paddock_uses() {
        let payload = r#"{"session_id":"s","cwd":"/r","hook_event_name":"Notification",
            "notification_type":"permission_prompt","message":"big","agent_id":"a",
            "tool_name":"Bash"}"#;
        let r = record("Notification", "%1", payload, 7);
        assert_eq!(r.tool_name.as_deref(), Some("Bash"));
        assert_eq!(r.branch, None, "the hook reads git, not the payload");
        assert_eq!(r.session_id.as_deref(), Some("s"));
        assert_eq!(r.notification_type.as_deref(), Some("permission_prompt"));
        assert_eq!(r.agent_id.as_deref(), Some("a"));
        assert!(!serde_json::to_string(&r).unwrap().contains("big"));
        let bad = record("Stop", "%1", "not json", 8);
        assert_eq!(bad.event, "Stop");
        assert_eq!(bad.session_id, None);
    }

    const SPIKE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/spikes/02-subagent-worktree/fixtures"
    );

    #[test]
    fn a_subagents_description_comes_from_its_stop_payload() {
        let stop = std::fs::read_to_string(format!("{SPIKE}/SubagentStop-subagent.json")).unwrap();
        let r = record("SubagentStop", "%1", &stop, 1);
        assert_eq!(
            r.description.as_deref(),
            Some("Create fix/a branch and a.txt")
        );
        let start =
            std::fs::read_to_string(format!("{SPIKE}/SubagentStart-subagent.json")).unwrap();
        assert_eq!(record("SubagentStart", "%1", &start, 1).description, None);
        // The main agent's Stop lists subagents too, but describes none itself.
        let main = std::fs::read_to_string(format!("{SPIKE}/Stop.json")).unwrap();
        assert_eq!(record("Stop", "%1", &main, 1).description, None);
    }

    #[test]
    fn a_subagents_description_is_read_from_its_meta_file() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("s1.jsonl");
        let transcript = transcript.to_str().unwrap();
        std::fs::create_dir_all(dir.path().join("s1/subagents")).unwrap();
        std::fs::write(
            dir.path().join("s1/subagents/agent-a1.meta.json"),
            r#"{"agentType":"general-purpose","description":"Fix a","finished":false}"#,
        )
        .unwrap();
        assert_eq!(meta_description(transcript, "a1").as_deref(), Some("Fix a"));
        assert_eq!(meta_description(transcript, "b2"), None, "no meta file");
        assert_eq!(meta_description(transcript, "../a1"), None);
        assert_eq!(meta_description("/no/transcript", "a1"), None);
    }

    #[test]
    fn the_reader_returns_new_complete_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(dir.path(), 9);
        let mut reader = Reader::new(p.clone());
        assert!(reader.read_new().is_empty(), "no file yet");
        append(&p, &record("Stop", "%1", "{}", 1)).unwrap();
        append(&p, &record("SessionEnd", "%1", "{}", 2)).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(br#"{"event":"Stop","pane":"%2","#).unwrap();
        let got: Vec<String> = reader.read_new().into_iter().map(|r| r.event).collect();
        assert_eq!(got, ["Stop", "SessionEnd"]);
        f.write_all(b"\"ts\":3}\n").unwrap();
        let rest = reader.read_new();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].pane, "%2");
        assert!(reader.read_new().is_empty());
    }
}
