//! What `paddock worktree new|open|list` do, and what every subcommand
//! shares: where it runs ([`Context`]), the spaces on the hidden server,
//! and which worktree an argument names. `rm` is in [`super::rm`]. All
//! of it goes through the rules the app uses ([`crate::worktree`]).

use super::{rm, Code, Command, Fail, Invocation};
use crate::backend::{self, Backend, HookTerminal, SpaceId, TerminalId};
use crate::config::Config;
use crate::git::{self, WorkingTree, Worktree};
use crate::github::{self, Checks, Pr, Prs, Review};
use crate::hooks::now_ms;
use crate::paths::{HomePaths, Paths};
use crate::sessions::{agents, log, Agent, Sessions, State};
use crate::worktree::{self as shared, RISK_DEADLINE};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// The size a space starts at; a paddock that shows it resizes it.
const SPACE_SIZE: (u16, u16) = (80, 24);
/// How long `claude agents --json` may take before it counts as late.
const AGENTS_DEADLINE: Duration = Duration::from_secs(5);

/// Where the command runs: the backend, paddock's paths, and who called
/// it (their directory, and their terminal if they run in one).
pub(super) struct Context {
    pub backend: Backend,
    pub paths: Paths,
    pub cwd: PathBuf,
    /// The terminal of paddock's the command runs in, if any.
    pub caller: Option<HookTerminal>,
    /// How long `rm`'s check of what it would lose may take.
    pub risk_deadline: Duration,
}

/// A space in the backend: its id and name, and its directory
/// ([`shared::space_dir`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Space {
    pub id: SpaceId,
    pub name: String,
    pub dir: Option<String>,
}

/// The name of space `id` among `spaces`, else the id itself.
pub(super) fn name_of<'a>(spaces: &'a [Space], id: &'a SpaceId) -> &'a str {
    spaces
        .iter()
        .find(|s| s.id == *id)
        .map_or(id.as_str(), |s| s.name.as_str())
}

/// The ids of `spaces`.
pub(super) fn ids(spaces: &[Space]) -> Vec<SpaceId> {
    spaces.iter().map(|s| s.id.clone()).collect()
}

impl Context {
    pub fn new(backend: Backend, paths: Paths, cwd: PathBuf, caller: Option<HookTerminal>) -> Self {
        Self {
            backend,
            paths,
            cwd,
            caller,
            risk_deadline: RISK_DEADLINE,
        }
    }

    fn from_env() -> Result<Self, Fail> {
        let paths = Paths::from_env().map_err(|e| Fail::new(Code::Usage, format!("{e:#}")))?;
        let cwd = std::env::current_dir()
            .map_err(|e| Fail::new(Code::Usage, format!("no current directory: {e}")))?;
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let caller = backend::hook_terminal(var, &paths);
        let backend = Backend::configured(paths.clone())
            .map_err(|e| Fail::new(Code::Usage, format!("{e:#}")))?;
        // What runs where comes back empty from a host paddock cannot talk
        // to, and `rm` must not take that for nothing busy.
        backend
            .check()
            .map_err(|e| Fail::new(Code::Usage, format!("{e:#}")))?;
        Ok(Self::new(backend, paths, cwd, caller))
    }

    /// The directory `--from` names, else the current one.
    pub fn base<'a>(&'a self, from: Option<&'a Path>) -> &'a Path {
        from.unwrap_or(&self.cwd)
    }

    /// The working tree of `--from`, else of the current directory.
    pub fn tree(&self, from: Option<&Path>) -> Result<WorkingTree, Fail> {
        let dir = self.base(from);
        if !dir.is_dir() {
            let msg = format!("{} is not a directory", dir.display());
            return Err(Fail::new(Code::Usage, msg));
        }
        git::working_tree_in_time(&dir.to_string_lossy())
            .map_err(|e| Fail::new(Code::Late, e))?
            .ok_or_else(|| {
                let msg = format!("{} is not in a git repository", dir.display());
                Fail::new(Code::Usage, msg)
            })
    }

    /// paddock's config.
    pub fn config(&self) -> Result<Config, Fail> {
        Config::load(&self.paths.config_dir).map_err(|e| Fail::new(Code::Usage, format!("{e:#}")))
    }

    /// The `[worktrees] directory` from paddock's config.
    pub fn directory(&self) -> Result<PathBuf, Fail> {
        self.config().map(|c| c.worktrees.directory)
    }

    /// The backend's instance id (see [`Backend::instance`]); `None` if
    /// it does not run.
    pub fn instance(&self) -> Option<u32> {
        self.backend.instance().ok()
    }

    /// Every space in the backend, oldest first.
    pub fn spaces(&self) -> Vec<Space> {
        self.backend
            .spaces()
            .into_iter()
            .map(|s| Space {
                dir: shared::space_dir(&self.backend, &s.id),
                id: s.id,
                name: s.name,
            })
            .collect()
    }

    /// Start a shell in a new space named `name` in `dir`, starting the
    /// backend (with paddock's settings) if it does not run. Returns the
    /// space's name as the backend gave it (tmux writes `.` and `:` as `_`).
    pub fn new_space(&self, name: &str, dir: &Path) -> anyhow::Result<String> {
        if self.instance().is_none() {
            self.backend.prepare()?;
        }
        let space = self.backend.new_space(name, dir, SPACE_SIZE)?;
        self.backend.space_name(&space)
    }

    /// The caller's pane, if they run in a terminal of this backend.
    pub fn caller_terminal(&self, instance: Option<u32>) -> Option<TerminalId> {
        let caller = self.caller.as_ref()?;
        (Some(caller.instance) == instance).then(|| caller.terminal.clone())
    }

    /// The agent sessions in paddock's terminals, as the app knows them:
    /// the hook event log of the running server replayed, then what
    /// `claude agents --json` says of Claude's, and why Claude's word is
    /// missing if it did not answer in time.
    pub fn sessions(&self, instance: Option<u32>) -> (Sessions, Option<Fail>) {
        let mut sessions = Sessions::default();
        let Some(instance) = instance else {
            return (sessions, None);
        };
        let mut reader = log::Reader::new(log::path(&self.paths.state_dir, instance));
        for record in reader.read_new() {
            sessions.apply(&record);
        }
        let taken = now_ms();
        match fetch_agents(&self.backend, AGENTS_DEADLINE) {
            Ok(Some(list)) => sessions.reconcile(taken, &list),
            Ok(None) => {}
            Err(late) => return (sessions, Some(late)),
        }
        (sessions, None)
    }
}

/// `claude agents --json` (see [`agents::fetch`]), waiting at most
/// `limit`; `Ok(None)` if Claude could not tell (not installed, say).
fn fetch_agents(backend: &Backend, limit: Duration) -> Result<Option<Vec<Agent>>, Fail> {
    let (tx, rx) = mpsc::channel();
    let backend = backend.clone();
    // Left to finish on its own if late.
    std::thread::spawn(move || {
        let _ = tx.send(agents::fetch(&backend));
    });
    rx.recv_timeout(limit).map_err(|_| {
        Fail::new(
            Code::Late,
            "claude agents --json did not answer in time: nothing deleted",
        )
    })
}

/// Whether `dir` is `top` or inside it, links resolved, by whole path
/// parts (`feat-xy` is not inside `feat-x`).
pub(super) fn inside(dir: &str, top: &str) -> bool {
    Path::new(&git::resolved(dir)).starts_with(git::resolved(top))
}

/// The state of the agent in pane `pane`, unless it ended or none ran.
pub(super) fn state_in(sessions: &Sessions, pane: &str) -> Option<State> {
    sessions.get(pane).filter(|s| !s.ended()).map(|s| s.state)
}

pub fn run(i: &Invocation) -> Result<(), Fail> {
    let cx = Context::from_env()?;
    let from = i.from.as_deref();
    match &i.command {
        Command::New { branch, no_space } => new(&cx, from, branch, *no_space, i.json),
        Command::Open { target } => open(&cx, from, target, i.json),
        Command::List => list(&cx, from, i.json),
        Command::Rm { target, discard } => rm::rm(&cx, from, target, *discard, i.json),
    }
}

/// What `new` and `open` print with `--json`.
#[derive(Serialize)]
struct Made<'a> {
    path: &'a str,
    branch: Option<&'a str>,
    repo: &'a str,
    space: Option<&'a str>,
}

pub(super) fn print_json(value: &impl Serialize) -> Result<(), Fail> {
    let text = serde_json::to_string(value).map_err(|e| Fail::new(Code::Usage, e.to_string()))?;
    println!("{text}");
    Ok(())
}

fn new(
    cx: &Context,
    from: Option<&Path>,
    branch: &str,
    no_space: bool,
    json: bool,
) -> Result<(), Fail> {
    let tree = cx.tree(from)?;
    let directory = cx.directory()?;
    let (path, space) = shared::create(&directory, &tree, branch, |name, path| {
        if no_space {
            Ok(None)
        } else {
            cx.new_space(name, &path).map(Some)
        }
    })?;
    let path = git::resolved(&path.to_string_lossy());
    if json {
        return print_json(&Made {
            path: &path,
            branch: Some(branch.trim()),
            repo: tree.repo(),
            space: space.as_deref(),
        });
    }
    println!("{path}");
    Ok(())
}

/// The worktree of `tree`'s repository that `target` names: a directory
/// (relative to `base`, the `--from` directory or the current one) in
/// it, or a branch checked out in it. If it names one worktree by path
/// and another by branch, it names neither.
pub(super) fn find(tree: &WorkingTree, base: &Path, target: &str) -> Result<Worktree, Fail> {
    let all = git::worktrees(&tree.main);
    let dir = base.join(target);
    let by_path = dir
        .is_dir()
        .then(|| git::working_tree(&dir.to_string_lossy()))
        .flatten()
        .and_then(|t| all.iter().find(|w| w.path == t.top));
    let by_branch = all.iter().find(|w| w.branch.as_deref() == Some(target));
    pick(by_path, by_branch, tree.repo(), target)
}

/// [`find`]'s choice between the worktree `target` names as a path and
/// the one it names as a branch.
fn pick(
    by_path: Option<&Worktree>,
    by_branch: Option<&Worktree>,
    repo: &str,
    target: &str,
) -> Result<Worktree, Fail> {
    match (by_path, by_branch) {
        (Some(p), Some(b)) if p != b => Err(Fail::new(
            Code::Usage,
            format!(
                "{target:?} names two worktrees of {repo}: {} as a path and {} as a branch; \
                 give the full path",
                p.path, b.path
            ),
        )),
        (Some(w), _) | (None, Some(w)) => Ok(w.clone()),
        (None, None) => Err(Fail::new(
            Code::Usage,
            format!("no worktree of {repo} for {target:?}"),
        )),
    }
}

fn open(cx: &Context, from: Option<&Path>, target: &str, json: bool) -> Result<(), Fail> {
    let tree = cx.tree(from)?;
    let w = find(&tree, cx.base(from), target)?;
    let spaces = cx.spaces();
    let open = shared::spaces_in(&cx.backend, &ids(&spaces), &w.path)?;
    let repo = tree.repo();
    let space = match open.first() {
        Some(id) => {
            let name = name_of(&spaces, id).to_string();
            // A notice, not an error: the output stays the same shape.
            eprintln!("paddock: {} already has a space: {name}", w.path);
            name
        }
        None => {
            let name = shared::space_name(repo, &tree.main, &w);
            cx.new_space(&name, Path::new(&w.path))
                .map_err(|e| Fail::new(Code::Git, format!("{e:#}")))?
        }
    };
    if json {
        return print_json(&Made {
            path: &w.path,
            branch: w.branch.as_deref(),
            repo,
            space: Some(&space),
        });
    }
    println!("{}", w.path);
    Ok(())
}

/// One worktree as `list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Row {
    path: String,
    /// `None` if detached.
    branch: Option<String>,
    /// The main checkout, not a linked worktree.
    main: bool,
    /// paddock made it (so `rm` may delete it).
    ours: bool,
    /// The first space in it, if any is open.
    space: Option<String>,
    dirty: bool,
    /// Every commit checked out there is in the base.
    merged: bool,
    /// Its branch's pull request, as `gh` gave it; `None` without one,
    /// without `gh`, or with `[github] refresh_secs = 0`.
    pr: Option<PrField>,
    /// The states of the agent sessions in its spaces' tabs.
    agents: Vec<&'static str>,
}

/// A worktree's pull request as `list` gives it: its state and checks in
/// the dashboard's words, its review in snake case (`checks` and `review`
/// `None` without any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PrField {
    number: u64,
    state: &'static str,
    checks: Option<&'static str>,
    review: Option<&'static str>,
    url: String,
}

impl From<&Pr> for PrField {
    fn from(pr: &Pr) -> Self {
        Self {
            number: pr.number,
            state: pr.state.label(),
            checks: pr.checks.map(Checks::label),
            review: pr.review.map(Review::key),
            url: pr.url.clone(),
        }
    }
}

/// Ask `gh` for the pull requests of the repository at `main`, on a
/// thread so the rest of `list` goes on meanwhile; none if `refresh` is
/// zero (PR status off). Joined, it gives none if `gh` could not answer
/// (not installed, logged out, no GitHub remote, slower than its limit):
/// `list` only reports, and says nothing of it.
fn ask_prs(main: &str, refresh: Duration) -> Option<std::thread::JoinHandle<Prs>> {
    if refresh.is_zero() {
        return None;
    }
    let main = main.to_string();
    Some(std::thread::spawn(move || {
        github::pr_list(&main).unwrap_or_default()
    }))
}

fn list(cx: &Context, from: Option<&Path>, json: bool) -> Result<(), Fail> {
    let tree = cx.tree(from)?;
    let config = cx.config()?;
    let directory = config.worktrees.directory;
    // One `gh` call for the repository, while the rest is read.
    let asked = ask_prs(&tree.main, config.github_refresh);
    let instance = cx.instance();
    // `list` only reports: Claude not answering leaves the hook log's word.
    let (sessions, _) = cx.sessions(instance);
    let spaces = cx.spaces();
    // Each space's working tree, read once.
    let tops: Vec<Option<String>> = spaces
        .iter()
        .map(|s| s.dir.as_deref().and_then(git::working_tree).map(|t| t.top))
        .collect();
    let worktrees = git::worktrees(&tree.main);
    let prs = asked.and_then(|h| h.join().ok()).unwrap_or_default();
    let rows: Vec<Row> = worktrees
        .into_iter()
        .map(|w| {
            let mine: Vec<&Space> = spaces
                .iter()
                .zip(&tops)
                .filter(|(_, top)| top.as_deref() == Some(w.path.as_str()))
                .map(|(s, _)| s)
                .collect();
            // As the sidebar: paddock does not follow an agent in a float.
            let agents = mine
                .iter()
                .flat_map(|s| cx.backend.space_terminals(&s.id))
                .filter(|p| !p.float)
                .filter_map(|t| state_in(&sessions, t.id.as_str()))
                .map(State::label)
                .collect();
            let main = w.path == tree.main;
            let linked = WorkingTree {
                top: w.path.clone(),
                main: tree.main.clone(),
            };
            Row {
                ours: !main && shared::is_ours(&directory, &linked),
                space: mine.first().map(|s| s.name.clone()),
                dirty: git::is_dirty(&w.path),
                merged: main || git::in_base(&w.path, "HEAD"),
                pr: w
                    .branch
                    .as_ref()
                    .and_then(|b| prs.get(b))
                    .map(PrField::from),
                agents,
                main,
                path: w.path,
                branch: w.branch,
            }
        })
        .collect();
    if json {
        return print_json(&rows);
    }
    print!("{}", table(&rows, &HomePaths::new()));
    Ok(())
}

/// `list`'s text: one line per worktree, its columns aligned.
fn table(rows: &[Row], home: &HomePaths) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let or_dash = |s: Option<&str>| s.unwrap_or("-").to_string();
            vec![
                home.show(&r.path),
                r.branch.clone().unwrap_or_else(|| "(detached)".into()),
                (if r.main { "main" } else { "linked" }).into(),
                (if r.ours { "ours" } else { "-" }).into(),
                or_dash(r.space.as_deref()),
                (if r.dirty { "dirty" } else { "clean" }).into(),
                (if r.merged { "merged" } else { "unmerged" }).into(),
                r.pr.as_ref()
                    .map_or_else(|| "-".into(), |p| format!("#{} {}", p.number, p.state)),
                if r.agents.is_empty() {
                    "-".into()
                } else {
                    r.agents.join(",")
                },
            ]
        })
        .collect();
    let columns = cells.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            cells
                .iter()
                .map(|r| r[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    cells
        .iter()
        .map(|r| {
            let line: Vec<String> = r
                .iter()
                .zip(&widths)
                .map(|(cell, &w)| format!("{cell:<w$}"))
                .collect();
            line.join("  ").trim_end().to_string() + "\n"
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PrState;

    fn row(path: &str, branch: Option<&str>) -> Row {
        Row {
            path: path.into(),
            branch: branch.map(str::to_string),
            main: false,
            ours: false,
            space: None,
            dirty: false,
            merged: true,
            pr: None,
            agents: vec![],
        }
    }

    #[test]
    fn a_list_row_s_json_has_every_field_in_a_fixed_order() {
        let r = Row {
            main: false,
            ours: true,
            space: Some("repo/feat/x".into()),
            dirty: true,
            merged: false,
            agents: vec!["waiting", "idle"],
            pr: Some(PrField::from(&Pr::sample(
                123,
                PrState::Open,
                Some(Checks::Passing),
                Some(Review::ChangesRequested),
            ))),
            ..row("/w/repo/feat-x", Some("feat/x"))
        };
        let detached = row("/w/repo/loose", None);
        let json = serde_json::to_string(&[r, detached]).unwrap();
        assert_eq!(
            json,
            r#"[{"path":"/w/repo/feat-x","branch":"feat/x","main":false,"ours":true,"space":"repo/feat/x","dirty":true,"merged":false,"pr":{"number":123,"state":"open","checks":"passing","review":"changes_requested","url":"https://github.com/o/r/pull/123"},"agents":["waiting","idle"]},{"path":"/w/repo/loose","branch":null,"main":false,"ours":false,"space":null,"dirty":false,"merged":true,"pr":null,"agents":[]}]"#
        );
    }

    #[test]
    fn a_prs_json_names_its_state_checks_and_review_in_words() {
        let field = |p: &Pr| serde_json::to_value(PrField::from(p)).unwrap();
        let draft = field(&Pr::sample(1, PrState::Draft, Some(Checks::Pending), None));
        assert_eq!(
            (&draft["state"], &draft["checks"], &draft["review"]),
            (&"draft".into(), &"pending".into(), &serde_json::Value::Null)
        );
        let merged = field(&Pr::sample(
            2,
            PrState::Merged,
            Some(Checks::Failing),
            Some(Review::Approved),
        ));
        assert_eq!(
            (&merged["state"], &merged["checks"], &merged["review"]),
            (&"merged".into(), &"failing".into(), &"approved".into())
        );
        let closed = field(&Pr::sample(
            3,
            PrState::Closed,
            None,
            Some(Review::Required),
        ));
        assert_eq!(
            (&closed["state"], &closed["checks"], &closed["review"]),
            (
                &"closed".into(),
                &serde_json::Value::Null,
                &"review_required".into()
            )
        );
    }

    #[test]
    fn made_json_names_path_branch_repo_and_space() {
        let made = Made {
            path: "/w/repo/feat-x",
            branch: Some("feat/x"),
            repo: "repo",
            space: None,
        };
        assert_eq!(
            serde_json::to_string(&made).unwrap(),
            r#"{"path":"/w/repo/feat-x","branch":"feat/x","repo":"repo","space":null}"#
        );
    }

    #[test]
    fn the_text_list_is_one_aligned_line_per_worktree() {
        let main = Row {
            main: true,
            space: Some("repo".into()),
            ..row("/r/repo", Some("main"))
        };
        let x = Row {
            ours: true,
            dirty: true,
            merged: false,
            agents: vec!["running"],
            pr: Some(PrField::from(&Pr::sample(123, PrState::Open, None, None))),
            ..row("/w/repo/feat-x", Some("feat/x"))
        };
        let home = HomePaths::new();
        let got = table(&[main, x, row("/w/repo/loose", None)], &home);
        assert_eq!(
            got,
            "/r/repo         main        main    -     repo  clean  merged    -          -\n\
             /w/repo/feat-x  feat/x      linked  ours  -     dirty  unmerged  #123 open  running\n\
             /w/repo/loose   (detached)  linked  -     -     clean  merged    -          -\n"
        );
        assert_eq!(table(&[], &home), "");
    }

    #[test]
    fn inside_compares_whole_path_parts() {
        assert!(inside("/nonexistent/w/feat-x", "/nonexistent/w/feat-x"));
        assert!(inside("/nonexistent/w/feat-x/src", "/nonexistent/w/feat-x"));
        assert!(!inside("/nonexistent/w/feat-xy", "/nonexistent/w/feat-x"));
        assert!(!inside("/nonexistent/w", "/nonexistent/w/feat-x"));
    }

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: path.into(),
            branch: branch.map(str::to_string),
        }
    }

    #[test]
    fn a_name_for_two_different_worktrees_names_neither() {
        let b = wt("/w/repo/feat-b", Some("feat/b"));
        let dash = wt("/w/repo/x", Some("feat-b"));
        let both = pick(Some(&b), Some(&dash), "repo", "feat-b").unwrap_err();
        assert_eq!(both.code, Code::Usage);
        assert!(
            both.message.contains("/w/repo/feat-b as a path"),
            "{both:?}"
        );
        assert!(both.message.contains("/w/repo/x as a branch"), "{both:?}");
        // The same worktree both ways, or only one way: that one.
        assert_eq!(pick(Some(&b), Some(&b), "repo", "x"), Ok(b.clone()));
        assert_eq!(pick(Some(&b), None, "repo", "x"), Ok(b.clone()));
        assert_eq!(pick(None, Some(&dash), "repo", "x"), Ok(dash));
        let none = pick(None, None, "repo", "nope").unwrap_err();
        assert_eq!(none.message, "no worktree of repo for \"nope\"");
    }

    #[test]
    fn spaces_are_named_by_id() {
        let spaces = [Space {
            id: "$1".into(),
            name: "repo/feat/x".into(),
            dir: None,
        }];
        assert_eq!(name_of(&spaces, &"$1".into()), "repo/feat/x");
        assert_eq!(name_of(&spaces, &"$9".into()), "$9");
        assert_eq!(ids(&spaces), ["$1"]);
    }
}
