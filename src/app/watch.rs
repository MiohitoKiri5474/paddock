//! Background threads that feed session information into the event loop:
//! new lines of the hook event log, and `claude agents --json` snapshots.

use super::Event;
use crate::hooks::now_ms;
use crate::sessions::{agents, log};
use crate::tmux::Server;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// How often to look for new hook events.
const LOG_POLL: Duration = Duration::from_millis(200);
/// How often to ask Claude which sessions run (catches what hooks miss).
const AGENTS_POLL: Duration = Duration::from_secs(3);

/// Start both threads. They end when the event loop goes away.
pub fn start(server: &Server, state_dir: &Path, tx: &Sender<Event>) -> Result<()> {
    let pid: u32 = server
        .run(&["display-message", "-p", "#{pid}"])?
        .trim()
        .parse()
        .context("cannot read the tmux server's pid")?;
    let mut reader = log::Reader::new(log::path(state_dir, pid));
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let records = reader.read_new();
        if !records.is_empty() && events.send(Event::Hooks(records)).is_err() {
            break;
        }
        std::thread::sleep(LOG_POLL);
    });

    let server = server.clone();
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let taken = now_ms();
        if let Some(list) = agents::fetch(&server) {
            if events.send(Event::Agents(taken, list)).is_err() {
                break;
            }
        }
        std::thread::sleep(AGENTS_POLL);
    });
    Ok(())
}
