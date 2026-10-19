//! paddock: a terminal workspace for running Claude Code.

mod app;
mod input;
mod paths;
mod terminal;
mod tmux;
mod ui;

fn main() -> anyhow::Result<()> {
    app::run()
}
