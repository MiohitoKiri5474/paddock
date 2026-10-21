//! What each `paddock worktree` subcommand does, against the real git
//! and the hidden tmux server, through the rules the app uses
//! ([`crate::worktree`]).

use super::{allowed, Code, Command, Discard, Fail, Invocation};
use crate::app::HomePaths;
use crate::config::Config;
use crate::git::{self, WorkingTree, Worktree};
use crate::hooks::now_ms;
use crate::paths::Paths;
use crate::sessions::{agents, log, Agent, Sessions, State};
use crate::tmux::structure::FLOAT_OPTION;
use crate::tmux::Server;
use crate::worktree::{self as shared, WorktreeSpace, RISK_DEADLINE};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// The size a space starts at; a paddock that shows it resizes it.
const SPACE_SIZE: (u16, u16) = (80, 24);
/// How long `claude agents --json` may take before it counts as late.
const AGENTS_DEADLINE: Duration = Duration::from_secs(5);

/// Where the command runs: the hidden server, paddock's paths, and who
/// called it (their directory, and their tmux pane if they run in one).
struct Context {
    server: Server,
    paths: Paths,
    cwd: PathBuf,
    /// `$TMUX` and `$TMUX_PANE`, if set.
    tmux: Option<(String, String)>,
}

impl Context {
    fn from_env() -> Result<Self, Fail> {
        let paths = Paths::from_env().map_err(|e| Fail::new(Code::Usage, format!("{e:#}")))?;
        let cwd = std::env::current_dir()
            .map_err(|e| Fail::new(Code::Usage, format!("no current directory: {e}")))?;
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let tmux = var("TMUX").zip(var("TMUX_PANE"));
        Ok(Self {
            server: Server::new(paths.clone()),
            paths,
            cwd,
            tmux,
        })
    }

    /// The working tree of `--from`, else of the current directory.
    fn tree(&self, from: Option<&Path>) -> Result<WorkingTree, Fail> {
        let dir = from.unwrap_or(&self.cwd);
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

    /// The `[worktrees] directory` from paddock's config.
    fn directory(&self) -> Result<PathBuf, Fail> {
        Config::load(&self.paths.config_dir)
            .map(|c| c.worktrees.directory)
            .map_err(|e| Fail::new(Code::Usage, format!("{e:#}")))
    }

    /// The hidden server's pid; `None` if it does not run.
    fn server_pid(&self) -> Option<u32> {
        self.server
            .run(&["display-message", "-p", "#{pid}"])
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// Every space (tmux session id and name) on the hidden server.
    fn spaces(&self) -> Vec<(String, String)> {
        self.server
            .sessions()
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect()
    }

    /// Start a shell in a new space named `name` in `dir`, starting the
    /// server (with paddock's config) if it does not run. Returns the
    /// space's name as tmux gave it (it writes `.` and `:` as `_`).
    fn new_space(&self, name: &str, dir: &Path) -> anyhow::Result<String> {
        if self.server_pid().is_none() {
            self.server.write_config()?;
        }
        let session = self.server.new_session(name, dir, SPACE_SIZE)?;
        self.server.display(&session.id, "#{session_name}")
    }

    /// The panes of space `id` that are not floats.
    fn panes(&self, id: &str) -> Vec<Pane> {
        let format = format!("#{{{FLOAT_OPTION}}}\x1f#{{pane_id}}\x1f#{{pane_current_path}}");
        self.server
            .run(&["list-panes", "-s", "-t", id, "-F", &format])
            .map(|out| parse_panes(&out))
            .unwrap_or_default()
    }

    /// The caller's pane, if they run in a terminal of this server.
    fn caller_pane(&self, pid: Option<u32>) -> Option<String> {
        let (tmux, pane) = self.tmux.as_ref()?;
        let (theirs, pane) = log::terminal(tmux, pane, &self.paths.socket)?;
        (Some(theirs) == pid).then_some(pane)
    }

    /// The Claude sessions in paddock's terminals, as the app knows them:
    /// the hook event log of the running server replayed, then what
    /// `claude agents --json` says, and why Claude's word is missing if it
    /// did not answer in time.
    fn sessions(&self, pid: Option<u32>) -> (Sessions, Option<Fail>) {
        let mut sessions = Sessions::default();
        let Some(pid) = pid else {
            return (sessions, None);
        };
        let mut reader = log::Reader::new(log::path(&self.paths.state_dir, pid));
        for record in reader.read_new() {
            sessions.apply(&record);
        }
        let taken = now_ms();
        match fetch_agents(&self.server, AGENTS_DEADLINE) {
            Ok(Some(list)) => sessions.reconcile(taken, &list),
            Ok(None) => {}
            Err(late) => return (sessions, Some(late)),
        }
        (sessions, None)
    }
}

/// `claude agents --json` (see [`agents::fetch`]), waiting at most
/// `limit`; `Ok(None)` if Claude could not tell (not installed, say).
fn fetch_agents(server: &Server, limit: Duration) -> Result<Option<Vec<Agent>>, Fail> {
    let (tx, rx) = mpsc::channel();
    let server = server.clone();
    // Left to finish on its own if late.
    std::thread::spawn(move || {
        let _ = tx.send(agents::fetch(&server));
    });
    rx.recv_timeout(limit).map_err(|_| {
        Fail::new(
            Code::Late,
            "claude agents --json did not answer in time: nothing deleted",
        )
    })
}

/// One pane of a space.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pane {
    id: String,
    path: String,
}

/// `list-panes` output of float marker, pane id and directory, without
/// floats' panes (paddock does not follow Claude in a float).
fn parse_panes(out: &str) -> Vec<Pane> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\x1f');
            let float = parts.next()?;
            let id = parts.next()?.to_string();
            let path = parts.next()?.to_string();
            float.is_empty().then_some(Pane { id, path })
        })
        .collect()
}

/// Whether `dir` is `top` or inside it, links resolved, by whole path
/// parts (`feat-xy` is not inside `feat-x`).
fn inside(dir: &str, top: &str) -> bool {
    Path::new(&git::resolved(dir)).starts_with(git::resolved(top))
}

/// The state of the Claude in pane `pane`, unless it ended or none ran.
fn state_in(sessions: &Sessions, pane: &str) -> Option<State> {
    sessions.get(pane).filter(|s| !s.ended()).map(|s| s.state)
}

pub fn run(i: &Invocation) -> Result<(), Fail> {
    let cx = Context::from_env()?;
    let from = i.from.as_deref();
    match &i.command {
        Command::New { branch, no_space } => new(&cx, from, branch, *no_space, i.json),
        Command::Open { target } => open(&cx, from, target, i.json),
        Command::List => list(&cx, from, i.json),
        Command::Rm { target, discard } => rm(&cx, from, target, *discard, i.json),
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

fn print_json(value: &impl Serialize) -> Result<(), Fail> {
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
/// in it (its top), else a branch checked out in it.
fn find(tree: &WorkingTree, target: &str) -> Result<Worktree, Fail> {
    let all = git::worktrees(&tree.main);
    let by_path = Path::new(target)
        .is_dir()
        .then(|| git::working_tree(target))
        .flatten()
        .and_then(|t| all.iter().find(|w| w.path == t.top));
    by_path
        .or_else(|| all.iter().find(|w| w.branch.as_deref() == Some(target)))
        .cloned()
        .ok_or_else(|| {
            let repo = tree.repo();
            Fail::new(Code::Usage, format!("no worktree of {repo} for {target:?}"))
        })
}

fn open(cx: &Context, from: Option<&Path>, target: &str, json: bool) -> Result<(), Fail> {
    let tree = cx.tree(from)?;
    let w = find(&tree, target)?;
    let spaces = cx.spaces();
    let ids: Vec<String> = spaces.iter().map(|(id, _)| id.clone()).collect();
    let open = shared::spaces_in(&cx.server, &ids, &w.path)?;
    let repo = tree.repo();
    let name_of = |id: &str| spaces.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone());
    let (space, made) = match open.first().and_then(|id| name_of(id)) {
        Some(name) => (name, false),
        None => {
            let name = shared::space_name(repo, &tree.main, &w);
            let made = cx
                .new_space(&name, Path::new(&w.path))
                .map_err(|e| Fail::new(Code::Git, format!("{e:#}")))?;
            (made, true)
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
    if made {
        println!("{}", w.path);
    } else {
        println!("{} already has a space: {space}", w.path);
    }
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
    /// The states of the Claude sessions in its spaces.
    agents: Vec<&'static str>,
}

fn list(cx: &Context, from: Option<&Path>, json: bool) -> Result<(), Fail> {
    let tree = cx.tree(from)?;
    let directory = cx.directory()?;
    let pid = cx.server_pid();
    // `list` only reports: Claude not answering leaves the hook log's word.
    let (sessions, _) = cx.sessions(pid);
    let spaces = cx.spaces();
    // Each space's working tree, read once.
    let placed: Vec<(String, String, Option<String>)> = spaces
        .iter()
        .map(|(id, name)| {
            let top = shared::space_dir(&cx.server, id)
                .and_then(|d| git::working_tree(&d))
                .map(|t| t.top);
            (id.clone(), name.clone(), top)
        })
        .collect();
    let rows: Vec<Row> = git::worktrees(&tree.main)
        .into_iter()
        .map(|w| {
            let mine: Vec<&(String, String, Option<String>)> = placed
                .iter()
                .filter(|(_, _, top)| top.as_deref() == Some(w.path.as_str()))
                .collect();
            let agents = mine
                .iter()
                .flat_map(|(id, _, _)| cx.panes(id))
                .filter_map(|p| state_in(&sessions, &p.id))
                .map(State::label)
                .collect();
            let main = w.path == tree.main;
            let linked = WorkingTree {
                top: w.path.clone(),
                main: tree.main.clone(),
            };
            Row {
                ours: !main && shared::is_ours(&directory, &linked),
                space: mine.first().map(|(_, name, _)| name.clone()),
                dirty: git::is_dirty(&w.path),
                merged: main || git::in_base(&w.path, "HEAD"),
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

/// What `rm` prints with `--json`.
#[derive(Serialize)]
struct Removed<'a> {
    path: &'a str,
    branch: Option<&'a str>,
    /// The spaces closed with it.
    closed: Vec<String>,
    /// Whether `branch -d` deleted the branch (it keeps an unmerged one).
    branch_deleted: bool,
}

fn rm(
    cx: &Context,
    from: Option<&Path>,
    target: &str,
    discard: Discard,
    json: bool,
) -> Result<(), Fail> {
    let tree = cx.tree(from)?;
    let w = find(&tree, target)?;
    let label = w
        .branch
        .clone()
        .unwrap_or_else(|| git::dir_name(&w.path).to_string());
    let refuse = |why: String| Err(Fail::new(Code::Refused, format!("{label} {why}")));
    if w.path == tree.main {
        return refuse("is the main checkout: paddock never deletes it".into());
    }
    let linked = WorkingTree {
        top: w.path.clone(),
        main: tree.main.clone(),
    };
    if !shared::is_ours(&cx.directory()?, &linked) {
        return refuse(
            "was not made by paddock (not under [worktrees] directory): left alone".into(),
        );
    }
    let spaces = cx.spaces();
    let ids: Vec<String> = spaces.iter().map(|(id, _)| id.clone()).collect();
    let closing = shared::spaces_in(&cx.server, &ids, &w.path)?;
    let pid = cx.server_pid();
    if inside(&cx.cwd.to_string_lossy(), &w.path) {
        return refuse("holds the current directory: run rm from outside it".into());
    }
    let caller = cx.caller_pane(pid);
    if let Some(pane) = &caller {
        let space = cx.server.display(pane, "#{session_id}").ok();
        let path = cx.server.display(pane, "#{pane_current_path}").ok();
        let here = space.is_some_and(|s| closing.contains(&s))
            || path.is_some_and(|p| inside(&p, &w.path));
        if here {
            return refuse("holds the calling terminal: run rm from another one".into());
        }
    }
    let (sessions, late) = cx.sessions(pid);
    if let Some(late) = late {
        return Err(late);
    }
    // Every pane of the spaces that close, and any pane anywhere whose
    // directory is in the worktree.
    let top = w.path.as_str();
    let busy = ids
        .iter()
        .flat_map(|id| {
            let closes = closing.contains(id);
            cx.panes(id)
                .into_iter()
                .filter(move |p| closes || inside(&p.path, top))
                .map(move |p| (id.clone(), p.id))
        })
        .find_map(|(id, pane)| match state_in(&sessions, &pane) {
            Some(state @ (State::Running | State::Waiting)) => Some((id, state)),
            _ => None,
        });
    if let Some((id, state)) = busy {
        let name = spaces
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(id.as_str(), |(_, n)| n.as_str());
        return refuse(format!(
            "has Claude {} in space {name}: nothing deleted",
            state.label()
        ));
    }
    let confirmed = allowed(
        &label,
        shared::risks_within(&w.path, RISK_DEADLINE).as_ref(),
        discard,
    )?;
    let target = WorktreeSpace {
        space: closing.first().cloned().unwrap_or_default(),
        spaces: closing.clone(),
        path: w.path.clone(),
        branch: w.branch.clone(),
        main: tree.main.clone(),
    };
    let notice = shared::remove(&cx.server, &target, Some(&confirmed)).map_err(Fail::from)?;
    if let Some(notice) = &notice {
        eprintln!("paddock: {notice}");
    }
    let closed = closing
        .iter()
        .filter_map(|id| spaces.iter().find(|(i, _)| i == id))
        .map(|(_, name)| name.clone())
        .collect();
    if json {
        return print_json(&Removed {
            path: &w.path,
            branch: w.branch.as_deref(),
            closed,
            branch_deleted: w.branch.is_some() && notice.is_none(),
        });
    }
    println!("{}", w.path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, branch: Option<&str>) -> Row {
        Row {
            path: path.into(),
            branch: branch.map(str::to_string),
            main: false,
            ours: false,
            space: None,
            dirty: false,
            merged: true,
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
            ..row("/w/repo/feat-x", Some("feat/x"))
        };
        let detached = row("/w/repo/loose", None);
        let json = serde_json::to_string(&[r, detached]).unwrap();
        assert_eq!(
            json,
            r#"[{"path":"/w/repo/feat-x","branch":"feat/x","main":false,"ours":true,"space":"repo/feat/x","dirty":true,"merged":false,"agents":["waiting","idle"]},{"path":"/w/repo/loose","branch":null,"main":false,"ours":false,"space":null,"dirty":false,"merged":true,"agents":[]}]"#
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
            ..row("/w/repo/feat-x", Some("feat/x"))
        };
        let home = HomePaths::new();
        let got = table(&[main, x, row("/w/repo/loose", None)], &home);
        assert_eq!(
            got,
            "/r/repo         main        main    -     repo  clean  merged    -\n\
             /w/repo/feat-x  feat/x      linked  ours  -     dirty  unmerged  running\n\
             /w/repo/loose   (detached)  linked  -     -     clean  merged    -\n"
        );
        assert_eq!(table(&[], &home), "");
    }

    #[test]
    fn panes_of_floats_are_left_out() {
        let out = "\x1f%1\x1f/r/a\ng\x1f%2\x1f/r/b\n\x1f%3\x1f/r/c d\n";
        let ids: Vec<String> = parse_panes(out).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["%1", "%3"]);
    }

    #[test]
    fn inside_compares_whole_path_parts() {
        assert!(inside("/nonexistent/w/feat-x", "/nonexistent/w/feat-x"));
        assert!(inside("/nonexistent/w/feat-x/src", "/nonexistent/w/feat-x"));
        assert!(!inside("/nonexistent/w/feat-xy", "/nonexistent/w/feat-x"));
        assert!(!inside("/nonexistent/w", "/nonexistent/w/feat-x"));
    }
}
