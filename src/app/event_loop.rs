//! The main loop: feed terminal output, route input, redraw at most once a frame.

use super::input::Flow;
use super::rows::Row;
use super::screen::Screen;
use super::space::changes_structure;
use super::state::{App, Mode, TAB_BAR_ROWS};
use super::{watch, Event};
use crate::hooks::now_ms;
use crate::tmux::Message;
use crate::ui::{dashboard, help, session_view, sidebar};
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

/// What an event may have changed, for what to do after handling it.
struct EventEffects {
    /// Hook events refresh git at once.
    hooks: bool,
    /// Pane output: the most frequent event, changing nothing but a screen.
    output: bool,
    /// Anything but tmux and the readers' results may change which rows are
    /// on screen.
    rows_may_move: bool,
}

impl EventEffects {
    fn of(event: &Event) -> Self {
        Self {
            hooks: matches!(event, Event::Hooks(_)),
            output: matches!(
                event,
                Event::Tmux {
                    msg: Message::Output { .. },
                    ..
                }
            ),
            rows_may_move: !matches!(event, Event::Tmux { .. } | Event::Git(_) | Event::Spaces(_)),
        }
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
        let effects = EventEffects::of(&event);
        // A terminal can end with its exit still queued: a write to it fails
        // then. Say so, and go on rather than quit without saving.
        let (flow, changed) = handle(&mut app, event, screen).unwrap_or_else(|e| {
            app.notice = Some(format!("{e:#}"));
            (Flow::Continue, true)
        });
        dirty |= changed;
        follow_up(&mut app, &mut git, &effects);
        if flow == Flow::Quit {
            break;
        }
    }
    app.detach_all()
}

/// Keep up with an event just handled (see `effects`): turns seen, the tab bar
/// scrolled to the current tab, the dashboard's selection and git's tasks.
fn follow_up(app: &mut App, git: &mut GitFeed, effects: &EventEffects) {
    // Any event may show a tab or bring the window back.
    app.see_current();
    // Pane output changes the space or tab shown only through a refresh,
    // which notes the page itself.
    if !effects.output {
        app.track_page();
        app.settle_tabs();
    }
    if effects.rows_may_move {
        app.anchor_dashboard();
        git.update(app, effects.hooks);
    }
}

/// Apply one event. Returns whether to go on and whether the screen may have changed.
fn handle(app: &mut App, event: Event, screen: &mut Screen) -> Result<(Flow, bool)> {
    match event {
        Event::Tmux { session, msg } => Ok(handle_tmux(app, &session, msg)),
        Event::Input(input) => handle_input(app, input, screen),
        Event::Detach => Ok((Flow::Quit, true)),
        Event::Hooks(records) => {
            app.hooks(&records);
            Ok((Flow::Continue, true))
        }
        Event::Agents(taken, agents) => {
            app.sessions.reconcile(taken, &agents);
            Ok((Flow::Continue, true))
        }
        Event::Git(read) => {
            app.task_status.extend(read);
            Ok((Flow::Continue, true))
        }
        Event::Details(read) => {
            app.task_details.extend(read);
            Ok((Flow::Continue, true))
        }
        Event::Spaces(read) => {
            app.places.extend(read);
            Ok((Flow::Continue, true))
        }
        Event::NotifyFailed(error) => {
            if let Some(notice) = app.attention.notify_failed(&error) {
                app.notice = Some(notice);
            }
            Ok((Flow::Continue, true))
        }
    }
}

/// A control message from space `session`'s client.
fn handle_tmux(app: &mut App, session: &str, msg: Message) -> (Flow, bool) {
    match msg {
        Message::Output { pane, data } => {
            if !app.terminals.contains_key(&pane) {
                // A pane made since the last look, maybe through `tmux
                // attach`. The refresh may change the tab shown, which
                // `follow_up` skips for output: note the page here.
                app.refresh_reported(session);
                app.track_page();
            }
            if let Some(t) = app.terminals.get_mut(&pane) {
                t.output(&data);
            }
            (
                Flow::Continue,
                app.shows(&pane) || app.shows_in_preview(&pane),
            )
        }
        // The space's tmux session ended: its last pane exited or was closed.
        Message::Exit if app.space_ended(session) => (Flow::Continue, true),
        Message::Exit => (Flow::Quit, true),
        Message::Notification(n) if changes_structure(&n) => {
            app.refresh_reported(session);
            (Flow::Continue, true)
        }
        Message::Notification(_) | Message::Reply { .. } => (Flow::Continue, false),
    }
}

/// Input from the outer terminal.
fn handle_input(app: &mut App, event: ct::Event, screen: &mut Screen) -> Result<(Flow, bool)> {
    let flow = match event {
        ct::Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key)?,
        ct::Event::Paste(text) => {
            app.paste(&text)?;
            Flow::Continue
        }
        ct::Event::Mouse(m) => return Ok((Flow::Continue, app.mouse(m)?)),
        ct::Event::Resize(cols, rows) => {
            app.resize((cols, rows))?;
            screen.autoresize()?;
            Flow::Continue
        }
        ct::Event::FocusGained | ct::Event::FocusLost => {
            app.attention.window_focused = matches!(event, ct::Event::FocusGained);
            return Ok((Flow::Continue, false));
        }
        _ => return Ok((Flow::Continue, false)),
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
        // The frame may already have a new size the app has not seen yet
        // (its resize event is still queued): never draw outside it.
        // When the sizes agree, the panes fit: clipping hides only the lag.
        debug_assert!(
            app.screen != (full.width, full.height) || full.intersection(app.area()) == app.area(),
            "panes outside the frame"
        );
        let panes = app.area().intersection(full);
        let term_cursor = draw_panes(app, panes, f.buffer_mut());
        let dash_cursor = draw_dashboard(app, &rows, panes, f.buffer_mut());
        if let Mode::Help = app.mode {
            help::render(&app.prefix_name, &app.keymap, full, f.buffer_mut());
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
    app.tab_bar_view(rows).render(area, buf)
}

/// The current tab's panes and the borders between them. Returns the
/// focused pane's cursor.
fn draw_panes(app: &App, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let focus = app.current_pane();
    let panes: Vec<(String, Rect)> = app
        .pane_rects()
        .into_iter()
        .map(|(pane, r)| (pane, r.intersection(area)))
        .collect();
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
    session_view::borders(area, &rects, buf);
    cursor
}

/// The dashboard over the panes, if open. Returns the previewed pane's
/// cursor while typing into it.
fn draw_dashboard(app: &App, rows: &[Row], area: Rect, buf: &mut Buffer) -> Option<Position> {
    let d = app.dashboard()?;
    let row = app.dashboard_row(d);
    let keys = app.keymap.dashboard.hint();
    let view = dashboard::View {
        rows,
        keys: &keys,
        details: &app.task_details,
        selected: d.selected.as_ref(),
        preview: row
            .and_then(|r| app.terminals.get(&r.pane))
            .map(|t| t.emulator()),
        interactive: d.interactive,
        now: now_ms() / MS_PER_SEC,
    };
    dashboard::render(&view, area, buf)
}
