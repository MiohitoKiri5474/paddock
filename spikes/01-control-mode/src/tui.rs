//! Interactive harness: draw one tmux pane through our emulator with ratatui.
//! Prefix C-a: `C-a d` detaches (pane keeps running), `C-a C-a` sends C-a.

use crate::emu::{self, Col, MouseMode};
use crate::keys::{encode_key, encode_mouse_sgr, encode_paste};
use crate::pane::{fetch_seed, wait_reply, Pane};
use crate::tmux::{self, Control, Event};
use anyhow::Result;
use crossterm::event::{
    self as cev, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;
use std::io::{stdout, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(16);
const WHEEL_LINES: i32 = 3;

enum AppEvent {
    Tmux(Event),
    Term(cev::Event),
}

fn unwrap_tmux(e: AppEvent) -> Option<Event> {
    match e {
        AppEvent::Tmux(t) => Some(t),
        AppEvent::Term(_) => None,
    }
}

fn color(c: Col) -> Color {
    match c {
        Col::Default => Color::Reset,
        Col::Idx(i) => Color::Indexed(i),
        Col::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn run(emu_name: &str, session: &str, cmd: Option<&str>) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    let pane_rows = rows.saturating_sub(1).max(1);
    if !tmux::has_session(session) {
        tmux::new_session(session, &std::env::current_dir()?, cols, pane_rows, cmd)?;
    }
    let pane_id = tmux::tmux(&["display", "-p", "-t", session, "#{pane_id}"])?.trim().to_string();

    let (tx, rx) = mpsc::channel();
    let mut ctl = Control::attach(session, tx.clone(), AppEvent::Tmux)?;
    ctl.send(&format!("refresh-client -C {cols}x{pane_rows}"))?;
    wait_reply(&rx, unwrap_tmux, &pane_id)?;
    let mut pane = Pane::new(&pane_id, emu::make(emu_name, cols, pane_rows, 50_000));
    let seed = fetch_seed(&mut ctl, &rx, unwrap_tmux, &pane_id)?;
    pane.apply_seed(&seed)?;

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    // Create the terminal (it queries the cursor) before the input thread owns stdin.
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    term.clear()?;
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = cev::read() {
            if input_tx.send(AppEvent::Term(ev)).is_err() {
                break;
            }
        }
    });

    let res = event_loop(&mut term, &mut ctl, &rx, &mut pane, pane_rows);
    execute!(
        out,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;
    out.flush()?;
    ctl.detach();
    res
}

fn event_loop(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ctl: &mut Control,
    rx: &mpsc::Receiver<AppEvent>,
    pane: &mut Pane,
    mut pane_rows: u16,
) -> Result<()> {
    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    let mut prefix = false;
    let mut bytes_in: u64 = 0;
    loop {
        let ev = if dirty {
            let wait = FRAME.saturating_sub(last_draw.elapsed());
            match rx.recv_timeout(wait) {
                Ok(e) => Some(e),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            Some(rx.recv()?)
        };
        if let Some(ev) = ev {
            match ev {
                AppEvent::Tmux(Event::Output { pane: p, data }) if p == pane.id => {
                    bytes_in += data.len() as u64;
                    pane.feed(&data);
                    dirty = true;
                }
                AppEvent::Tmux(Event::Exit) => return Ok(()),
                AppEvent::Tmux(_) => {}
                AppEvent::Term(cev::Event::Resize(c, r)) => {
                    pane_rows = r.saturating_sub(1).max(1);
                    pane.emu.resize(c, pane_rows);
                    ctl.send(&format!("refresh-client -C {c}x{pane_rows}"))?;
                    term.autoresize()?;
                    dirty = true;
                }
                AppEvent::Term(cev::Event::Key(k)) => {
                    if k.kind == KeyEventKind::Release {
                        continue;
                    }
                    let is_ca = k.code == KeyCode::Char('a') && k.modifiers == KeyModifiers::CONTROL;
                    if prefix {
                        prefix = false;
                        if k.code == KeyCode::Char('d') {
                            return Ok(());
                        }
                        if !is_ca {
                            continue;
                        }
                    } else if is_ca {
                        prefix = true;
                        continue;
                    }
                    if let Some(b) = encode_key(&k, pane.key_modes()) {
                        if pane.emu.scroll_offset() > 0 {
                            pane.emu.scroll(i32::MIN / 2);
                            dirty = true;
                        }
                        ctl.send_bytes(&pane.id, &b)?;
                    }
                }
                AppEvent::Term(cev::Event::Paste(text)) => {
                    let b = encode_paste(&text, pane.emu.modes().bracketed_paste);
                    ctl.send_bytes(&pane.id, &b)?;
                }
                AppEvent::Term(cev::Event::Mouse(m)) => {
                    let modes = pane.emu.modes();
                    let wants = match m.kind {
                        MouseEventKind::Moved => modes.mouse == MouseMode::Any,
                        MouseEventKind::Drag(_) => matches!(modes.mouse, MouseMode::Drag | MouseMode::Any),
                        _ => modes.mouse != MouseMode::Off,
                    };
                    let shift = m.modifiers.contains(KeyModifiers::SHIFT);
                    if wants && !shift && m.row < pane_rows {
                        if let Some(b) = encode_mouse_sgr(m.kind, m.column, m.row, m.modifiers) {
                            ctl.send_bytes(&pane.id, &b)?;
                        }
                    } else if matches!(m.kind, MouseEventKind::ScrollUp) {
                        pane.emu.scroll(WHEEL_LINES);
                        dirty = true;
                    } else if matches!(m.kind, MouseEventKind::ScrollDown) {
                        pane.emu.scroll(-WHEEL_LINES);
                        dirty = true;
                    }
                }
                AppEvent::Term(_) => {}
            }
        }
        if dirty && last_draw.elapsed() >= FRAME {
            draw(term, pane, pane_rows, bytes_in)?;
            last_draw = Instant::now();
            dirty = false;
        }
    }
}

fn draw(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    pane: &Pane,
    pane_rows: u16,
    bytes_in: u64,
) -> Result<()> {
    let emu = &pane.emu;
    let (cols, rows) = emu.size();
    let modes = emu.modes();
    let km = pane.key_modes();
    term.draw(|f| {
        let area = f.area();
        let buf = f.buffer_mut();
        for y in 0..rows.min(pane_rows).min(area.height) {
            for x in 0..cols.min(area.width) {
                let c = emu.cell(y, x);
                if c.spacer {
                    continue;
                }
                let mut st = Style::default().fg(color(c.fg)).bg(color(c.bg));
                let mut m = Modifier::empty();
                m.set(Modifier::BOLD, c.bold);
                m.set(Modifier::ITALIC, c.italic);
                m.set(Modifier::UNDERLINED, c.underline);
                m.set(Modifier::REVERSED, c.inverse);
                st = st.add_modifier(m);
                let cell = &mut buf[(x, y)];
                cell.set_symbol(if c.text.is_empty() { " " } else { &c.text });
                cell.set_style(st);
            }
        }
        let status = format!(
            " {} | kitty={} mok={} paste={} mouse={:?} alt={} | scroll {} | {} KiB in | C-a d detach ",
            emu.name(),
            km.kitty_flags,
            km.modify_other_keys,
            modes.bracketed_paste as u8,
            modes.mouse,
            modes.alt_screen as u8,
            emu.scroll_offset(),
            bytes_in / 1024,
        );
        if area.height > pane_rows {
            buf.set_string(0, pane_rows, &status, Style::default().add_modifier(Modifier::REVERSED));
        }
        if modes.show_cursor && emu.scroll_offset() == 0 {
            let (cy, cx) = emu.cursor();
            f.set_cursor_position(Position::new(cx, cy));
        }
    })?;
    Ok(())
}
