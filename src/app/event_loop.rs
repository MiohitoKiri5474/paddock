//! The main loop: feed terminal output, route input, redraw at most once a frame.

use super::input::Flow;
use super::rows::Row;
use super::screen::Screen;
use super::space::changes_structure;
use super::state::{App, CloseTarget, Mode, Renaming, TAB_BAR_ROWS};
use super::{watch, Event};
use crate::hooks::now_ms;
use crate::tmux::Message;
use crate::ui::{dashboard, help, session_view, sidebar, tab_bar::TabBar};
use anyhow::Result;
use crossterm::event::{self as ct, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Minimum time between two draws (about 60 frames a second).
const FRAME: Duration = Duration::from_millis(16);
/// Milliseconds in a second.
const MS_PER_SEC: u64 = 1000;

/// Tells the git thread which task rows are on screen.
struct GitFeed {
    tx: Sender<watch::Watched>,
    sent: watch::Watched,
}

impl GitFeed {
    /// Send the tasks on screen if they changed, or always if `now`.
    fn update(&mut self, app: &App, now: bool) {
        let tasks = app.watched_tasks();
        if now || tasks != self.sent {
            // The thread only ends with the app.
            let _ = self.tx.send(tasks.clone());
            self.sent = tasks;
        }
    }
}

/// What waiting for the next event gave.
enum Wait {
    Event(Event),
    /// Nothing yet, and the next frame is due.
    FrameDue,
    /// No more events will come.
    Closed,
}

/// Idle: block until something happens. Dirty: wake up for the next frame.
fn next_event(rx: &Receiver<Event>, dirty: bool, last_draw: Instant) -> Wait {
    if !dirty {
        return rx.recv().map_or(Wait::Closed, Wait::Event);
    }
    match rx.recv_timeout(FRAME.saturating_sub(last_draw.elapsed())) {
        Ok(e) => Wait::Event(e),
        Err(RecvTimeoutError::Timeout) => Wait::FrameDue,
        Err(RecvTimeoutError::Disconnected) => Wait::Closed,
    }
}

pub fn run(
    screen: &mut Screen,
    mut app: App,
    rx: &Receiver<Event>,
    git: Sender<watch::Watched>,
) -> Result<()> {
    let mut git = GitFeed {
        tx: git,
        sent: watch::Watched::default(),
    };
    // The spaces' directories and branches are read at once.
    git.update(&app, true);
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
        let event = match next_event(rx, dirty, last_draw) {
            Wait::Event(e) => e,
            Wait::FrameDue => continue,
            Wait::Closed => break,
        };
        // Hook events refresh git; anything else but terminal output and git
        // itself may change which task rows are on screen.
        let hooks = matches!(event, Event::Hooks(_));
        let output = matches!(
            event,
            Event::Tmux {
                msg: Message::Output { .. },
                ..
            }
        );
        let rows_may_move = !matches!(event, Event::Tmux { .. } | Event::Git(_) | Event::Spaces(_));
        // A terminal can end with its exit still queued: a write to it fails
        // then. Say so, and go on rather than quit without saving.
        let (flow, changed) = handle(&mut app, event, screen).unwrap_or_else(|e| {
            app.notice = Some(format!("{e:#}"));
            (Flow::Continue, true)
        });
        dirty |= changed;
        // Any event may show a tab or bring the window back.
        app.see_current();
        if !output {
            app.settle_tabs();
        }
        if rows_may_move {
            app.anchor_dashboard();
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
            msg: Message::Output { pane, data },
        } => {
            if !app.terminals.contains_key(&pane) {
                // A pane made since the last look, maybe through `tmux
                // attach`.
                app.refresh_reported(&session);
            }
            if let Some(t) = app.terminals.get_mut(&pane) {
                t.output(&data);
            }
            let seen = app.shows(&pane) || app.shows_in_preview(&pane);
            return Ok((Flow::Continue, seen));
        }
        // That space's tmux session ended: its last pane exited or was closed.
        Event::Tmux {
            session,
            msg: Message::Exit,
        } => {
            if app.space_ended(&session) {
                Flow::Continue
            } else {
                Flow::Quit
            }
        }
        Event::Tmux {
            session,
            msg: Message::Notification(n),
        } if changes_structure(&n) => {
            app.refresh_reported(&session);
            Flow::Continue
        }
        Event::Tmux { .. } => return Ok((Flow::Continue, false)),
        Event::Hooks(records) => {
            app.hooks(&records);
            Flow::Continue
        }
        Event::Agents(taken, agents) => {
            app.sessions.reconcile(taken, &agents);
            Flow::Continue
        }
        Event::Git(read) => {
            app.task_status.extend(read);
            Flow::Continue
        }
        Event::Details(read) => {
            app.task_details.extend(read);
            Flow::Continue
        }
        Event::Spaces(read) => {
            app.places.extend(read);
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
            let selected = app.sidebar_selection();
            let side_area = Rect {
                width: side,
                ..full
            };
            let spaces = app.sidebar_spaces(&rows);
            let agents = app.sidebar_agents(&rows);
            let bg = app.current_space_bg;
            sidebar::render(&spaces, &agents, selected, bg, side_area, f.buffer_mut());
        }
        let area = Rect {
            x: full.x + side,
            width: full.width - side,
            ..full
        };
        let bar_area = Rect {
            height: TAB_BAR_ROWS.min(area.height),
            ..area
        };
        let bar_cursor = draw_tab_bar(app, &rows, bar_area, f.buffer_mut());
        let term_cursor = draw_panes(app, f.buffer_mut());
        let dash_cursor = draw_dashboard(app, &rows, f.buffer_mut());
        if let Mode::Help = app.mode {
            help::render(&app.prefix_name, full, f.buffer_mut());
        } else if app.dashboard().is_some() {
            // Only typing into the preview shows a cursor.
            if let Some(cursor) = dash_cursor {
                f.set_cursor_position(cursor);
            }
        } else if let Mode::Sidebar(_) = app.mode {
            // The sidebar has the keyboard: no terminal cursor.
        } else if let Some(cursor) = bar_cursor.or(term_cursor) {
            f.set_cursor_position(cursor);
        }
    })?;
    Ok(())
}

/// The tab bar, or the question before a close. Returns the cursor while
/// renaming.
fn draw_tab_bar(app: &App, rows: &[Row], area: Rect, buf: &mut Buffer) -> Option<Position> {
    let armed = app.prefix.armed().then_some(app.prefix_name.as_str());
    let ask = match &app.mode {
        Mode::ConfirmClose { state, target, .. } => {
            let what = match target {
                CloseTarget::Pane(_) => "close",
                CloseTarget::Space(_) => "close space",
            };
            Some(format!("Claude is {}: {what}? y/n", state.label()))
        }
        _ => None,
    };
    let bar = TabBar {
        // The question takes the whole bar, so it always fits.
        tabs: match ask {
            Some(_) => Vec::new(),
            None => app.tabs(rows),
        },
        current: app.current_space().map_or(0, |i| i.structure.current),
        first: app.tab_first,
        colors: app.tab_colors,
        rename: match &app.mode {
            Mode::Rename { target, text } => {
                let what = match target {
                    Renaming::Tab => "tab",
                    Renaming::Space => "space",
                };
                Some((what, text.as_str()))
            }
            _ => None,
        },
        status: ask.as_deref().or(app.notice.as_deref()).or(armed),
    };
    bar.render(area, buf)
}

/// The current tab's panes and the borders between them. Returns the
/// focused pane's cursor.
fn draw_panes(app: &App, buf: &mut Buffer) -> Option<Position> {
    let focus = app.current_pane();
    let panes = app.pane_rects();
    let mut cursor = None;
    for (pane, rect) in &panes {
        let Some(t) = app.terminals.get(pane) else {
            continue;
        };
        let at = session_view::render(t.emulator(), *rect, buf);
        if Some(pane) == focus.as_ref() {
            cursor = at;
        }
    }
    let rects: Vec<Rect> = panes.iter().map(|(_, r)| *r).collect();
    session_view::borders(app.area(), &rects, buf);
    cursor
}

/// The dashboard over the panes, if open. Returns the previewed pane's
/// cursor while typing into it.
fn draw_dashboard(app: &App, rows: &[Row], buf: &mut Buffer) -> Option<Position> {
    let d = app.dashboard()?;
    let row = app.dashboard_row(d);
    let view = dashboard::View {
        rows,
        details: &app.task_details,
        selected: d.selected.as_ref(),
        preview: row
            .and_then(|r| app.terminals.get(&r.pane))
            .map(|t| t.emulator()),
        interactive: d.interactive,
        now: now_ms() / MS_PER_SEC,
    };
    dashboard::render(&view, app.area(), buf)
}
