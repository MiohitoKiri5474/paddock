//! The main loop: feed terminal output, route input, redraw at most once a frame.

use super::attention::tab_states;
use super::input::Flow;
use super::layout::Split;
use super::screen::Screen;
use super::state::{session_area, App, Mode, TAB_BAR_ROWS};
use super::{watch, Event};
use crate::tmux::Message;
use crate::ui::{help, session_view, sidebar, tab_bar::TabBar};
use anyhow::Result;
use crossterm::event::{self as ct, KeyEventKind};
use ratatui::layout::Rect;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Minimum time between two draws (about 60 frames a second).
const FRAME: Duration = Duration::from_millis(16);

/// Tells the git thread which task rows are on screen.
struct GitFeed {
    tx: Sender<watch::Tasks>,
    sent: watch::Tasks,
}

impl GitFeed {
    /// Send the tasks on screen if they changed, or always if `now`.
    fn update(&mut self, app: &App, now: bool) {
        let tasks = app.visible_tasks();
        if now || tasks != self.sent {
            // The thread only ends with the app.
            let _ = self.tx.send(tasks.clone());
            self.sent = tasks;
        }
    }
}

pub fn run(
    screen: &mut Screen,
    mut app: App,
    rx: &Receiver<Event>,
    git: Sender<watch::Tasks>,
) -> Result<()> {
    let mut git = GitFeed {
        tx: git,
        sent: Vec::new(),
    };
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
        // Hook events refresh git; anything else but terminal output and git
        // itself may change which task rows are on screen.
        let hooks = matches!(event, Event::Hooks(_));
        let rows_may_move = !matches!(event, Event::Tmux { .. } | Event::Git(_));
        let (flow, changed) = handle(&mut app, event, screen)?;
        dirty |= changed;
        if rows_may_move {
            git.update(&app, hooks);
        }
        if flow == Flow::Quit {
            break;
        }
    }
    app.detach_all()
}

/// Apply one event. Returns whether to go on and whether the screen may have changed.
fn handle(app: &mut App, event: Event, screen: &mut Screen) -> Result<(Flow, bool)> {
    let flow = match event {
        Event::Tmux {
            session,
            msg: Message::Output { data, .. },
        } => {
            if let Some(t) = app.terminals.get_mut(&session) {
                t.output(&data);
            }
            return Ok((Flow::Continue, app.shows(&session)));
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
        Event::Hooks(records) => {
            app.hooks(&records);
            app.relayout()?;
            Flow::Continue
        }
        Event::Agents(taken, agents) => {
            app.sessions.reconcile(taken, &agents);
            app.relayout()?;
            Flow::Continue
        }
        Event::Git(read) => {
            app.task_status.extend(read);
            Flow::Continue
        }
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
        Event::Input(ct::Event::FocusGained) => {
            app.attention.window_focused = true;
            return Ok((Flow::Continue, false));
        }
        Event::Input(ct::Event::FocusLost) => {
            app.attention.window_focused = false;
            return Ok((Flow::Continue, false));
        }
        Event::Input(_) => return Ok((Flow::Continue, false)),
        Event::Detach => Flow::Quit,
    };
    Ok((flow, true))
}

fn draw(screen: &mut Screen, app: &App) -> Result<()> {
    screen.draw(|f| {
        let full = f.area();
        let rows = app.rows();
        let side = app.sidebar_width().min(full.width);
        if side > 0 {
            let selected = match &app.mode {
                Mode::Sidebar(pane) => pane.as_deref(),
                _ => None,
            };
            let side_area = Rect {
                width: side,
                ..full
            };
            sidebar::render(&rows, selected, side_area, f.buffer_mut());
        }
        let area = Rect {
            x: full.x + side,
            width: full.width - side,
            ..full
        };
        let bar_height = TAB_BAR_ROWS.min(area.height);
        let bar_area = Rect {
            height: bar_height,
            ..area
        };
        let term_area = session_area((full.width, full.height), side);
        let armed = app.prefix.armed().then_some(app.prefix_name.as_str());
        let ask = match &app.mode {
            Mode::ConfirmClose { state, .. } => {
                Some(format!("Claude is {}: close? y/n", state.label()))
            }
            _ => None,
        };
        let bar = TabBar {
            // The question takes the whole bar, so it always fits.
            names: match ask {
                Some(_) => Vec::new(),
                None => app.tabs.iter().map(|t| t.name.as_str()).collect(),
            },
            current: app.tabs.current_index(),
            rename: match &app.mode {
                Mode::Rename(text) => Some(text.as_str()),
                _ => None,
            },
            status: ask.as_deref().or(app.notice.as_deref()).or(armed),
            states: tab_states(&rows, app.tabs.iter().count()),
        };
        let bar_cursor = bar.render(bar_area, f.buffer_mut());
        let mut term_cursor = None;
        if let Some(tab) = app.tabs.current() {
            let (panes, borders) = tab.content.rects(term_area);
            for (id, rect) in panes {
                let Some(t) = app.terminals.get(&id) else {
                    continue;
                };
                let cursor = session_view::render(t.emulator(), rect, f.buffer_mut());
                if id == tab.content.focus {
                    term_cursor = cursor;
                }
            }
            for b in borders {
                session_view::border(b.area, b.split == Split::Columns, f.buffer_mut());
            }
        }
        if let Mode::Help = app.mode {
            help::render(&app.prefix_name, full, f.buffer_mut());
        } else if let Mode::Sidebar(_) = app.mode {
            // The sidebar has the keyboard: no terminal cursor.
        } else if let Some(cursor) = bar_cursor.or(term_cursor) {
            f.set_cursor_position(cursor);
        }
    })?;
    Ok(())
}
