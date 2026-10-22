//! Reads program status reports (OSC 7501) from pane output. A program
//! writes `ESC ] 7501 ; <key>=<value>:<key>=<value>... ST` to say whether it
//! is working, blocked on you, done or failed; ST is `ESC \` or BEL. The
//! keys are those of the spec (Rex's "program status" page, after Mitchell
//! Hashimoto's proposal): `state` (required), `app`, `kind`, `id`,
//! `progress`, `title` and `msg`; paddock keeps `state`, `app` and `kind`.
//! The program's support query, `ESC ] 7501 ; ? ST`, is not a report.
//!
//! The scanner only reads: the bytes still go to the emulator, which draws
//! nothing for an OSC it does not know.

/// The longest sequence the spec allows; a longer one is dropped whole.
const MAX_LEN: usize = 4096;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// What an OSC 7501 body starts with.
const PREFIX: &[u8] = b"7501;";

/// The states a report may have.
const STATES: &[&str] = &["idle", "working", "done", "blocked", "error", "clear"];

/// One status report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// `idle`, `working`, `done`, `blocked`, `error` or `clear`.
    pub state: String,
    /// The program that sent it (`app`), if it said.
    pub app: Option<String>,
    /// What a `blocked` program waits for (`permission`, `question`,
    /// `auth`), if it said.
    pub kind: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// Saw `ESC`.
    Esc,
    /// Inside an OSC; its body is in `body`.
    Osc,
    /// Saw `ESC` inside an OSC: `\` ends it, anything else aborts it.
    OscEsc,
}

/// Remembers a sequence split across chunks.
#[derive(Debug, Default)]
pub struct StatusScanner {
    state: State,
    body: Vec<u8>,
    /// The OSC outgrew [`MAX_LEN`]: it is dropped at its end.
    too_long: bool,
}

impl StatusScanner {
    /// The reports completed in `bytes`, in order.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<Report> {
        let mut reports = Vec::new();
        for &b in bytes {
            self.state = match (self.state, b) {
                (State::Ground, ESC) | (State::Esc, ESC) => State::Esc,
                (State::Ground, _) => State::Ground,
                (State::Esc, b']') => self.start(),
                (State::Esc, _) => State::Ground,
                (State::Osc, BEL) => {
                    reports.extend(self.finish());
                    State::Ground
                }
                (State::Osc, ESC) => State::OscEsc,
                (State::Osc, _) => {
                    self.push(b);
                    State::Osc
                }
                (State::OscEsc, b'\\') => {
                    reports.extend(self.finish());
                    State::Ground
                }
                // Not ST: the OSC is cut short and this byte follows an ESC.
                (State::OscEsc, b']') => self.start(),
                (State::OscEsc, ESC) => State::Esc,
                (State::OscEsc, _) => State::Ground,
            };
        }
        reports
    }

    fn start(&mut self) -> State {
        self.body.clear();
        self.too_long = false;
        State::Osc
    }

    fn push(&mut self, b: u8) {
        // Only an OSC 7501 is kept past its first bytes.
        let other = self.body.len() >= PREFIX.len() && !self.body.starts_with(PREFIX);
        if self.too_long || other {
            return;
        }
        if self.body.len() >= MAX_LEN {
            self.too_long = true;
            self.body.clear();
            return;
        }
        self.body.push(b);
    }

    fn finish(&mut self) -> Option<Report> {
        let body = std::mem::take(&mut self.body);
        if self.too_long {
            return None;
        }
        parse(body.strip_prefix(PREFIX)?)
    }
}

/// The report in an OSC 7501 body (after `7501;`): `None` for the support
/// query, or without a known `state`. Pairs are `:`-separated `key=value`;
/// a malformed pair is skipped, and of a repeated key the last wins.
fn parse(body: &[u8]) -> Option<Report> {
    let body = std::str::from_utf8(body).ok()?;
    let mut state = None;
    let mut app = None;
    let mut kind = None;
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let value = Some(value.trim().to_string());
        match key.trim() {
            "state" => state = value,
            "app" => app = value,
            "kind" => kind = value,
            _ => {}
        }
    }
    let state = state.filter(|s| STATES.contains(&s.as_str()))?;
    Some(Report { state, app, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What agy 1.4.3 wrote, captured from a real run (throwaway home,
    /// `AGY_CLI_OSC7501=on`): its sign-in screen, then quitting.
    const SIGN_IN: &[u8] =
        b"\x1b]7501;state=blocked:kind=auth:app=antigravity:msg=U2lnbiBpbiByZXF1aXJlZA==\x07";
    const QUIT: &[u8] = b"\x1b]7501;state=idle:app=antigravity\x07";
    /// agy's support query, written when the variable is not set.
    const QUERY: &[u8] = b"\x1b]7501;?\x07";

    fn report(state: &str, app: Option<&str>, kind: Option<&str>) -> Report {
        Report {
            state: state.into(),
            app: app.map(str::to_string),
            kind: kind.map(str::to_string),
        }
    }

    #[test]
    fn reads_agys_reports() {
        let mut s = StatusScanner::default();
        let mut bytes = b"\x1b[?1049h\x1b]11;?\x07hello".to_vec();
        bytes.extend_from_slice(SIGN_IN);
        bytes.extend_from_slice(b"\x1b[2J");
        bytes.extend_from_slice(QUIT);
        assert_eq!(
            s.scan(&bytes),
            [
                report("blocked", Some("antigravity"), Some("auth")),
                report("idle", Some("antigravity"), None),
            ]
        );
    }

    #[test]
    fn a_report_split_anywhere_is_read_once() {
        let mut all = QUERY.to_vec();
        all.extend_from_slice(b"text");
        all.extend_from_slice(SIGN_IN);
        all.extend_from_slice(b"\x1b[m");
        for cut in 0..=all.len() {
            let mut s = StatusScanner::default();
            let mut got = s.scan(&all[..cut]);
            got.extend(s.scan(&all[cut..]));
            assert_eq!(
                got,
                [report("blocked", Some("antigravity"), Some("auth"))],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn st_ends_a_report_as_bel_does() {
        let mut s = StatusScanner::default();
        let got = s.scan(b"\x1b]7501;state=working:app=antigravity\x1b\\");
        assert_eq!(got, [report("working", Some("antigravity"), None)]);
    }

    #[test]
    fn the_query_and_other_oscs_are_no_reports() {
        let mut s = StatusScanner::default();
        assert!(s.scan(QUERY).is_empty());
        assert!(s
            .scan(b"\x1b]0;state=done\x07\x1b]75010;state=done\x07")
            .is_empty());
        assert!(
            s.scan(b"\x1b]7501;state=sleeping\x07").is_empty(),
            "no such state"
        );
        assert!(s.scan(b"\x1b]7501;app=x\x07").is_empty(), "no state");
    }

    #[test]
    fn pairs_follow_the_spec() {
        let mut s = StatusScanner::default();
        let got = s.scan(b"\x1b]7501; state = working :junk:state=done:app=brew\x07");
        assert_eq!(got, [report("done", Some("brew"), None)], "last wins");
    }

    #[test]
    fn an_osc_cut_short_by_another_escape_is_dropped() {
        let mut s = StatusScanner::default();
        let got = s.scan(b"\x1b]7501;state=done\x1b[m\x1b]7501;state=working\x07");
        assert_eq!(got, [report("working", None, None)]);
    }

    #[test]
    fn a_report_too_long_is_dropped_whole() {
        let mut s = StatusScanner::default();
        let mut long = b"\x1b]7501;state=done:msg=".to_vec();
        long.extend(std::iter::repeat_n(b'A', MAX_LEN));
        long.push(BEL);
        assert!(s.scan(&long).is_empty());
        assert_eq!(
            s.scan(QUIT),
            [report("idle", Some("antigravity"), None)],
            "the next one is read"
        );
    }
}
