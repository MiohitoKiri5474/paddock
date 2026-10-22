//! What paddock reads from GitHub, through the `gh` CLI: each branch's
//! pull request, with its state, CI checks and review (ticket 36). One
//! `gh pr list` per repository; nothing is cached here (the dashboard's
//! thread caches it, see `app::prs`), so a command can ask once and go.
//! Without `gh`, logged out, with no GitHub remote, or when `gh` fails or
//! takes longer than git's reads, there is no PR status: never an error
//! on screen. paddock changes nothing on GitHub; `p` on the dashboard only
//! asks `gh` to open a PR in the browser.

use crate::git;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Overrides the `gh` paddock runs (tests use a stub).
const GH_VAR: &str = "PADDOCK_GH";
/// The newest pull requests asked for, whatever their state. Each brings
/// its checks, which GitHub is slow to roll up: on a busy repository 20
/// take about 3.5 s, 30 near 5 s (the deadline), 50 time out.
// ponytail: a branch whose PR is older than the newest 20 of the
// repository shows git's `merged` only. The upgrade is a second call by
// `--head` for the branches not found.
const LIMIT: u32 = 20;
/// The fields read from each pull request.
const FIELDS: &str = "number,state,isDraft,headRefName,statusCheckRollup,reviewDecision,url";
/// How long `gh` may take, as a git read.
const TIMEOUT: Duration = git::READ_TIMEOUT;
/// How long opening a PR in the browser may take before `gh` is let go.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// A pull request's state, a draft being an open one not ready yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrState {
    /// How the dashboard writes it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// Its CI checks rolled up: any failing fails them, else any still to
/// finish keeps them pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    Passing,
    Failing,
    Pending,
}

impl Checks {
    /// Its mark: `✓`, `✗` or `•`.
    pub fn mark(self) -> &'static str {
        match self {
            Self::Passing => "✓",
            Self::Failing => "✗",
            Self::Pending => "•",
        }
    }
}

/// Its review decision (`reviewDecision`), when the repository asks for
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Review {
    Approved,
    ChangesRequested,
    Required,
}

impl Review {
    /// How the dashboard writes it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes requested",
            Self::Required => "review required",
        }
    }
}

/// A branch's pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub state: PrState,
    /// `None` without checks.
    pub checks: Option<Checks>,
    /// `None` without a review decision.
    pub review: Option<Review>,
    /// Its page. Not shown on the dashboard (`p` asks gh by number, as
    /// ticket 36 says); `paddock worktree list --json` gives it (ticket 37).
    pub url: String,
}

/// Each branch's newest pull request, by branch name.
pub type Prs = HashMap<String, Pr>;

/// A pull request as `gh pr list --json` gives it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    number: u64,
    state: String,
    #[serde(default)]
    is_draft: bool,
    head_ref_name: String,
    #[serde(default)]
    status_check_rollup: Vec<RawCheck>,
    /// `""` when none, as gh writes it; `null` taken the same.
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    url: String,
}

/// One entry of `statusCheckRollup`: a check run (GitHub Actions and
/// apps: `status`, then `conclusion` once completed) or a commit status
/// (`state`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCheck {
    #[serde(rename = "__typename", default)]
    typename: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    workflow_name: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    started_at: String,
}

impl RawCheck {
    /// What makes two entries the same check run again (a re-run, or one
    /// workflow run per event): its workflow and name, or its context.
    fn key(&self) -> (&str, &str, &str) {
        (&self.typename, &self.workflow_name, self.name_or_context())
    }

    fn name_or_context(&self) -> &str {
        if self.context.is_empty() {
            &self.name
        } else {
            &self.context
        }
    }

    /// Where this one entry stands, as gh's own `pr status` reads it.
    fn outcome(&self) -> Checks {
        let state = if self.typename == "StatusContext" || !self.state.is_empty() {
            self.state.as_str()
        } else if self.status == "COMPLETED" {
            self.conclusion.as_str()
        } else {
            // QUEUED, IN_PROGRESS, WAITING, PENDING, REQUESTED.
            return Checks::Pending;
        };
        match state {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => Checks::Passing,
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => Checks::Failing,
            // PENDING, EXPECTED, STALE, and anything new.
            _ => Checks::Pending,
        }
    }
}

/// `checks` rolled up, each check counted once, by its latest run: a
/// stale failure re-run since does not fail them. `None` without checks.
fn rollup(checks: &[RawCheck]) -> Option<Checks> {
    let mut latest: HashMap<(&str, &str, &str), &RawCheck> = HashMap::new();
    for c in checks {
        let slot = latest.entry(c.key()).or_insert(c);
        // RFC 3339 times in UTC sort as text.
        if c.started_at > slot.started_at {
            *slot = c;
        }
    }
    let outcomes: Vec<Checks> = latest.values().map(|c| c.outcome()).collect();
    if outcomes.is_empty() {
        None
    } else if outcomes.contains(&Checks::Failing) {
        Some(Checks::Failing)
    } else if outcomes.contains(&Checks::Pending) {
        Some(Checks::Pending)
    } else {
        Some(Checks::Passing)
    }
}

impl RawPr {
    fn into_pr(self) -> Option<Pr> {
        let state = match (self.state.as_str(), self.is_draft) {
            ("OPEN", true) => PrState::Draft,
            ("OPEN", false) => PrState::Open,
            ("MERGED", _) => PrState::Merged,
            ("CLOSED", _) => PrState::Closed,
            _ => return None,
        };
        let review = match self.review_decision.as_deref() {
            Some("APPROVED") => Some(Review::Approved),
            Some("CHANGES_REQUESTED") => Some(Review::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(Review::Required),
            _ => None,
        };
        Some(Pr {
            number: self.number,
            state,
            checks: rollup(&self.status_check_rollup),
            review,
            url: self.url,
        })
    }
}

/// `gh pr list --json` output as each branch's newest pull request (the
/// highest number), whatever order gh lists them in.
pub fn parse(json: &str) -> Result<Prs, String> {
    let raw: Vec<RawPr> = serde_json::from_str(json).map_err(|e| format!("gh's JSON: {e}"))?;
    let mut prs = Prs::new();
    for r in raw {
        let branch = r.head_ref_name.clone();
        let Some(pr) = r.into_pr() else {
            continue;
        };
        if prs.get(&branch).is_none_or(|old| old.number < pr.number) {
            prs.insert(branch, pr);
        }
    }
    Ok(prs)
}

/// The `gh` to run: `PADDOCK_GH` if set, else `gh` on `PATH`.
fn gh() -> String {
    std::env::var(GH_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "gh".into())
}

/// `gh args…` in `dir`, never asking anything: no stdin, no prompts, no
/// update notice. `GH_REPO` is removed: set, gh would ask that one
/// repository whatever `dir` is (`GH_HOST` stays, for GitHub Enterprise).
fn command(dir: &str, args: &[&str]) -> Command {
    let mut command = Command::new(gh());
    command
        .args(args)
        .current_dir(dir)
        .env_remove("GH_REPO")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// The pull requests of the repository `dir` is in, by branch: one
/// `gh pr list` of the newest [`LIMIT`], waiting at most [`TIMEOUT`].
/// Fails with why (no `gh`, logged out, no GitHub remote, too slow).
pub fn pr_list(dir: &str) -> Result<Prs, String> {
    let limit = LIMIT.to_string();
    let args = [
        "pr", "list", "--state", "all", "--limit", &limit, "--json", FIELDS,
    ];
    let mut command = command(dir, &args);
    command.stdout(Stdio::piped());
    let out = git::wait(command, TIMEOUT)?;
    parse(&out)
}

/// Open PR `number` of the repository `dir` is in, in the browser
/// (`gh pr view --web`), without waiting for it: on a thread of its own,
/// waiting at most [`OPEN_TIMEOUT`], then handing the outcome to `done`.
pub fn open_web(dir: &str, number: u64, done: impl FnOnce(Result<(), String>) + Send + 'static) {
    let n = number.to_string();
    let mut command = command(dir, &["pr", "view", "--web", &n]);
    command.stdout(Stdio::null());
    std::thread::spawn(move || {
        let result = git::wait(command, OPEN_TIMEOUT).map(drop);
        done(result);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gh");

    fn fixture(name: &str) -> Prs {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        parse(&text).unwrap()
    }

    #[test]
    fn a_branch_with_several_prs_gets_its_newest() {
        let prs = fixture("paddock.json");
        assert_eq!(prs["develop"].number, 37, "#37 over #25");
        assert_eq!(prs["develop"].state, PrState::Merged);
        let agy = &prs["feat/agy"];
        assert_eq!(agy.number, 41);
        assert_eq!(agy.checks, Some(Checks::Passing));
        assert_eq!(agy.review, None, "\"\" is no decision");
        assert_eq!(
            agy.url,
            "https://github.com/MiohitoKiri5474/paddock/pull/41"
        );
        // Listed oldest first, the newest still wins.
        let text = std::fs::read_to_string(format!("{FIXTURES}/paddock.json")).unwrap();
        let mut raw: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        raw.reverse();
        let prs = parse(&serde_json::to_string(&raw).unwrap()).unwrap();
        assert_eq!(prs["develop"].number, 37);
    }

    #[test]
    fn states_checks_and_reviews_are_read_from_captured_json() {
        let prs = fixture("mixed.json");
        let at = |branch: &str| prs[branch].clone();
        let pending = at("fix/342820-shallow-plugin-clones");
        assert_eq!(pending.state, PrState::Open);
        assert_eq!(pending.checks, Some(Checks::Pending), "a queued run");
        assert_eq!(pending.review, Some(Review::Required));
        let failing = at("agents/fix-workspace-filter-and-issues-342635-342633");
        assert_eq!(failing.checks, Some(Checks::Failing));
        assert_eq!(failing.review, Some(Review::Approved));
        let status_fail = at("automated-cherry-pick-of-#143285-upstream-release-1.37");
        assert_eq!(status_fail.checks, Some(Checks::Failing), "a commit status");
        assert_eq!(status_fail.review, None);
        assert_eq!(at("fsquota-projid-errors").checks, Some(Checks::Pending));
        let draft = at("kep-5075-ga");
        assert_eq!(draft.state, PrState::Draft);
        assert_eq!(
            at("fix/issue-create-resolve-type-before-create").checks,
            Some(Checks::Passing),
            "skipped counts as passing"
        );
        let closed = at("agents/continue-archived-remote-chats");
        assert_eq!(closed.state, PrState::Closed);
        assert_eq!(closed.review, Some(Review::ChangesRequested));
        assert_eq!(closed.checks, Some(Checks::Passing));
        let none = at("fix/kubelet-targeted-mountinfo");
        assert_eq!(none.checks, None, "no checks: nothing");
        assert_eq!(none.state, PrState::Closed);
    }

    /// A one-PR list on `feat/x` with these rollup entries.
    fn with_checks(checks: &str) -> Pr {
        let json = format!(
            r#"[{{"number":7,"state":"OPEN","isDraft":false,"headRefName":"feat/x",
                "reviewDecision":null,"url":"u","statusCheckRollup":[{checks}]}}]"#
        );
        parse(&json).unwrap().remove("feat/x").unwrap()
    }

    #[test]
    fn a_check_counts_by_its_latest_run() {
        let run = |at: &str, conclusion: &str| {
            format!(
                r#"{{"__typename":"CheckRun","name":"test","workflowName":"CI",
                    "status":"COMPLETED","conclusion":"{conclusion}","startedAt":"{at}"}}"#
            )
        };
        let rerun = format!(
            "{},{}",
            run("2026-10-01T10:00:00Z", "FAILURE"),
            run("2026-10-01T11:00:00Z", "SUCCESS")
        );
        assert_eq!(with_checks(&rerun).checks, Some(Checks::Passing));
        let broke = format!(
            "{},{}",
            run("2026-10-01T11:00:00Z", "FAILURE"),
            run("2026-10-01T10:00:00Z", "SUCCESS")
        );
        assert_eq!(with_checks(&broke).checks, Some(Checks::Failing));
        assert_eq!(with_checks("").review, None, "null is no decision");
        assert_eq!(with_checks("").checks, None);
        let expected = r#"{"__typename":"StatusContext","context":"ci","state":"EXPECTED"}"#;
        assert_eq!(with_checks(expected).checks, Some(Checks::Pending));
        let timed_out =
            r#"{"__typename":"CheckRun","name":"a","status":"COMPLETED","conclusion":"TIMED_OUT"}"#;
        assert_eq!(with_checks(timed_out).checks, Some(Checks::Failing));
    }

    #[test]
    fn gh_never_inherits_gh_repo() {
        // Set or not here, the command removes it.
        let c = command("/tmp", &["pr", "list"]);
        let gh_repo = c.get_envs().find(|(k, _)| *k == "GH_REPO");
        assert_eq!(gh_repo, Some((std::ffi::OsStr::new("GH_REPO"), None)));
        assert!(c.get_envs().all(|(k, _)| k != "GH_HOST"), "GH_HOST is kept");
        assert_eq!(c.get_current_dir(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn json_gh_did_not_write_is_an_error() {
        assert!(parse("not json").is_err());
        assert_eq!(parse("[]").unwrap(), Prs::new());
    }
}
