//! Copy mode (`C-a [`): the terminal typing goes to (the float shown, else
//! the focused pane) holds its output back so its view stays still, and the
//! keys of `[keys.copy]` move a cursor over its history and screen, select,
//! copy (as dragging does) and search. Leaving gives the live view back,
//! with the output that came meanwhile.

use super::input::Flow;
use super::mouse::copy as to_clipboard;
use super::state::{App, Mode};
use crate::input::keymap::CopyAction;
use crate::input::line::LineEdit;
use crate::input::{Command, KeyAction};
use crate::terminal::emulator::{Emulator, Found, Motion, Spot};
use crate::terminal::search;
use crate::ui::copy::{CopyView, Status};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

/// The copy-mode actions whose keys the status line shows, as far as they
/// fit (the help has no room for them).
const HINTS: &[&str] = &[
    "select",
    "select_lines",
    "copy",
    "search_up",
    "search_down",
    "next_match",
    "leave",
];

/// A terminal in copy mode.
pub struct CopyMode {
    /// Its pane.
    pub pane: String,
    pub cursor: Spot,
    /// Where the selection started, and whether it takes whole lines.
    pub anchor: Option<(Spot, bool)>,
    /// The last search.
    pub search: Option<Search>,
    /// The search being typed, if the prompt is open.
    pub typing: Option<Typing>,
    /// The status line's key hints, from the keys as configured.
    pub hints: Vec<String>,
}

/// A search typed in copy mode.
pub struct Search {
    pub query: String,
    /// `?` searches on towards the bottom, `/` back towards the top.
    pub forward: bool,
    /// Every match, top to bottom.
    pub found: Vec<Found>,
    /// The match the cursor went to last.
    pub current: Option<usize>,
}

/// The search prompt: the text typed and its direction.
pub struct Typing {
    pub line: LineEdit,
    pub forward: bool,
}

impl CopyMode {
    /// What the status line shows: the search being typed, else where the
    /// cursor is and the keys.
    fn status<'a>(&'a self, emu: &Emulator) -> Status<'a> {
        if let Some(t) = &self.typing {
            let key = if t.forward { '?' } else { '/' };
            return Status::Prompt { key, line: &t.line };
        }
        let (n, total) = emu.line_number(self.cursor);
        let mut left = format!(" COPY  line {n}/{total}");
        if let Some(s) = &self.search {
            let i = s.current.map_or(0, |i| i + 1);
            left.push_str(&format!("  match {i}/{}", s.found.len()));
        }
        Status::Info {
            left,
            hints: &self.hints,
        }
    }

    /// The view of copy mode over `emu`, for drawing.
    pub fn view<'a>(&'a self, emu: &Emulator) -> CopyView<'a> {
        CopyView {
            cursor: self.cursor,
            found: self.search.as_ref().map_or(&[], |s| s.found.as_slice()),
            status: self.status(emu),
        }
    }
}

impl App {
    /// Copy mode in the terminal typing goes to, the search prompt open if
    /// `search`; the view holds still from here.
    pub(super) fn enter_copy(&mut self, search: bool) {
        let Some(pane) = self.float_pane().or_else(|| self.current_pane()) else {
            return;
        };
        let Some(t) = self.terminals.get_mut(&pane) else {
            return;
        };
        t.pause();
        let cursor = t.emulator().copy_start();
        self.mode = Mode::Copy(CopyMode {
            pane,
            cursor,
            anchor: None,
            search: None,
            typing: search.then(|| Typing {
                line: LineEdit::default(),
                forward: false,
            }),
            hints: self.keymap.copy.hints_for(HINTS),
        });
    }

    /// Leave copy mode in `copy`'s terminal: the selection goes and the
    /// view is live again, with the output held back drawn. The caller
    /// sets the mode.
    pub(super) fn leave_copy(&mut self, copy: &CopyMode) {
        if let Some(t) = self.terminals.get_mut(&copy.pane) {
            t.go_on();
            let emu = t.emulator_mut();
            emu.clear_selection();
            emu.scroll_to_bottom();
        }
    }

    /// After any event: copy mode ends by itself when its terminal closed,
    /// is no longer on screen (another tab, the float hidden), or went on
    /// because it held too much output.
    pub(super) fn check_copy(&mut self) {
        let Mode::Copy(copy) = &self.mode else {
            return;
        };
        let pane = copy.pane.clone();
        let paused = self.terminals.get(&pane).is_some_and(|t| t.is_paused());
        if paused && self.shows(&pane) {
            return;
        }
        if let Mode::Copy(copy) = std::mem::replace(&mut self.mode, Mode::Normal) {
            self.leave_copy(&copy);
        }
        if self
            .terminals
            .get_mut(&pane)
            .is_some_and(|t| t.take_overflowed())
        {
            self.notice = Some("copy mode left: too much output came meanwhile".into());
        }
    }

    /// A key in copy mode: while typing a search, it edits the search;
    /// else the prefix works (a command leaves copy mode first, except
    /// copy mode itself and search), and `[keys.copy]` keys act. Direct
    /// chords do not fire here.
    pub(super) fn copy_key(&mut self, mut copy: CopyMode, key: KeyEvent) -> Result<Flow> {
        if let Some(typing) = copy.typing.take() {
            self.search_key(copy, typing, key);
            return Ok(Flow::Continue);
        }
        match self.prefix.handle(key) {
            KeyAction::Command(Command::CopyMode) | KeyAction::None => {}
            KeyAction::Command(Command::Search) => {
                copy.typing = Some(Typing {
                    line: LineEdit::default(),
                    forward: false,
                });
            }
            KeyAction::Command(c) => {
                self.leave_copy(&copy);
                return self.command(c);
            }
            // A key not bound in `[keys.copy]` does nothing.
            KeyAction::Forward(key) => {
                if let Some(action) = self.keymap.copy.action(&key) {
                    return Ok(self.copy_action(copy, action));
                }
            }
        }
        self.mode = Mode::Copy(copy);
        Ok(Flow::Continue)
    }

    /// Do `action` in copy mode.
    fn copy_action(&mut self, mut copy: CopyMode, action: CopyAction) -> Flow {
        let Some(t) = self.terminals.get_mut(&copy.pane) else {
            return Flow::Continue;
        };
        let emu = t.emulator_mut();
        let page = i32::from(emu.size().1);
        match action {
            CopyAction::Move(m) => copy.cursor = emu.motion(copy.cursor, m),
            CopyAction::HalfPage(dir) => {
                let lines = -dir * (page / 2).max(1);
                copy.cursor = emu.motion(copy.cursor, Motion::Scroll(lines));
            }
            CopyAction::Page(dir) => {
                copy.cursor = emu.motion(copy.cursor, Motion::Scroll(-dir * page));
            }
            CopyAction::Select => toggle_selection(&mut copy, emu, false),
            CopyAction::SelectLines => toggle_selection(&mut copy, emu, true),
            CopyAction::Copy => {
                let text = emu.selected_text();
                self.leave_copy(&copy);
                if let Some(text) = text {
                    self.report(|_| to_clipboard(&text));
                }
                return Flow::Continue;
            }
            CopyAction::Leave => {
                self.leave_copy(&copy);
                return Flow::Continue;
            }
            CopyAction::Search { forward } => {
                copy.typing = Some(Typing {
                    line: LineEdit::default(),
                    forward,
                });
            }
            CopyAction::NextMatch | CopyAction::PreviousMatch => {
                if let Some(forward) = copy.search.as_ref().map(|s| s.forward) {
                    let same = action == CopyAction::NextMatch;
                    self.jump(&mut copy, forward == same);
                }
            }
        }
        self.follow_cursor(&copy);
        self.mode = Mode::Copy(copy);
        Flow::Continue
    }

    /// A key while typing a search: `Enter` searches (an empty search does
    /// nothing), `Esc` closes the prompt, others edit the text as in the
    /// prompts.
    fn search_key(&mut self, mut copy: CopyMode, mut typing: Typing, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let query = typing.line.text().to_string();
                if !query.is_empty() {
                    let found = self
                        .terminals
                        .get(&copy.pane)
                        .map(|t| t.emulator().find(&query))
                        .unwrap_or_default();
                    copy.search = Some(Search {
                        query,
                        forward: typing.forward,
                        found,
                        current: None,
                    });
                    self.jump(&mut copy, typing.forward);
                    self.follow_cursor(&copy);
                }
            }
            KeyCode::Esc => {}
            _ => {
                typing.line.key(key);
                copy.typing = Some(typing);
            }
        }
        self.mode = Mode::Copy(copy);
    }

    /// Move the cursor to the nearest match `forward` (down) or back (up)
    /// from it, wrapping around; with none, a notice, and it stays.
    fn jump(&mut self, copy: &mut CopyMode, forward: bool) {
        let Some(s) = &mut copy.search else {
            return;
        };
        let starts: Vec<Spot> = s.found.iter().map(|f| f.start).collect();
        let Some(i) = search::next(&starts, &copy.cursor, forward) else {
            self.notice = Some(format!("no match for {:?}", s.query));
            return;
        };
        s.current = Some(i);
        copy.cursor = starts[i];
    }

    /// Show the cursor, and keep the selection running to it.
    fn follow_cursor(&mut self, copy: &CopyMode) {
        let Some(t) = self.terminals.get_mut(&copy.pane) else {
            return;
        };
        let emu = t.emulator_mut();
        emu.reveal(copy.cursor);
        if let Some((anchor, lines)) = copy.anchor {
            emu.select_between(anchor, copy.cursor, lines);
        }
    }

    /// The wheel in copy mode, over its terminal: the view scrolls by
    /// `lines` (positive = back into history), taking the cursor along
    /// where it would leave it.
    pub(super) fn copy_wheel(&mut self, lines: i32) {
        let Mode::Copy(copy) = &mut self.mode else {
            return;
        };
        let Some(t) = self.terminals.get_mut(&copy.pane) else {
            return;
        };
        let emu = t.emulator_mut();
        copy.cursor = emu.wheel(copy.cursor, lines);
        if let Some((anchor, whole)) = copy.anchor {
            emu.select_between(anchor, copy.cursor, whole);
        }
    }

    /// Copy mode's view for pane `pane`, if it is the one in copy mode.
    pub(super) fn copy_view(&self, pane: &str) -> Option<CopyView<'_>> {
        let Mode::Copy(copy) = &self.mode else {
            return None;
        };
        let t = self.terminals.get(pane).filter(|_| copy.pane == pane)?;
        Some(copy.view(t.emulator()))
    }

    /// Paste in copy mode: into the search being typed; else nowhere.
    pub(super) fn copy_paste(&mut self, text: &str) {
        if let Mode::Copy(CopyMode {
            typing: Some(t), ..
        }) = &mut self.mode
        {
            t.line.insert(text);
        }
    }
}

/// `v` (characters) or `V` (`lines`): start selecting at the cursor; the
/// same again stops, the other kind switches to it.
fn toggle_selection(copy: &mut CopyMode, emu: &mut Emulator, lines: bool) {
    copy.anchor = match copy.anchor {
        Some((_, kind)) if kind == lines => None,
        Some((anchor, _)) => Some((anchor, lines)),
        None => Some((copy.cursor, lines)),
    };
    if copy.anchor.is_none() {
        emu.clear_selection();
    }
}
