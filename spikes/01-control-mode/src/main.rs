//! Spike 01: Claude Code through tmux control mode and our own emulator.
//! Throwaway code; see RESULTS.md and docs/adr/0005.

mod checks;
mod emu;
mod keys;
mod pane;
mod tmux;
mod tui;

use anyhow::Result;

const USAGE: &str = "usage:
  paddock-spike-01 tui [--emu yas|alacritty|vt100] [--session NAME] [--cmd CMD]
  paddock-spike-01 check render|history|claude|claude-esc|bench|all  (claude-esc submits one no-tools prompt)
  paddock-spike-01 kill-server";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(String::as_str) {
        Some("tui") => {
            let emu = opt("--emu").unwrap_or_else(|| "yas".into());
            let session = opt("--session").unwrap_or_else(|| "main".into());
            tui::run(&emu, &session, opt("--cmd").as_deref())
        }
        Some("check") => {
            let what = args.get(1).map(String::as_str).unwrap_or("all");
            let all = what == "all";
            if all || what == "render" {
                checks::render()?;
            }
            if all || what == "history" {
                checks::history()?;
            }
            if what == "claude" {
                checks::claude(false)?;
            }
            if all || what == "claude-esc" {
                checks::claude(true)?;
            }
            if all || what == "bench" {
                checks::bench()?;
            }
            Ok(())
        }
        Some("kill-server") => {
            tmux::kill_server();
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
