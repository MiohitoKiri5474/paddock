//! paddock: a terminal workspace for running coding agents (Claude Code,
//! Gemini CLI).

mod app;
mod backend;
mod cli;
mod config;
mod debug;
mod desktop;
mod fuzzy;
mod git;
mod github;
mod hooks;
mod input;
mod instance;
mod paths;
mod sessions;
mod shell;
mod terminal;
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
    [--agent <kind>]...       only for these agents (claude, gemini)
  worktree new|open|list|rm   manage worktrees as spaces, without the UI
                              (paddock worktree --help)
  server upgrade [<binary>]   hand the running paddock server's terminals
                              to this paddock (or <binary>), programs
                              running on
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
        // Hidden from the usage: for tests and bug reports.
        ["debug", "state", "--json"] => debug::print_state(),
        // Hidden from the usage: paddock starts it (ADR 0007).
        ["server"] => backend::serve(),
        ["server", "upgrade"] => upgrade_server(None),
        ["server", "upgrade", binary] => upgrade_server(Some(binary)),
        // Hidden from the usage: what a server runs as it hands over
        // (ADR 0009).
        ["server", "--check-handover"] => backend::check_handover(),
        ["server", "--take-over", record, conn] => backend::take_over(record, conn),
        _ => anyhow::bail!("{}", USAGE.trim_end()),
    }
}

/// `paddock server upgrade [<binary>]`: hand the running native server's
/// terminals to `binary`, this one by default, whatever protocol version
/// it speaks (ADR 0009).
fn upgrade_server(binary: Option<&str>) -> anyhow::Result<()> {
    use anyhow::Context;
    let exe = match binary {
        Some(b) => std::path::absolute(b).with_context(|| format!("no path {b}"))?,
        None => std::env::current_exe().context("cannot find paddock's own binary")?,
    };
    let paths = paths::Paths::from_env()?;
    let backend = backend::Backend::new(paths, config::BackendKind::Native);
    let pid = match backend.upgrade(&exe) {
        Ok(pid) => pid,
        Err(e) if e.downcast_ref::<backend::NoHandover>().is_some() => anyhow::bail!(
            "{e:#}; to restart it instead, start paddock and answer yes when it \
             offers to (its programs end, and its layout comes back)"
        ),
        Err(e) => return Err(e),
    };
    println!(
        "the paddock server (pid {pid}) now runs {}, its programs running on",
        exe.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{version, USAGE};

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

    #[test]
    fn the_debug_command_is_not_in_the_usage() {
        assert!(!USAGE.contains("debug"), "{USAGE}");
    }
}
