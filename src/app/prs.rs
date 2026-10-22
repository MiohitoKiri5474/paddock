//! The pull requests of the dashboard's task rows (ticket 36) and of the
//! sidebar's spaces (ticket 37), read with `gh` on a thread of their own,
//! so a slow `gh` never holds up the screen nor git's reads. One `gh pr
//! list` per repository (its main checkout: worktrees of one repository
//! share it), cached for `[github] refresh_secs`, and asked again when
//! the dashboard opens if what was read is older than [`REOPEN_FLOOR`].
//! Repositories are asked one after another (each up to gh's 5 s limit);
//! a pass stops early once the dashboard closes or nothing shows PRs.
//! While the sidebar is hidden and the dashboard closed, nothing is
//! asked. A
//! repository `gh` cannot answer for (no `gh`, logged out, no GitHub
//! remote, failed, too slow) has no PR status until it can, and logs one
//! line to `gh.log` in the state directory, not again until it has
//! answered.

use super::watch::Tasks;
use super::Event;
use crate::git;
use crate::github::{self, Pr, Prs};
use crate::hooks::now_ms;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// How often the thread looks whether a repository is due.
const TICK: Duration = Duration::from_secs(1);
/// Opening the dashboard asks again only for what was read longer ago
/// than this, so toggling it does not run gh each time.
const REOPEN_FLOOR: Duration = Duration::from_secs(10);
/// The log of repositories `gh` could not answer for, in the state
/// directory.
pub const LOG_FILE: &str = "gh.log";

/// Tasks' PRs as read, by `(directory, branch)`: `None` without one, or
/// when `gh` could not say.
pub type PrUpdate = Vec<((String, String), Option<Pr>)>;

/// One repository's last `gh pr list`: when, whether it is due at once
/// (the dashboard opened), and what it gave (`None`: it failed).
struct Fetched {
    at: Instant,
    due: bool,
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

    /// The dashboard opened at `now`: directories are placed again, and
    /// what was read [`REOPEN_FLOOR`] ago or more is due at once. What was
    /// read stays shown until read again.
    pub fn opened(&mut self, now: Instant) {
        self.repo_of.clear();
        for f in self.repos.values_mut() {
            f.due |= now.saturating_duration_since(f.at) >= REOPEN_FLOOR;
        }
    }

    /// Ask `fetch` for each repository of `tasks` that is due by `clock`
    /// (never read,
    /// read `refresh` ago, or marked by [`Self::opened`]), one after
    /// another, placing directories with `resolve`; before each, stop if
    /// `stop` says so. Returns whether anything was read, and the failures
    /// to log (once per repository until it answers again).
    pub fn refresh(
        &mut self,
        tasks: &[(String, String)],
        clock: impl Fn() -> Instant,
        resolve: impl Fn(&str) -> Option<String>,
        fetch: impl Fn(&str) -> Result<Prs, String>,
        mut stop: impl FnMut() -> bool,
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
                .is_none_or(|f| f.due || clock().saturating_duration_since(f.at) >= self.refresh);
            if !due {
                continue;
            }
            if stop() {
                break;
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
            let fetched = Fetched {
                // When this one's read finished: a slow one before it
                // does not make it due early.
                at: clock(),
                due: false,
                prs,
            };
            self.repos.insert(repo, fetched);
            read = true;
        }
        (read, failures)
    }

    /// Each task's PR as last read.
    pub fn lookup(&self, tasks: &[(String, String)]) -> PrUpdate {
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
fn log(path: &Path, lines: &[String]) {
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

/// What shows PRs now: the open dashboard's tasks and, while the sidebar
/// shows, each space's directory and branch; empty while neither shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wanted {
    /// `(directory, branch)` pairs, each once.
    pub tasks: Tasks,
    /// The dashboard is open (opening it asks again past the floor).
    pub dashboard: bool,
}

/// Take `inbox` (what came, oldest first) into `wanted`: the latest list
/// wins, and the dashboard opened if it is open now and was closed before
/// or anywhere in between (a close and a reopen queued behind a slow gh
/// still count).
fn take(wanted: &mut Wanted, inbox: Vec<Wanted>, cache: &mut Cache) {
    let mut closed = !wanted.dashboard;
    for w in inbox {
        if w.dashboard && closed {
            cache.opened(Instant::now());
        }
        closed = !w.dashboard;
        *wanted = w;
    }
}

/// Whether a pass reading for `now` should stop before its next repository,
/// told `latest`: nothing shows PRs any more, or the dashboard closed
/// (what is left to read may be its tasks only; the next pass reads what
/// still shows).
fn stops(now: &Wanted, latest: Option<&Wanted>) -> bool {
    latest.is_some_and(|w| w.tasks.is_empty() || (now.dashboard && !w.dashboard))
}

/// Start the thread, reading every `refresh` (none if zero: PR status is
/// off and `gh` never runs) and logging to `log_path`. It reads only while
/// something shows PRs (see [`Wanted`]), and sends what it read as
/// [`Event::Prs`]. Returns where to send what is wanted.
pub fn start(refresh: Duration, log_path: PathBuf, tx: &Sender<Event>) -> Option<Sender<Wanted>> {
    if refresh.is_zero() {
        return None;
    }
    let (watch_tx, watch_rx) = mpsc::channel::<Wanted>();
    let events = tx.clone();
    std::thread::spawn(move || run(&watch_rx, &events, refresh, &log_path));
    Some(watch_tx)
}

/// The thread's loop, until the app goes away.
fn run(watch_rx: &Receiver<Wanted>, events: &Sender<Event>, refresh: Duration, log_path: &Path) {
    let mut cache = Cache::new(refresh);
    let mut wanted = Wanted::default();
    let mut sent: Option<Tasks> = None;
    // What came during the last pass, not taken yet.
    let mut inbox: Vec<Wanted> = Vec::new();
    loop {
        if inbox.is_empty() {
            match watch_rx.recv_timeout(TICK) {
                Ok(w) => inbox.push(w),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        inbox.extend(watch_rx.try_iter());
        take(&mut wanted, std::mem::take(&mut inbox), &mut cache);
        // The gate: nothing is read while nothing shows PRs (the sidebar
        // hidden and the dashboard closed).
        if wanted.tasks.is_empty() {
            continue;
        }
        let tasks = &wanted.tasks;
        // Before each repository, look whether what shows changed so.
        let stop = || {
            inbox.extend(watch_rx.try_iter());
            stops(&wanted, inbox.last())
        };
        let (read, failures) = cache.refresh(tasks, Instant::now, repo_of, github::pr_list, stop);
        log(log_path, &failures);
        if read || sent.as_ref() != Some(tasks) {
            if events.send(Event::Prs(cache.lookup(tasks))).is_err() {
                return;
            }
            sent = Some(tasks.clone());
        }
    }
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

    /// A clock stopped at `t`.
    fn at(t: Instant) -> impl Fn() -> Instant {
        move || t
    }

    /// Never stop a pass.
    fn go() -> bool {
        false
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
        let (read, failures) = cache.refresh(&tasks, at(t0), resolve, fetch, go);
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

        let (read, _) = cache.refresh(&tasks, at(t0 + Duration::from_secs(59)), resolve, fetch, go);
        assert!(!read);
        assert_eq!(calls.borrow().len(), 1, "cached");
        let (read, _) = cache.refresh(&tasks, at(t0 + Duration::from_secs(60)), resolve, fetch, go);
        assert!(read);
        assert_eq!(calls.borrow().len(), 2, "due again");
    }

    #[test]
    fn each_repository_is_stamped_when_its_own_read_ends() {
        let mut cache = Cache::new(Duration::from_secs(10));
        let t0 = Instant::now();
        // Each read takes 5 s: /a ends at t0 + 5, /b at t0 + 10.
        let now = RefCell::new(t0);
        let clock = || *now.borrow();
        let fetch = |_: &str| {
            *now.borrow_mut() += Duration::from_secs(5);
            Ok(Prs::new())
        };
        let resolve = |dir: &str| Some(dir.to_string());
        let tasks = [task("/a", "x"), task("/b", "y")];
        cache.refresh(&tasks, clock, resolve, fetch, go);
        assert_eq!(cache.repos["/a"].at, t0 + Duration::from_secs(5));
        assert_eq!(cache.repos["/b"].at, t0 + Duration::from_secs(10));
        // At t0 + 12 neither is due (stamped at the pass's start, both
        // would be).
        let (read, _) = cache.refresh(&tasks, at(t0 + Duration::from_secs(12)), resolve, fetch, go);
        assert!(!read);
    }

    #[test]
    fn opening_asks_again_only_past_the_floor() {
        let mut cache = Cache::new(Duration::from_secs(60));
        let calls = RefCell::new(0);
        let fetch = |_: &str| {
            *calls.borrow_mut() += 1;
            Ok(Prs::new())
        };
        let tasks = [task("/r", "feat/a")];
        let t0 = Instant::now();
        cache.refresh(&tasks, at(t0), resolve, fetch, go);
        // Closed and opened again within the floor: nothing new.
        let soon = t0 + REOPEN_FLOOR - Duration::from_secs(1);
        cache.opened(soon);
        cache.refresh(&tasks, at(soon), resolve, fetch, go);
        assert_eq!(*calls.borrow(), 1, "toggling does not run gh");
        // Past it: at once, well before `refresh`.
        let later = t0 + REOPEN_FLOOR;
        cache.opened(later);
        cache.refresh(&tasks, at(later), resolve, fetch, go);
        assert_eq!(*calls.borrow(), 2);
        cache.refresh(
            &tasks,
            at(later + Duration::from_secs(1)),
            resolve,
            fetch,
            go,
        );
        assert_eq!(*calls.borrow(), 2, "due once, not again");
    }

    #[test]
    fn a_pass_stops_before_the_next_repository_once_told() {
        let mut cache = Cache::new(Duration::from_secs(60));
        let calls = RefCell::new(Vec::new());
        let fetch = |repo: &str| {
            calls.borrow_mut().push(repo.to_string());
            Ok(Prs::new())
        };
        let resolve = |dir: &str| Some(dir.to_string());
        let tasks = [task("/a", "x"), task("/b", "y"), task("/c", "z")];
        let mut asked = 0;
        let stop_after_one = || {
            asked += 1;
            asked > 1
        };
        let t0 = Instant::now();
        let (read, _) = cache.refresh(&tasks, at(t0), resolve, fetch, stop_after_one);
        assert!(read);
        assert_eq!(*calls.borrow(), ["/a"], "one after another, then stopped");
        cache.refresh(&tasks, at(t0), resolve, fetch, go);
        assert_eq!(*calls.borrow(), ["/a", "/b", "/c"], "the rest next pass");
    }

    #[test]
    fn a_pass_stops_when_nothing_shows_or_the_dashboard_closes() {
        let shows = |dashboard, tasks: &[(String, String)]| Wanted {
            tasks: tasks.to_vec(),
            dashboard,
        };
        let some = [task("/r", "feat/a")];
        let open = shows(true, &some);
        let sidebar = shows(false, &some);
        assert!(!stops(&open, None), "nothing new");
        assert!(!stops(&open, Some(&open)));
        assert!(stops(&open, Some(&sidebar)), "the dashboard closed");
        assert!(stops(&open, Some(&shows(false, &[]))), "nothing shows");
        assert!(stops(&sidebar, Some(&shows(false, &[]))), "sidebar hidden");
        assert!(!stops(&sidebar, Some(&sidebar)), "the sidebar still shows");
        assert!(!stops(&sidebar, Some(&open)), "opening adds, not stops");
    }

    #[test]
    fn a_close_and_reopen_queued_together_count_as_opening() {
        let open = |dashboard| Wanted {
            dashboard,
            ..Wanted::default()
        };
        let mut cache = Cache::new(Duration::from_secs(60));
        let tasks = [task("/r", "feat/a")];
        let t0 = Instant::now() - REOPEN_FLOOR;
        cache.refresh(&tasks, at(t0), resolve, |_| Ok(Prs::new()), go);
        let mut watched = open(true);
        take(&mut watched, vec![open(false), open(true)], &mut cache);
        assert!(watched.dashboard);
        assert!(cache.repos["/r"].due, "opened again");
        let mut cache = Cache::new(Duration::from_secs(60));
        cache.refresh(&tasks, at(t0), resolve, |_| Ok(Prs::new()), go);
        take(&mut watched, vec![open(true)], &mut cache);
        assert!(!cache.repos["/r"].due, "still open: not opened");
    }

    #[test]
    fn a_failing_repository_shows_no_pr_and_logs_once_until_it_answers() {
        let mut cache = Cache::new(Duration::from_secs(1));
        let tasks = [task("/r", "feat/a")];
        let t0 = Instant::now();
        let ok = |_: &str| Ok(Prs::from([("feat/a".to_string(), pr(1))]));
        let bad = |_: &str| Err("gh: not logged in".to_string());
        cache.refresh(&tasks, at(t0), resolve, ok, go);
        assert_eq!(cache.lookup(&tasks)[0].1, Some(pr(1)));
        let (_, failures) =
            cache.refresh(&tasks, at(t0 + Duration::from_secs(1)), resolve, bad, go);
        assert_eq!(failures, ["/r: gh: not logged in"]);
        assert_eq!(cache.lookup(&tasks)[0].1, None, "git only");
        let (_, failures) =
            cache.refresh(&tasks, at(t0 + Duration::from_secs(2)), resolve, bad, go);
        assert!(failures.is_empty(), "logged once");
        cache.refresh(&tasks, at(t0 + Duration::from_secs(3)), resolve, ok, go);
        let (_, failures) =
            cache.refresh(&tasks, at(t0 + Duration::from_secs(4)), resolve, bad, go);
        assert_eq!(failures.len(), 1, "again after it answered");
    }
}
