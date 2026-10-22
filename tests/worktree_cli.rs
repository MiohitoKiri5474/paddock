//! Ticket 25: `paddock worktree new|open|list|rm`, what `C-a W`, `C-a O`
//! and sidebar `x` do, without the UI. Against real temporary
//! repositories and the sandbox's own tmux server; every command runs in
//! a sandbox directory, never in paddock's own repository.

mod harness;

use harness::{Sandbox, PROMPT};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);

/// Run git in `dir`, ignoring the user's own git configuration.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `main` with one commit.
fn repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    commit(&dir, "one");
    dir
}

fn commit(dir: &Path, message: &str) {
    std::fs::write(dir.join(message), "x\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn has_branch(dir: &Path, branch: &str) -> bool {
    !git(dir, &["branch", "--list", branch]).is_empty()
}

fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Where `worktree new` puts `branch`: the default directory in the
/// sandbox's home.
fn worktree(sandbox: &Sandbox, branch: &str) -> PathBuf {
    sandbox
        .path("home/.paddock/worktrees/repo")
        .join(branch.replace('/', "-"))
}

/// `paddock worktree <args>` run in `dir`.
fn cli(sandbox: &Sandbox, dir: &Path, args: &[&str]) -> Output {
    let mut all = vec!["worktree"];
    all.extend(args);
    sandbox.run_in(dir, &all, &[], "")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Assert `out` exited with `code`, with one stderr line containing
/// `says` (none if `code` is 0).
fn exits(out: &Output, code: i32, says: &str) {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    let err = stderr(out);
    if code != 0 {
        assert_eq!(err.lines().count(), 1, "one stderr line: {err:?}");
        assert!(err.contains(says), "{err:?} lacks {says:?}");
    }
}

fn json(out: &Output) -> Value {
    serde_json::from_str(&stdout(out)).unwrap_or_else(|e| panic!("{e}: {out:?}"))
}

/// `worktree new <branch> --no-space`, which must succeed; its path.
fn made(sandbox: &Sandbox, dir: &Path, branch: &str) -> PathBuf {
    let out = cli(sandbox, dir, &["new", branch, "--no-space"]);
    exits(&out, 0, "");
    PathBuf::from(stdout(&out).trim())
}

/// The tmux session id (`$N`) of session `name`.
fn session_id(sandbox: &Sandbox, name: &str) -> String {
    sandbox
        .tmux(&["list-sessions", "-F", "#{session_id}\t#{session_name}"])
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(_, n)| *n == name)
        .map(|(id, _)| id.to_string())
        .unwrap_or_else(|| panic!("no session {name}"))
}

/// Report Claude as running in the terminal of session `name`, whose
/// directory is `cwd`.
fn claude_runs_in(sandbox: &Sandbox, name: &str, cwd: &Path) {
    let payload = std::fs::read_to_string(format!("{SPIKE}/UserPromptSubmit.json"))
        .unwrap()
        .replace("\"/tmp/repo\"", &format!("{:?}", cwd.display()));
    sandbox.hook(&session_id(sandbox, name), "UserPromptSubmit", &payload);
}

#[test]
fn new_makes_a_worktree_and_a_space_that_a_running_paddock_shows() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);

    let out = cli(
        &sandbox,
        &sandbox.path("home"),
        &["new", "feat/x", "--json", "--from", &dir.to_string_lossy()],
    );
    exits(&out, 0, "");
    let path = real(&worktree(&sandbox, "feat/x"));
    assert_eq!(
        json(&out),
        serde_json::json!({
            "path": path.to_string_lossy(),
            "branch": "feat/x",
            "repo": "repo",
            "space": "repo/feat/x",
        })
    );
    assert_eq!(git(&path, &["branch", "--show-current"]), "feat/x");
    assert_eq!(sandbox.session_names(), ["space-1", "repo/feat/x"]);
    let pane_dir = sandbox.display("repo/feat/x", "#{pane_current_path}");
    assert_eq!(PathBuf::from(pane_dir), path);
    paddock.wait_for("the new space in the sidebar", |_| {
        paddock.sidebar().iter().any(|r| r.trim() == "repo/feat/x")
    });

    // And `rm` takes it away again.
    let out = cli(&sandbox, &dir, &["rm", "feat/x"]);
    exits(&out, 0, "");
    assert_eq!(stdout(&out).trim(), path.to_string_lossy());
    assert!(!path.exists());
    assert!(!has_branch(&dir, "feat/x"));
    paddock.wait_for("the space gone from the sidebar", |_| {
        !paddock.sidebar().iter().any(|r| r.contains("feat/x"))
    });
    assert_eq!(sandbox.session_names(), ["space-1"]);
}

#[test]
fn a_space_named_after_a_branch_keeps_a_hash() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);

    // tmux expands formats in names: `#S` would become the name itself.
    let out = cli(&sandbox, &dir, &["new", "v#S", "--json"]);
    exits(&out, 0, "");
    assert_eq!(json(&out)["space"], "repo/v#S");
    assert_eq!(sandbox.session_names(), ["repo/v#S"]);
}

#[test]
fn new_starts_the_server_when_paddock_is_closed_and_no_space_makes_none() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);

    let path = made(&sandbox, &dir, "feat/n");
    assert_eq!(path, real(&worktree(&sandbox, "feat/n")));
    assert!(sandbox.session_names().is_empty(), "--no-space");

    let out = cli(&sandbox, &dir, &["new", "feat/s", "--json"]);
    exits(&out, 0, "");
    assert_eq!(json(&out)["space"], "repo/feat/s");
    assert_eq!(sandbox.session_names(), ["repo/feat/s"]);
}

#[test]
fn an_existing_branch_is_checked_out_and_failures_make_nothing() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    git(&dir, &["branch", "feat/y"]);
    commit(&dir, "two");
    let tip = git(&dir, &["rev-parse", "feat/y"]);

    let path = made(&sandbox, &dir, "feat/y");
    assert_eq!(
        git(&path, &["rev-parse", "HEAD"]),
        tip,
        "the branch as it was"
    );

    // A bad name is a usage error.
    let out = cli(&sandbox, &dir, &["new", "bad..name"]);
    exits(&out, 1, "not a valid branch name");
    assert!(!worktree(&sandbox, "bad..name").exists());

    // git refuses (main is checked out already): its reason, exit 2.
    let out = cli(&sandbox, &dir, &["new", "main"]);
    exits(&out, 2, "'main'");
    assert!(!stderr(&out).contains("Preparing"));
    assert!(!worktree(&sandbox, "main").exists());

    // The space cannot be made (its name is taken): the worktree and the
    // branch made for it go again.
    sandbox.tmux(&["new-session", "-d", "-s", "repo/feat/z"]);
    let out = cli(&sandbox, &dir, &["new", "feat/z"]);
    exits(&out, 2, "duplicate session");
    assert!(!worktree(&sandbox, "feat/z").exists());
    assert!(!has_branch(&dir, "feat/z"));
    assert_eq!(sandbox.session_names(), ["repo/feat/z"]);

    // Not in a repository.
    let out = cli(&sandbox, &sandbox.path("home"), &["new", "feat/q"]);
    exits(&out, 1, "not in a git repository");
    // Usage errors.
    exits(&cli(&sandbox, &dir, &["new"]), 1, "needs a branch");
    exits(&cli(&sandbox, &dir, &["frob"]), 1, "unknown command");
}

#[test]
fn open_makes_a_space_once() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let path = made(&sandbox, &dir, "feat/o");

    let out = cli(&sandbox, &dir, &["open", "feat/o", "--json"]);
    exits(&out, 0, "");
    assert_eq!(
        json(&out),
        serde_json::json!({
            "path": path.to_string_lossy(),
            "branch": "feat/o",
            "repo": "repo",
            "space": "repo/feat/o",
        })
    );
    assert_eq!(sandbox.session_names(), ["repo/feat/o"]);

    // Again, by path: it says so and changes nothing.
    let out = cli(&sandbox, &dir, &["open", &path.to_string_lossy()]);
    exits(&out, 0, "");
    assert_eq!(stdout(&out).trim(), path.to_string_lossy());
    assert!(
        stderr(&out).contains("already has a space: repo/feat/o"),
        "{out:?}"
    );
    assert_eq!(sandbox.session_names(), ["repo/feat/o"]);

    exits(
        &cli(&sandbox, &dir, &["open", "nope"]),
        1,
        "no worktree of repo",
    );
}

#[test]
fn list_tells_each_kind_of_worktree_apart() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let clean = made(&sandbox, &dir, "feat/clean");
    let dirty = made(&sandbox, &dir, "feat/dirty");
    std::fs::write(dirty.join("new.txt"), "x\n").unwrap();
    let ahead = made(&sandbox, &dir, "feat/ahead");
    commit(&ahead, "ahead");
    let elsewhere = sandbox.path("wt/feat-o");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/o",
            &elsewhere.to_string_lossy(),
        ],
    );
    let loose = worktree(&sandbox, "loose");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &loose.to_string_lossy(),
        ],
    );
    let out = cli(&sandbox, &dir, &["open", "feat/clean"]);
    exits(&out, 0, "");
    claude_runs_in(&sandbox, "repo/feat/clean", &clean);

    let out = cli(&sandbox, &dir, &["list", "--json"]);
    exits(&out, 0, "");
    let rows = json(&out);
    let row = |path: &Path| {
        let path = real(path).to_string_lossy().into_owned();
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == path.as_str())
            .unwrap_or_else(|| panic!("no row for {path}: {rows:#}"))
            .clone()
    };
    let fields = |r: &Value| {
        let f = |k: &str| r[k].clone();
        (
            f("branch"),
            f("main"),
            f("ours"),
            f("space"),
            f("dirty"),
            f("merged"),
            f("agents"),
        )
    };
    use serde_json::json as j;
    assert_eq!(
        fields(&row(&dir)),
        (
            j!("main"),
            j!(true),
            j!(false),
            j!(null),
            j!(false),
            j!(true),
            j!([])
        )
    );
    assert_eq!(
        fields(&row(&clean)),
        (
            j!("feat/clean"),
            j!(false),
            j!(true),
            j!("repo/feat/clean"),
            j!(false),
            j!(true),
            j!(["running"])
        )
    );
    assert_eq!(
        fields(&row(&dirty)),
        (
            j!("feat/dirty"),
            j!(false),
            j!(true),
            j!(null),
            j!(true),
            j!(true),
            j!([])
        )
    );
    assert_eq!(
        fields(&row(&ahead)),
        (
            j!("feat/ahead"),
            j!(false),
            j!(true),
            j!(null),
            j!(false),
            j!(false),
            j!([])
        )
    );
    assert_eq!(
        fields(&row(&elsewhere)),
        (
            j!("feat/o"),
            j!(false),
            j!(false),
            j!(null),
            j!(false),
            j!(true),
            j!([])
        )
    );
    assert_eq!(
        fields(&row(&loose)),
        (
            j!(null),
            j!(false),
            j!(true),
            j!(null),
            j!(false),
            j!(true),
            j!([])
        )
    );
    assert_eq!(rows.as_array().unwrap().len(), 6);

    // The text form: one aligned line each, from a linked worktree too.
    let out = cli(&sandbox, &ahead, &["list"]);
    exits(&out, 0, "");
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 6, "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("(detached)") && l.contains("ours")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l.contains("feat/clean") && l.ends_with("running")),
        "{text}"
    );
}

#[test]
fn rm_refuses_each_risk_with_exit_3_and_deletes_nothing() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);

    let dirty = made(&sandbox, &dir, "feat/dirty");
    std::fs::write(dirty.join("new.txt"), "x\n").unwrap();
    exits(
        &cli(&sandbox, &dir, &["rm", "feat/dirty"]),
        3,
        "uncommitted changes",
    );
    exits(
        &cli(&sandbox, &dir, &["rm", "feat/dirty", "--discard-commits"]),
        3,
        "--discard-changes",
    );
    assert!(dirty.join("new.txt").is_file());

    let ahead = made(&sandbox, &dir, "feat/ahead");
    commit(&ahead, "ahead");
    exits(
        &cli(&sandbox, &dir, &["rm", "feat/ahead"]),
        3,
        "commits not in main",
    );
    exits(
        &cli(&sandbox, &dir, &["rm", "feat/ahead", "--discard-changes"]),
        3,
        "--discard-commits",
    );
    assert!(ahead.is_dir());

    let loose = worktree(&sandbox, "loose");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &loose.to_string_lossy(),
        ],
    );
    commit(&loose, "loose");
    exits(
        &cli(&sandbox, &dir, &["rm", &loose.to_string_lossy()]),
        3,
        "commits not in main",
    );
    assert!(loose.is_dir());

    let elsewhere = sandbox.path("wt/feat-o");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/o",
            &elsewhere.to_string_lossy(),
        ],
    );
    exits(
        &cli(&sandbox, &dir, &["rm", "feat/o"]),
        3,
        "not made by paddock",
    );
    assert!(elsewhere.is_dir());

    exits(&cli(&sandbox, &dir, &["rm", "main"]), 3, "main checkout");
    exits(
        &cli(
            &sandbox,
            &dir,
            &[
                "rm",
                &dir.to_string_lossy(),
                "--discard-changes",
                "--discard-commits",
            ],
        ),
        3,
        "main checkout",
    );
    assert!(dir.join(".git").is_dir());
}

#[test]
fn rm_never_deletes_the_tree_its_caller_or_a_busy_claude_is_in() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let out = cli(&sandbox, &dir, &["new", "feat/c"]);
    exits(&out, 0, "");
    let path = real(&worktree(&sandbox, "feat/c"));

    // Run from inside it (a subdirectory too).
    std::fs::create_dir(path.join("sub")).unwrap();
    exits(
        &cli(&sandbox, &path.join("sub"), &["rm", "feat/c"]),
        3,
        "current directory",
    );

    // Run from its space's terminal, with the directory elsewhere.
    let id = session_id(&sandbox, "repo/feat/c");
    let tmux = sandbox.display(&id, "#{socket_path},#{pid},0");
    let pane = sandbox.display(&id, "#{pane_id}");
    let out = sandbox.run_in(
        &dir,
        &["worktree", "rm", "feat/c"],
        &[("TMUX", &tmux), ("TMUX_PANE", &pane)],
        "",
    );
    exits(&out, 3, "calling terminal");

    // A Claude running in its space.
    claude_runs_in(&sandbox, "repo/feat/c", &path);
    exits(
        &cli(
            &sandbox,
            &dir,
            &["rm", "feat/c", "--discard-changes", "--discard-commits"],
        ),
        3,
        "agent running in space repo/feat/c",
    );
    assert!(path.is_dir());
    assert_eq!(sandbox.session_names(), ["repo/feat/c"]);
}

#[test]
fn rm_goes_on_with_the_right_discard_flags_and_keeps_an_unmerged_branch() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);

    let dirty = made(&sandbox, &dir, "feat/dirty");
    std::fs::write(dirty.join("new.txt"), "x\n").unwrap();
    let out = cli(
        &sandbox,
        &dir,
        &["rm", "feat/dirty", "--discard-changes", "--json"],
    );
    exits(&out, 0, "");
    assert_eq!(
        json(&out),
        serde_json::json!({
            "path": dirty.to_string_lossy(),
            "branch": "feat/dirty",
            "closed": [],
            "branch_deleted": true,
        })
    );
    assert!(!dirty.exists());
    assert!(!has_branch(&dir, "feat/dirty"));

    // Commits not in main: the worktree goes, `branch -d` keeps the branch.
    let out = cli(&sandbox, &dir, &["new", "feat/ahead"]);
    exits(&out, 0, "");
    let ahead = real(&worktree(&sandbox, "feat/ahead"));
    commit(&ahead, "ahead");
    let out = cli(
        &sandbox,
        &dir,
        &["rm", "feat/ahead", "--discard-commits", "--json"],
    );
    exits(&out, 0, "");
    assert!(stderr(&out).contains("branch feat/ahead kept"), "{out:?}");
    assert_eq!(json(&out)["closed"], serde_json::json!(["repo/feat/ahead"]));
    assert_eq!(json(&out)["branch_deleted"], false);
    assert!(!ahead.exists());
    assert!(has_branch(&dir, "feat/ahead"), "never -D");
    assert!(sandbox.session_names().is_empty());
}

#[test]
fn help_lists_the_worktree_commands() {
    let sandbox = Sandbox::new();
    let home = sandbox.path("home");
    for args in [&["--help"][..], &["worktree", "--help"]] {
        let out = sandbox.run_in(&home, args, &[], "");
        assert!(out.status.success(), "{out:?}");
        let text = stdout(&out);
        for word in ["worktree", "new", "open", "list", "rm"] {
            assert!(text.contains(word), "{args:?}: {text}");
        }
    }
    let out = sandbox.run_in(&home, &["worktree", "--help"], &[], "");
    assert!(stdout(&out).contains("4  timeout"), "{out:?}");
}

#[test]
fn rm_counts_a_busy_claude_in_a_float_and_list_leaves_floats_out() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let out = cli(&sandbox, &dir, &["new", "feat/f"]);
    exits(&out, 0, "");
    let path = real(&worktree(&sandbox, "feat/f"));
    // A float of the space, marked as paddock marks one.
    let id = session_id(&sandbox, "repo/feat/f");
    let target = format!("{id}:");
    let path_arg = path.to_string_lossy();
    sandbox.tmux(&[
        "new-window",
        "-d",
        "-n",
        "fl",
        "-t",
        &target,
        "-c",
        &path_arg,
    ]);
    let float = format!("{id}:fl");
    sandbox.tmux(&["set-option", "-w", "-t", &float, "@paddock-float", "G"]);
    let payload = std::fs::read_to_string(format!("{SPIKE}/UserPromptSubmit.json"))
        .unwrap()
        .replace("\"/tmp/repo\"", &format!("{:?}", path.display()));
    sandbox.hook(&float, "UserPromptSubmit", &payload);

    exits(
        &cli(&sandbox, &dir, &["rm", "feat/f"]),
        3,
        "agent running in space repo/feat/f",
    );
    assert!(path.is_dir());
    assert_eq!(sandbox.session_names(), ["repo/feat/f"]);

    // The sidebar does not follow Claude in a float, nor does list.
    let out = cli(&sandbox, &dir, &["list", "--json"]);
    exits(&out, 0, "");
    let rows = json(&out);
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["branch"] == "feat/f")
        .unwrap()
        .clone();
    assert_eq!(row["agents"], serde_json::json!([]));
}

#[test]
fn a_name_that_is_one_worktree_s_path_and_another_s_branch_names_neither() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    // Branch feat/b's worktree sits at `<repo>/feat-b`; branch feat-b's
    // where paddock puts it.
    let at_b = dir.join("feat-b");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/b",
            &at_b.to_string_lossy(),
        ],
    );
    let dash = made(&sandbox, &dir, "feat-b");
    // From home, `feat-b` is a path only against `--from`.
    let home = sandbox.path("home");
    let from = dir.to_string_lossy();
    for command in ["rm", "open"] {
        let out = cli(&sandbox, &home, &[command, "feat-b", "--from", &from]);
        exits(&out, 1, "names two worktrees");
        let err = stderr(&out);
        assert!(err.contains(&*real(&at_b).to_string_lossy()), "{err}");
        assert!(err.contains(&*dash.to_string_lossy()), "{err}");
    }
    assert!(at_b.is_dir() && dash.is_dir());
    assert!(sandbox.session_names().is_empty());
    // The full path names one.
    let out = cli(
        &sandbox,
        &home,
        &["rm", &dash.to_string_lossy(), "--from", &from],
    );
    exits(&out, 0, "");
    assert!(!dash.exists());
}

/// `feat/x`'s pull request as the stub `gh` gives it: open, checks
/// failing, review required.
const PRS: &str = r#"[
  {"number":123,"state":"OPEN","isDraft":false,"headRefName":"feat/x",
   "reviewDecision":"REVIEW_REQUIRED","url":"https://github.com/o/r/pull/123",
   "statusCheckRollup":[{"__typename":"CheckRun","name":"test","workflowName":"CI",
     "status":"COMPLETED","conclusion":"FAILURE","startedAt":"2026-10-22T10:00:00Z"}]}
]"#;

/// The `pr` of each row of `list --json` run in `dir`, by branch, after
/// checking it exits 0 with nothing on stderr.
fn listed_prs(sandbox: &Sandbox, dir: &Path) -> Vec<(Value, Value)> {
    let out = cli(sandbox, dir, &["list", "--json"]);
    exits(&out, 0, "");
    assert_eq!(stderr(&out), "", "list says nothing of gh");
    let rows = json(&out);
    let rows = rows.as_array().unwrap();
    for r in rows {
        let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
        let at = |k: &str| keys.iter().position(|x| *x == k).unwrap();
        assert_eq!(at("pr"), at("merged") + 1, "pr right after merged");
    }
    rows.iter()
        .map(|r| (r["branch"].clone(), r["pr"].clone()))
        .collect()
}

#[test]
fn list_gives_each_branchs_pr_from_one_gh_call() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    let dir = repo(&sandbox);
    let x = made(&sandbox, &dir, "feat/x");
    made(&sandbox, &dir, "feat/y");
    let prs = listed_prs(&sandbox, &x);
    use serde_json::json as j;
    assert_eq!(
        prs,
        [
            (j!("main"), j!(null)),
            (
                j!("feat/x"),
                j!({"number": 123, "state": "open", "checks": "failing",
                    "review": "review_required", "url": "https://github.com/o/r/pull/123"})
            ),
            (j!("feat/y"), j!(null)),
        ]
    );
    // One call, in the main checkout, from a linked worktree.
    assert_eq!(
        sandbox.gh_calls(),
        [format!(
            "{} pr list --state all --limit 20 --json \
             number,state,isDraft,headRefName,statusCheckRollup,reviewDecision,url",
            real(&dir).display()
        )]
    );
    let out = cli(&sandbox, &dir, &["list"]);
    exits(&out, 0, "");
    let text = stdout(&out);
    let line = |branch: &str| {
        text.lines()
            .find(|l| l.split_whitespace().nth(1) == Some(branch))
            .unwrap_or_else(|| panic!("no {branch}: {text}"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(
        line("feat/x").ends_with("clean merged #123 open -"),
        "{text}"
    );
    assert!(line("feat/y").ends_with("clean merged - -"), "{text}");
    assert!(line("main").ends_with("clean merged - -"), "{text}");
}

#[test]
fn list_without_gh_or_with_pr_status_off_gives_no_pr_and_exits_0() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    made(&sandbox, &dir, "feat/x");
    // A gh that fails (logged out): no PR, nothing said.
    let none = |prs: Vec<(Value, Value)>| prs.iter().all(|(_, pr)| pr.is_null());
    assert!(none(listed_prs(&sandbox, &dir)));
    assert_eq!(sandbox.gh_calls().len(), 1);
    // Off: gh is never run, whatever it would say.
    sandbox.set_gh(PRS);
    sandbox.write_config("[github]\nrefresh_secs = 0\n");
    assert!(none(listed_prs(&sandbox, &dir)));
    assert_eq!(sandbox.gh_calls().len(), 1, "not run");
    // No gh at all.
    sandbox.write_config("");
    sandbox.remove_gh();
    assert!(none(listed_prs(&sandbox, &dir)));
    let out = cli(&sandbox, &dir, &["list"]);
    exits(&out, 0, "");
    assert_eq!(stderr(&out), "");
}

#[test]
fn list_gives_up_on_a_slow_gh_after_5_s() {
    let sandbox = Sandbox::new();
    sandbox.set_gh(PRS);
    // The stub hangs for 30 s.
    sandbox.slow_gh();
    let dir = repo(&sandbox);
    made(&sandbox, &dir, "feat/x");
    let start = std::time::Instant::now();
    let prs = listed_prs(&sandbox, &dir);
    let took = start.elapsed();
    assert!(prs.iter().all(|(_, pr)| pr.is_null()), "{prs:?}");
    assert!(took >= std::time::Duration::from_secs(5), "{took:?}");
    assert!(took < std::time::Duration::from_secs(10), "{took:?}");
}
