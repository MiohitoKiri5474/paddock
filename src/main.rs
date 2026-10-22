//! paddock: a terminal workspace for running coding agents (Claude Code).

mod app;
mod cli;
mod config;
mod fuzzy;
mod git;
mod hooks;
mod input;
mod instance;
mod paths;
mod sessions;
mod terminal;
mod tmux;
mod ui;
mod worktree;

const USAGE: &str = "\
usage: paddock [command]

commands:
  (none)                      open paddock
  help, -h, --help            print this help
  version, -V, --version      print paddock's version
  keys                        print the default keys as TOML
  hooks install|uninstall     add or remove paddock's agent hooks
    [--agent <kind>]...       only for these agents (claude)
  worktree new|open|list|rm   manage worktrees as spaces, without the UI
                              (paddock worktree --help)
";

/// `paddock <version>` for `version`, `-V` or `--version` alone, else
/// `None`.
fn version(args: &[&str]) -> Option<String> {
    matches!(args, ["version" | "-V" | "--version"])
        .then(|| format!("paddock {}", env!("CARGO_PKG_VERSION")))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Some(line) = version(&argv) {
        println!("{line}");
        return Ok(());
    }
    match argv[..] {
        [] => app::run(),
        ["-h" | "--help" | "help"] => {
            print!("{USAGE}");
            Ok(())
        }
        ["keys"] => {
            print!("{}", config::default_keys_toml());
            Ok(())
        }
        ["hook", ..] => hooks::hook(&argv[1..]),
        ["hooks", ..] => hooks::command(&argv[1..]),
        ["worktree", ..] => std::process::exit(cli::run(&args[1..])),
        _ => anyhow::bail!("{}", USAGE.trim_end()),
    }
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_prints_the_cargo_version() {
        let expected = format!("paddock {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(version(&["--version"]), Some(expected.clone()));
        assert_eq!(version(&["-V"]), Some(expected.clone()));
        assert_eq!(version(&["version"]), Some(expected));
    }

    #[test]
    fn version_needs_to_stand_alone() {
        assert_eq!(version(&[]), None);
        assert_eq!(version(&["-v"]), None);
        assert_eq!(version(&["version", "x"]), None);
        assert_eq!(version(&["--version", "x"]), None);
        assert_eq!(version(&["keys", "--version"]), None);
    }
}
