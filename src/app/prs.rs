//! The pull requests of the dashboard's task rows (ticket 36), read with
//! `gh` on a thread of their own, so a slow `gh` never holds up the
//! screen nor git's reads. One `gh pr list` per repository (its main
//! checkout: worktrees of one repository share it), cached for
//! `[github] refresh_secs`, and asked again at once when the dashboard
//! opens. While it is closed, nothing is asked. A repository `gh` cannot
//! answer for (no `gh`, logged out, no GitHub remote, failed, too slow)
//! has no PR status until it can, and logs one line to `gh.log` in the
//! state directory, not again until it has answered.

use super::watch::Watched;
use super::Event;
use crate::git;
use crate::github::{self, Pr, Prs};
use crate::hooks::now_ms;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// How often the thread looks whether a repository is due.
const TICK: Duration = Duration::from_secs(1);
/// The log of repositories `gh` could not answer for, in the state
/// directory.
pub const LOG_FILE: &str = "gh.log";

/// A task's PR as read, by `(directory, branch)`: `None` without one, or
/// when `gh` could not say.
pub type Read = Vec<((String, String), Option<Pr>)>;

/// One repository's last `gh pr list`: when (`None`: due again at once),
/// and what it gave (`None`: it failed).
struct Fetched {
    at: Option<Instant>,
    prs: Option<Prs>,
}

/// What the thread knows: each directory's repository and each
/// repository's pull requests.
pub struct Cache {
    refresh: Duration,
    /// Each task directory's main checkout; `None` outside a repository.
    repo_of: HashMap<String, Option<String>>,
    repos: HashMap<String, Fetched>,
    /// Repositories whose failure was logged since they last answered.
    logged: HashSet<String>,
}

impl Cache {
    pub fn new(refresh: Duration) -> Self {
        Self {
            refresh,
            repo_of: HashMap::new(),
            repos: HashMap::new(),
            logged: HashSet::new(),
        }
    }

    /// Everything is due again, and directories are placed again (the
    /// dashboard just opened).
    /// What was read stays shown until read again.
    pub fn expire(&mut self) {
        self.repo_of.clear();
        for f in self.repos.values_mut() {
            f.at = None;
        }
    }

    /// Ask `fetch` for each repository of `tasks` that is due (never read,
    /// or read `refresh` ago), placing directories with `resolve`. Returns
    /// whether anything was read, and the failures to log (once per
    /// repository until it answers again).
    pub fn refresh(
        &mut self,
        tasks: &[(String, String)],
        now: Instant,
        resolve: impl Fn(&str) -> Option<String>,
        fetch: impl Fn(&str) -> Result<Prs, String>,
    ) -> (bool, Vec<String>) {
        let mut repos: Vec<String> = Vec::new();
        for (dir, _) in tasks {
            let repo = self
                .repo_of
                .entry(dir.clone())
                .or_insert_with(|| resolve(dir))
                .clone();
            if let Some(repo) = repo.filter(|r| !repos.contains(r)) {
                repos.push(repo);
            }
        }
        let mut read = false;
        let mut failures = Vec::new();
        for repo in repos {
            let due = self
                .repos
                .get(&repo)
                .and_then(|f| f.at)
                .is_none_or(|at| now.saturating_duration_since(at) >= self.refresh);
            if !due {
                continue;
            }
            let prs = match fetch(&repo) {
                Ok(prs) => {
                    self.logged.remove(&repo);
                    Some(prs)
                }
                Err(e) => {
                    if self.logged.insert(repo.clone()) {
                        failures.push(format!("{repo}: {e}"));
                    }
                    None
                }
            };
            self.repos.insert(repo, Fetched { at: Some(now), prs });
            read = true;
        }
        (read, failures)
    }

    /// Each task's PR as last read.
    pub fn lookup(&self, tasks: &[(String, String)]) -> Read {
        tasks
            .iter()
            .map(|(dir, branch)| {
                let pr = self
                    .repo_of
                    .get(dir)
                    .cloned()
                    .flatten()
                    .and_then(|repo| self.repos.get(&repo))
                    .and_then(|f| f.prs.as_ref())
                    .and_then(|prs| prs.get(branch))
                    .cloned();
                ((dir.clone(), branch.clone()), pr)
            })
            .collect()
    }
}

/// `dir`'s repository: its main checkout.
fn repo_of(dir: &str) -> Option<String> {
    git::working_tree(dir).map(|t| t.main)
}

/// Append `lines` to the log at `path`, best effort.
fn log(path: &PathBuf, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let now = now_ms();
    let text: String = lines.iter().map(|l| format!("{now} {l}\n")).collect();
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
}

/// Start the thread, reading every `refresh` (none if zero: PR status is
/// off and `gh` never runs) and logging to `log_path`. It reads only while
/// the dashboard is open (`Watched::details`), and sends what it read as
/// [`Event::Prs`]. Returns where to send what is watched.
pub fn start(refresh: Duration, log_path: PathBuf, tx: &Sender<Event>) -> Option<Sender<Watched>> {
    if refresh.is_zero() {
        return None;
    }
    let (watch_tx, watch_rx) = mpsc::channel::<Watched>();
    let events = tx.clone();
    std::thread::spawn(move || {
        let mut cache = Cache::new(refresh);
        let mut watched = Watched::default();
        let mut sent: Option<Vec<(String, String)>> = None;
        loop {
            match watch_rx.recv_timeout(TICK) {
                Ok(first) => {
                    // Only the latest list matters, but a close and a
                    // reopen queued meanwhile (behind a slow gh) still
                    // count as the dashboard opening.
                    let mut closed = !watched.details;
                    let mut w = first;
                    for next in watch_rx.try_iter() {
                        closed |= !w.details;
                        w = next;
                    }
                    if w.details && closed {
                        cache.expire();
                    }
                    watched = w;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if !watched.details || watched.tasks.is_empty() {
                continue;
            }
            let tasks = &watched.tasks;
            let (read, failures) = cache.refresh(tasks, Instant::now(), repo_of, github::pr_list);
            log(&log_path, &failures);
            if read || sent.as_ref() != Some(tasks) {
                if events.send(Event::Prs(cache.lookup(tasks))).is_err() {
                    break;
                }
                sent = Some(tasks.clone());
            }
        }
    });
    Some(watch_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PrState;
    use std::cell::RefCell;

    fn pr(number: u64) -> Pr {
        Pr {
            number,
            state: PrState::Open,
            checks: None,
            review: None,
            url: String::new(),
        }
    }

    fn task(dir: &str, branch: &str) -> (String, String) {
        (dir.into(), branch.into())
    }

    /// Two worktrees of `/r` and a directory outside any repository.
    fn resolve(dir: &str) -> Option<String> {
        dir.starts_with("/r").then(|| "/r".to_string())
    }

    #[test]
    fn one_call_per_repository_then_none_until_due() {
        let mut cache = Cache::new(Duration::from_secs(60));
        let calls = RefCell::new(Vec::new());
        let fetch = |repo: &str| {
            calls.borrow_mut().push(repo.to_string());
            Ok(Prs::from([("feat/a".to_string(), pr(1))]))
        };
        let tasks = [
            task("/r", "feat/a"),
            task("/r/wt/b", "feat/b"),
            task("/tmp", "x"),
        ];
        let t0 = Instant::now();
        let (read, failures) = cache.refresh(&tasks, t0, resolve, fetch);
        assert!(read);
        assert!(failures.is_empty());
        assert_eq!(
            *calls.borrow(),
            ["/r"],
            "worktrees share their repository's call"
        );
        let got = cache.lookup(&tasks);
        assert_eq!(got[0].1, Some(pr(1)));
        assert_eq!(got[1].1, None, "no PR for feat/b");
        assert_eq!(got[2].1, None, "not a repository");

        let (read, _) = cache.refresh(&tasks, t0 + Duration::from_secs(59), resolve, fetch);
        assert!(!read);
        assert_eq!(calls.borrow().len(), 1, "cached");
        let (read, _) = cache.refresh(&tasks, t0 + Duration::from_secs(60), resolve, fetch);
        assert!(read);
        assert_eq!(calls.borrow().len(), 2, "due again");
        cache.expire();
        cache.refresh(&tasks, t0 + Duration::from_secs(61), resolve, fetch);
        assert_eq!(calls.borrow().len(), 3, "the dashboard opened: at once");
    }

    #[test]
    fn a_failing_repository_shows_no_pr_and_logs_once_until_it_answers() {
        let mut cache = Cache::new(Duration::from_secs(1));
        let tasks = [task("/r", "feat/a")];
        let t0 = Instant::now();
        let ok = |_: &str| Ok(Prs::from([("feat/a".to_string(), pr(1))]));
        let bad = |_: &str| Err("gh: not logged in".to_string());
        cache.refresh(&tasks, t0, resolve, ok);
        assert_eq!(cache.lookup(&tasks)[0].1, Some(pr(1)));
        let (_, failures) = cache.refresh(&tasks, t0 + Duration::from_secs(1), resolve, bad);
        assert_eq!(failures, ["/r: gh: not logged in"]);
        assert_eq!(cache.lookup(&tasks)[0].1, None, "git only");
        let (_, failures) = cache.refresh(&tasks, t0 + Duration::from_secs(2), resolve, bad);
        assert!(failures.is_empty(), "logged once");
        cache.refresh(&tasks, t0 + Duration::from_secs(3), resolve, ok);
        let (_, failures) = cache.refresh(&tasks, t0 + Duration::from_secs(4), resolve, bad);
        assert_eq!(failures.len(), 1, "again after it answered");
    }
}
