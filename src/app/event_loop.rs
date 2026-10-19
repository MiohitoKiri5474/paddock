//! The main loop: feed terminal output, route input, redraw at most once a frame.

use super::input::Flow;
use super::screen::Screen;
use super::state::{App, Mode, TAB_BAR_ROWS};
use super::Event;
use crate::tmux::Message;
use crate::ui::{help, session_view, tab_bar::TabBar};
use anyhow::Result;
use crossterm::event::{self as ct, KeyEventKind};
use ratatui::layout::Rect;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Minimum time between two draws (about 60 frames a second).
const FRAME: Duration = Duration::from_millis(16);

pub fn run(screen: &mut Screen, mut app: App, rx: &Receiver<Event>) -> Result<()> {
    let mut dirty = true;
    let mut last_draw = Instant::now()
        .checked_sub(FRAME)
        .unwrap_or_else(Instant::now);
    loop {
        if dirty && last_draw.elapsed() >= FRAME {
            draw(screen, &app)?;
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
        let (flow, changed) = handle(&mut app, event, screen)?;
        dirty |= changed;
        if flow == Flow::Quit {
            break;
        }
    }
    app.detach_all();
    Ok(())
}

/// Apply one event. Returns whether to go on and whether the screen may have changed.
fn handle(app: &mut App, event: Event, screen: &mut Screen) -> Result<(Flow, bool)> {
    let flow = match event {
        Event::Tmux {
            session,
            msg: Message::Output { data, .. },
        } => {
            let current = app.current().is_some_and(|t| t.session() == session);
            if let Some(tab) = app.tabs.find_mut(|t| t.session() == session) {
                tab.content.output(&data);
            }
            return Ok((Flow::Continue, current));
        }
        // That terminal's tmux session ended: its shell exited or it was closed.
        Event::Tmux {
            session,
            msg: Message::Exit,
        } => {
            if app.terminal_ended(&session) {
                Flow::Continue
            } else {
                Flow::Quit
            }
        }
        Event::Tmux { .. } => return Ok((Flow::Continue, false)),
        Event::Input(ct::Event::Key(key)) if key.kind != KeyEventKind::Release => app.key(key)?,
        Event::Input(ct::Event::Paste(text)) => {
            app.paste(&text)?;
            Flow::Continue
        }
        Event::Input(ct::Event::Mouse(m)) => return Ok((Flow::Continue, app.mouse(m)?)),
        Event::Input(ct::Event::Resize(cols, rows)) => {
            app.resize((cols, rows))?;
            screen.autoresize()?;
            Flow::Continue
        }
        Event::Input(_) => return Ok((Flow::Continue, false)),
        Event::Detach => Flow::Quit,
    };
    Ok((flow, true))
}

fn draw(screen: &mut Screen, app: &App) -> Result<()> {
    screen.draw(|f| {
        let area = f.area();
        let bar_height = TAB_BAR_ROWS.min(area.height);
        let bar_area = Rect {
            height: bar_height,
            ..area
        };
        let term_area = Rect {
            y: area.y + bar_height,
            height: area.height - bar_height,
            ..area
        };
        let armed = app.prefix.armed().then_some(app.prefix_name.as_str());
        let bar = TabBar {
            names: app.tabs.iter().map(|t| t.name.as_str()).collect(),
            current: app.tabs.current_index(),
            rename: match &app.mode {
                Mode::Rename(text) => Some(text.as_str()),
                _ => None,
            },
            status: app.notice.as_deref().or(armed),
        };
        let bar_cursor = bar.render(bar_area, f.buffer_mut());
        let term_cursor = app
            .current()
            .and_then(|t| session_view::render(t.emulator(), term_area, f.buffer_mut()));
        if let Mode::Help = app.mode {
            help::render(&app.prefix_name, area, f.buffer_mut());
        } else if let Some(cursor) = bar_cursor.or(term_cursor) {
            f.set_cursor_position(cursor);
        }
    })?;
    Ok(())
}
