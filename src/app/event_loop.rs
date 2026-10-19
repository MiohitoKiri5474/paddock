//! The main loop: feed terminal output, route input, redraw at most once a frame.

use super::screen::Screen;
use super::Event;
use crate::input::{encode_key, encode_paste, route_mouse, KeyAction, MouseAction, PrefixKeys};
use crate::terminal::Terminal;
use crate::tmux::Message;
use crate::ui::session_view;
use anyhow::Result;
use crossterm::event::{self as ct, KeyEventKind};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Minimum time between two draws (about 60 frames a second).
const FRAME: Duration = Duration::from_millis(16);

enum Flow {
    Continue,
    Quit,
}

pub fn run(screen: &mut Screen, mut terminal: Terminal, rx: &Receiver<Event>) -> Result<()> {
    let mut prefix = PrefixKeys::default();
    let mut dirty = true;
    let mut last_draw = Instant::now()
        .checked_sub(FRAME)
        .unwrap_or_else(Instant::now);
    loop {
        if dirty && last_draw.elapsed() >= FRAME {
            draw(screen, &terminal)?;
            last_draw = Instant::now();
            dirty = false;
        }
        // Idle: block until something happens. Dirty: wake up for the next frame.
        let event = if dirty {
            match rx.recv_timeout(FRAME.saturating_sub(last_draw.elapsed())) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(e) => e,
                Err(_) => break,
            }
        };
        let (flow, changed) = handle(event, &mut terminal, &mut prefix, screen)?;
        dirty |= changed;
        if let Flow::Quit = flow {
            break;
        }
    }
    terminal.detach();
    Ok(())
}

/// Apply one event. Returns whether to go on and whether the screen changed.
fn handle(
    event: Event,
    terminal: &mut Terminal,
    prefix: &mut PrefixKeys,
    screen: &mut Screen,
) -> Result<(Flow, bool)> {
    match event {
        Event::Tmux(Message::Output { pane, data }) if pane == terminal.pane() => {
            terminal.output(&data);
            Ok((Flow::Continue, true))
        }
        // The terminal's tmux session ended (its shell exited) or the server went away.
        Event::Tmux(Message::Exit) => Ok((Flow::Quit, false)),
        Event::Tmux(_) => Ok((Flow::Continue, false)),
        Event::Input(ct::Event::Key(key)) if key.kind != KeyEventKind::Release => {
            match prefix.handle(key) {
                KeyAction::Detach => Ok((Flow::Quit, false)),
                KeyAction::None => Ok((Flow::Continue, false)),
                KeyAction::Forward(key) => {
                    let was_scrolled = terminal.emulator().scroll_offset() > 0;
                    if let Some(bytes) = encode_key(&key, terminal.key_modes()) {
                        terminal.input(&bytes)?;
                    }
                    Ok((Flow::Continue, was_scrolled))
                }
            }
        }
        Event::Input(ct::Event::Paste(text)) => {
            let bytes = encode_paste(&text, terminal.emulator().modes().bracketed_paste);
            terminal.input(&bytes)?;
            Ok((Flow::Continue, true))
        }
        Event::Input(ct::Event::Mouse(m)) => {
            let modes = terminal.emulator().modes();
            match route_mouse(&m, m.column, m.row, modes) {
                MouseAction::Report(bytes) => terminal.input(&bytes)?,
                MouseAction::Scroll(lines) => terminal.emulator_mut().scroll(lines),
                MouseAction::Ignore => return Ok((Flow::Continue, false)),
            }
            Ok((Flow::Continue, true))
        }
        Event::Input(ct::Event::Resize(cols, rows)) => {
            terminal.resize(cols.max(2), rows.max(1))?;
            screen.autoresize()?;
            Ok((Flow::Continue, true))
        }
        Event::Input(_) => Ok((Flow::Continue, false)),
    }
}

fn draw(screen: &mut Screen, terminal: &Terminal) -> Result<()> {
    screen.draw(|f| {
        let area = f.area();
        if let Some(cursor) = session_view::render(terminal.emulator(), area, f.buffer_mut()) {
            f.set_cursor_position(cursor);
        }
    })?;
    Ok(())
}
