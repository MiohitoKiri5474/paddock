//! `paddock worktree`: what `C-a W`, `C-a O` and sidebar `x` do, for an
//! agent or a script. It drives paddock's backend directly, so it
//! works whether the app is open or not (a running paddock attaches the
//! spaces it makes and drops the ones it closes). Nothing prompts, reads
//! stdin or needs a terminal: output goes to stdout (`--json` for a
//! stable shape), each error is one line on stderr, and the exit code
//! says what kind of failure it was ([`Code`]).

mod rm;
mod worktree;

use crate::worktree::{Failure, Risks};
use std::path::PathBuf;

/// `paddock worktree --help`.
pub const HELP: &str = "\
usage: paddock worktree <command> [--json] [--from <dir>]

commands:
  new <branch> [--no-space]   check <branch> out in a new worktree under
                              [worktrees] directory and open a space there;
                              prints its path
  open <branch|path>          open a space for a worktree that has none
  list                        the repository's worktrees, one per line
  rm <branch|path> [--discard-changes] [--discard-commits]
                              close its spaces, remove the worktree and
                              delete the branch if merged (git branch -d)

options:
  --json        print JSON instead of text
  --from <dir>  the repository of <dir>, not of the current directory

exit codes:
  0  ok
  1  usage or config error
  2  git or backend error (git's reason on stderr)
  3  refused for a risk (stderr names it); nothing changed
  4  timeout; nothing deleted
";

/// How `paddock worktree` ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Ok = 0,
    Usage = 1,
    GitOrBackend = 2,
    Refused = 3,
    Late = 4,
}

/// A failure of the command: its exit code and its one-line message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub code: Code,
    pub message: String,
}

impl Fail {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<Failure> for Fail {
    fn from(f: Failure) -> Self {
        let code = match f {
            Failure::Invalid(_) => Code::Usage,
            Failure::Git(_) | Failure::Backend(_) => Code::GitOrBackend,
            Failure::Refused(_) => Code::Refused,
            Failure::Late(_) => Code::Late,
        };
        Self::new(code, f.to_string())
    }
}

/// One `paddock worktree` subcommand, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    New { branch: String, no_space: bool },
    Open { target: String },
    List,
    Rm { target: String, discard: Discard },
}

/// What `rm` was told it may lose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Discard {
    /// `--discard-changes`: uncommitted changes (and `--force`).
    pub changes: bool,
    /// `--discard-commits`: commits not in the base.
    pub commits: bool,
}

/// A parsed `paddock worktree …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub json: bool,
    /// `--from`: whose repository; the current directory without it.
    pub from: Option<PathBuf>,
}

/// What the arguments ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Run(Invocation),
}

/// Parse the arguments after `worktree`. A usage error says what is wrong.
pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut json = false;
    let mut from = None;
    let mut no_space = false;
    let mut discard = Discard::default();
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            // Not after a command: `new help` makes a branch `help`.
            "help" if words.is_empty() => return Ok(Parsed::Help),
            "--json" => json = true,
            "--no-space" => no_space = true,
            "--discard-changes" => discard.changes = true,
            "--discard-commits" => discard.commits = true,
            "--from" => {
                let dir = args.next().ok_or("--from needs a directory")?;
                from = Some(PathBuf::from(dir));
            }
            a if a.starts_with("--from=") => from = Some(PathBuf::from(&a["--from=".len()..])),
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            a => words.push(a.to_string()),
        }
    }
    let Some((name, rest)) = words.split_first() else {
        return Err("missing command (new, open, list or rm)".into());
    };
    let one = |what: &str| match rest {
        [x] => Ok(x.clone()),
        [] => Err(format!("{name} needs a {what}")),
        _ => Err(format!("{name} takes one {what}")),
    };
    let command = match name.as_str() {
        "new" => Command::New {
            branch: one("branch")?,
            no_space,
        },
        "open" => Command::Open {
            target: one("branch or path")?,
        },
        "list" if rest.is_empty() => Command::List,
        "list" => return Err("list takes no arguments".into()),
        "rm" => Command::Rm {
            target: one("branch or path")?,
            discard,
        },
        other => return Err(format!("unknown command {other:?}")),
    };
    let misplaced = match &command {
        Command::New { .. } => discard != Discard::default(),
        Command::Rm { .. } => no_space,
        Command::Open { .. } | Command::List => no_space || discard != Discard::default(),
    };
    if misplaced {
        return Err(format!("an option {name} does not take"));
    }
    Ok(Parsed::Run(Invocation {
        command,
        json,
        from,
    }))
}

/// Whether `rm` may go on, given what deleting would lose (`risks`, see
/// [`crate::worktree::risks_within`]) and what it was told to discard.
/// A check that ran out of time refuses first (exit 4): nothing is known.
/// Otherwise every risk not discarded is named (exit 3). On success,
/// returns what the delete may lose, for the re-check right before it:
/// only the risks found and discarded, never a blanket consent.
pub fn allowed(
    label: &str,
    risks: Option<&Risks>,
    detached: bool,
    discard: Discard,
) -> Result<Risks, Fail> {
    if risks.is_some_and(|r| r.late) {
        return Err(Fail::new(
            Code::Late,
            format!("couldn't check {label} in time: nothing deleted"),
        ));
    }
    let mut named = Vec::new();
    if let Some(r) = risks {
        if r.dirty && !discard.changes {
            named.push("has uncommitted changes (--discard-changes loses them)".to_string());
        }
        if let (Some(base), false) = (&r.unmerged, discard.commits) {
            // A branch keeps its commits (`branch -d` keeps it); only a
            // detached HEAD's go with the worktree.
            named.push(if detached {
                format!(
                    "has commits not in {base} on a detached HEAD (--discard-commits loses them)"
                )
            } else {
                format!(
                    "has commits not in {base} (--discard-commits removes the worktree \
                     anyway; the branch is kept)"
                )
            });
        }
    }
    if !named.is_empty() {
        return Err(Fail::new(
            Code::Refused,
            format!("{label} {}: nothing deleted", named.join(" and ")),
        ));
    }
    // Consent covers only what the check found: changes or commits
    // made after it are checked again before removing, and refused.
    Ok(Risks {
        dirty: discard.changes && risks.is_some_and(|r| r.dirty),
        unmerged: risks
            .and_then(|r| r.unmerged.clone())
            .filter(|_| discard.commits),
        late: false,
    })
}

/// Run `paddock worktree` with `args` (after `worktree`); returns the
/// exit code. Prints its output and errors itself.
pub fn run(args: &[String]) -> i32 {
    let invocation = match parse(args) {
        Ok(Parsed::Help) => {
            print!("{HELP}");
            return Code::Ok as i32;
        }
        Ok(Parsed::Run(i)) => i,
        Err(e) => {
            eprintln!("paddock: {e} (paddock worktree --help)");
            return Code::Usage as i32;
        }
    };
    match worktree::run(&invocation) {
        Ok(()) => Code::Ok as i32,
        Err(f) => {
            eprintln!("paddock: {}", one_line(&f.message));
            f.code as i32
        }
    }
}

/// `text` on one line: line breaks become `; `.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(line: &str) -> Result<Parsed, String> {
        let args: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        parse(&args)
    }

    fn run_of(line: &str) -> Invocation {
        match parse_words(line) {
            Ok(Parsed::Run(i)) => i,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn subcommands_and_their_options_parse() {
        let i = run_of("new feat/x --json --from /r --no-space");
        assert_eq!(
            i,
            Invocation {
                command: Command::New {
                    branch: "feat/x".into(),
                    no_space: true
                },
                json: true,
                from: Some("/r".into()),
            }
        );
        assert_eq!(
            run_of("open --from=/r feat/x").command,
            Command::Open {
                target: "feat/x".into()
            }
        );
        assert_eq!(run_of("list").command, Command::List);
        assert!(!run_of("list").json);
        assert_eq!(
            run_of("rm feat/x --discard-commits").command,
            Command::Rm {
                target: "feat/x".into(),
                discard: Discard {
                    changes: false,
                    commits: true
                }
            }
        );
    }

    #[test]
    fn help_is_asked_for_in_any_spelling() {
        for line in ["--help", "-h", "help", "rm --help", "new -h"] {
            assert_eq!(parse_words(line), Ok(Parsed::Help), "{line}");
        }
    }

    #[test]
    fn bad_arguments_are_usage_errors() {
        for line in [
            "",
            "new",
            "new a b",
            "list x",
            "rm",
            "frob",
            "new x --bogus",
            "new x --discard-changes",
            "open x --no-space",
            "rm x --no-space",
            "list --from",
        ] {
            assert!(parse_words(line).is_err(), "{line:?} parsed");
        }
    }

    fn risks(dirty: bool, unmerged: Option<&str>) -> Risks {
        Risks {
            dirty,
            unmerged: unmerged.map(str::to_string),
            late: false,
        }
    }

    const NONE: Discard = Discard {
        changes: false,
        commits: false,
    };
    const ALL: Discard = Discard {
        changes: true,
        commits: true,
    };

    #[test]
    fn each_risk_not_discarded_refuses_with_exit_3_naming_it() {
        let dirty = allowed("feat/x", Some(&risks(true, None)), false, NONE).unwrap_err();
        assert_eq!(dirty.code, Code::Refused);
        assert!(dirty.message.contains("uncommitted changes"), "{dirty:?}");
        assert!(dirty.message.contains("--discard-changes"), "{dirty:?}");
        let both = allowed("feat/x", Some(&risks(true, Some("main"))), false, NONE).unwrap_err();
        assert_eq!(
            both.message,
            "feat/x has uncommitted changes (--discard-changes loses them) and has \
             commits not in main (--discard-commits removes the worktree anyway; the \
             branch is kept): nothing deleted"
        );
        // Only a detached HEAD loses its commits with the worktree.
        let detached = allowed("loose", Some(&risks(false, Some("main"))), true, NONE);
        assert_eq!(
            detached.unwrap_err().message,
            "loose has commits not in main on a detached HEAD (--discard-commits loses \
             them): nothing deleted"
        );
        // Discarding one risk leaves the other refused.
        let changes = Discard {
            changes: true,
            commits: false,
        };
        let left = allowed("feat/x", Some(&risks(true, Some("main"))), false, changes).unwrap_err();
        assert_eq!(left.code, Code::Refused);
        assert!(!left.message.contains("uncommitted"), "{left:?}");
    }

    #[test]
    fn discarded_or_absent_risks_go_on_and_confirm_only_what_was_found() {
        assert_eq!(allowed("x", None, false, NONE), Ok(risks(false, None)));
        let ok = allowed("x", Some(&risks(true, Some("main"))), false, ALL).unwrap();
        assert_eq!(ok, risks(true, Some("main")));
        // Flags on a clean, merged tree consent to nothing: changes or
        // commits made after the check are refused by the re-check (and
        // never `--force`d).
        assert_eq!(allowed("x", None, false, ALL), Ok(risks(false, None)));
        let only_dirty = allowed("x", Some(&risks(true, None)), false, ALL).unwrap();
        assert_eq!(only_dirty, risks(true, None));
        let only_ahead = allowed("x", Some(&risks(false, Some("main"))), false, ALL).unwrap();
        assert_eq!(only_ahead, risks(false, Some("main")));
    }

    #[test]
    fn a_risk_check_past_its_deadline_exits_4_whatever_was_discarded() {
        for discard in [NONE, ALL] {
            let late = allowed("feat/x", Some(&Risks::unknown()), false, discard).unwrap_err();
            assert_eq!(late.code, Code::Late);
            assert_eq!(
                late.message,
                "couldn't check feat/x in time: nothing deleted"
            );
        }
    }

    #[test]
    fn failures_map_to_their_exit_codes() {
        let code = |f: Failure| Fail::from(f).code;
        assert_eq!(code(Failure::Invalid("x".into())), Code::Usage);
        assert_eq!(code(Failure::Git("x".into())), Code::GitOrBackend);
        assert_eq!(code(Failure::Backend("x".into())), Code::GitOrBackend);
        assert_eq!(code(Failure::Refused("x".into())), Code::Refused);
        assert_eq!(code(Failure::Late("x".into())), Code::Late);
        assert_eq!(Code::Late as i32, 4);
    }

    #[test]
    fn errors_print_on_one_line() {
        assert_eq!(one_line("a\n  b\n\nc"), "a; b; c");
    }
}
