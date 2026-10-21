//! paddock: a terminal workspace for running Claude Code.

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
  keys                        print the default keys as TOML
  hooks install|uninstall     add or remove paddock's Claude Code hooks
  worktree new|open|list|rm   manage worktrees as spaces, without the UI
                              (paddock worktree --help)
";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => app::run(),
        ["-h" | "--help" | "help"] => {
            print!("{USAGE}");
            Ok(())
        }
        ["keys"] => {
            print!("{}", config::default_keys_toml());
            Ok(())
        }
        ["hook", event] => hooks::hook(event),
        ["hooks", action] => hooks::command(action),
        ["worktree", ..] => std::process::exit(cli::run(&args[1..])),
        _ => anyhow::bail!("{}", USAGE.trim_end()),
    }
}
