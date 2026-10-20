//! paddock: a terminal workspace for running Claude Code.

mod app;
mod config;
mod git;
mod hooks;
mod input;
mod instance;
mod paths;
mod sessions;
mod terminal;
mod tmux;
mod ui;

const USAGE: &str = "usage: paddock [hooks install|hooks uninstall]";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => app::run(),
        ["hook", event] => hooks::hook(event),
        ["hooks", action] => hooks::command(action),
        _ => anyhow::bail!(USAGE),
    }
}
