//! Floats (`[floats]`): a program in a box over the screen, shown and
//! hidden by the prefix and its key. Each space has its own float per
//! key, kept by the backend apart from the tabs, so it keeps running
//! while hidden and after detaching, but is never a tab.
//! Hiding keeps the program; when it exits, the float closes.

use super::state::App;
use crate::backend::{self, SpaceId, Structure, TerminalId};
use crate::config::Float;
use crate::ui::float::{self, Float as FloatView};
use anyhow::Result;
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

/// A float whose program ends this soon after paddock started it, before
/// any input, exited at once: paddock says so.
const QUICK_EXIT: Duration = Duration::from_secs(2);

/// The float shown: space `space`'s float for `[floats]` entry `index`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// Its space.
    pub space: SpaceId,
    /// Its place in `[floats]`.
    pub index: usize,
    /// When paddock started its program, if this showing did and no
    /// input (keys, paste or mouse) has reached it since.
    pub started: Option<Instant>,
}

impl Shown {
    /// Whether this is space `space`'s float for entry `index`.
    fn is(&self, space: &SpaceId, index: usize) -> bool {
        self.space == *space && self.index == index
    }
}

impl App {
    /// The float shown now, as configured and as the backend runs it:
    /// only over its own space, and only while its program runs.
    pub(super) fn shown_float(&self) -> Option<(&Float, &backend::Float)> {
        let shown = self.float.as_ref()?;
        let space = self.current_space().filter(|s| s.id == shown.space)?;
        let float = self.floats.list.get(shown.index)?;
        Some((float, space.float(&float.key.name())?))
    }

    /// The shown float's terminal, which has the keyboard.
    pub fn float_pane(&self) -> Option<TerminalId> {
        Some(self.shown_float()?.1.active.clone())
    }

    /// Where a float's box goes: centred over the whole screen.
    pub(super) fn float_area(&self) -> Rect {
        let screen = Rect::new(0, 0, self.screen.0, self.screen.1);
        float::area(screen, (self.floats.width, self.floats.height))
    }

    /// The shown float's pane and where its terminal is drawn.
    pub(super) fn float_rect(&self) -> Option<(TerminalId, Rect)> {
        Some((self.float_pane()?, float::inner(self.float_area())))
    }

    /// The shown float, ready to draw.
    pub(super) fn float_view(&self) -> Option<FloatView<'_>> {
        let (float, running) = self.shown_float()?;
        let terminal = self.terminals.get(&running.active)?;
        Some(FloatView {
            title: &float.command,
            accent: self.tab_colors.current,
            emulator: terminal.emulator(),
            copy: self.copy_view(&running.active),
        })
    }

    /// The prefix and the key of `[floats]` entry `index`: hide that float
    /// if it is shown, else show the current space's, starting it in the
    /// focused terminal's directory if it does not run. Any other float
    /// shown is hidden.
    pub(super) fn toggle_float(&mut self, index: usize) -> Result<()> {
        let Some(space) = self.current_space() else {
            return Ok(());
        };
        let Some(float) = self.floats.list.get(index) else {
            return Ok(());
        };
        let mut shown = Shown {
            space: space.id.clone(),
            index,
            started: None,
        };
        let is_shown = self
            .float
            .as_ref()
            .is_some_and(|f| f.is(&shown.space, index));
        if is_shown && self.shown_float().is_some() {
            self.float = None;
            return Ok(());
        }
        let key = float.key.name();
        if space.float(&key).is_none() {
            let command = float.command.clone();
            let dir = self.new_dir();
            self.backend.new_float(&shown.space, &dir, &command, &key)?;
            shown.started = Some(Instant::now());
            let id = shown.space.clone();
            self.float = Some(shown);
            self.refresh(&id)?;
        } else {
            self.float = Some(shown);
        }
        // Its program may have exited already.
        self.forget_ended_float_noting_quick_exit();
        self.resize_float()
    }

    /// Give the shown float's terminal the inside of its box; the backend
    /// keeps that size for it apart from the tabs'.
    pub(super) fn resize_float(&mut self) -> Result<()> {
        let Some(id) = self.shown_float().map(|(_, f)| f.id.clone()) else {
            return Ok(());
        };
        let inside = float::inner(self.float_area());
        match self.current_space_mut() {
            Some(space) => space.resize_float(&id, (inside.width, inside.height)),
            None => Ok(()),
        }
    }

    /// Hide the shown float, if any; its program keeps running.
    pub(super) fn hide_float(&mut self) {
        self.float = None;
    }

    /// Forget the shown float once it has closed (its program exited) or
    /// its space has ended, and post a notice if its program ended within
    /// [`QUICK_EXIT`] of paddock starting it, before any input.
    pub(super) fn forget_ended_float_noting_quick_exit(&mut self) {
        let Some(shown) = &self.float else {
            return;
        };
        let float = self.floats.list.get(shown.index);
        let runs = self.spaces.iter().any(|s| {
            s.id == shown.space && float.is_some_and(|f| s.float(&f.key.name()).is_some())
        });
        if runs {
            return;
        }
        let quick = shown.started.is_some_and(|t| t.elapsed() < QUICK_EXIT);
        if let (true, Some(f)) = (quick, float) {
            self.notice = Some(format!("{} exited at once", f.command));
        }
        self.float = None;
    }

    /// Input is about to reach pane `pane`: if that is the shown float's,
    /// its program ran, so it quitting soon after gives no notice.
    pub(super) fn note_float_input(&mut self, pane: &TerminalId) {
        if self.float_pane().is_some_and(|p| p == *pane) {
            if let Some(shown) = &mut self.float {
                shown.started = None;
            }
        }
    }

    /// Whether pane `pane` is or was in a float of any space: paddock
    /// does not follow an agent there, not even its last hooks after the
    /// float closed.
    pub(super) fn is_float_pane(&self, pane: &str) -> bool {
        self.float_panes.contains(pane)
    }

    /// Remember the panes of the floats in `structure`, for
    /// [`is_float_pane`](Self::is_float_pane).
    // ponytail: never pruned, so it grows by one id per float started
    // while paddock runs. Harmless unless the backend restarts under a
    // running paddock and reuses a terminal id for a tab's.
    pub(super) fn remember_float_panes(&mut self, structure: &Structure) {
        let splits = structure.floats.iter().flat_map(|f| &f.splits);
        self.float_panes.extend(splits.map(|s| s.id.clone()));
    }

    /// At attach: kill the floats in `structure` whose key is no longer
    /// in `[floats]` (nothing could show or close them), leaving them out
    /// of it.
    pub(super) fn close_orphan_floats(&mut self, structure: &mut Structure) {
        let keys: Vec<String> = self.floats.list.iter().map(|f| f.key.name()).collect();
        let (kept, orphans): (Vec<_>, Vec<_>) = std::mem::take(&mut structure.floats)
            .into_iter()
            .partition(|f| keys.contains(&f.key));
        structure.floats = kept;
        for orphan in orphans {
            self.report(|app| app.backend.close_float(&orphan.id));
        }
    }
}
