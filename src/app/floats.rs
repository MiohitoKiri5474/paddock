//! Floats (`[floats]`): a program in a box over the screen, shown and
//! hidden by the prefix and its key. Each space has its own float per
//! key: a tmux window of the space's session marked with
//! [`FLOAT_OPTION`](crate::tmux::structure::FLOAT_OPTION), so it keeps
//! running while hidden and after detaching, but is never a tab. Hiding
//! keeps the program; when it exits, its window and the float close.

use super::state::App;
use crate::config::Float;
use crate::tmux::structure::Window;
use crate::ui::float::{self, Float as FloatView};
use anyhow::Result;
use ratatui::layout::Rect;

/// The float shown: space `space`'s float for `[floats]` entry `index`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The tmux session id (`$N`) of its space.
    pub space: String,
    /// Its place in `[floats]`.
    pub index: usize,
}

impl App {
    /// The float shown now, with its window: only over its own space,
    /// and only while its program runs.
    pub(super) fn shown_float(&self) -> Option<(&Float, &Window)> {
        let shown = self.float.as_ref()?;
        let space = self.current_space().filter(|s| s.id == shown.space)?;
        let float = self.floats.list.get(shown.index)?;
        let window = space.float(&float.key.name())?;
        Some((float, &window.window))
    }

    /// The shown float's pane, which has the keyboard.
    pub fn float_pane(&self) -> Option<String> {
        Some(self.shown_float()?.1.active.clone())
    }

    /// Where a float's box goes: centred over the whole screen.
    pub(super) fn float_area(&self) -> Rect {
        let screen = Rect::new(0, 0, self.screen.0, self.screen.1);
        float::area(screen, (self.floats.width, self.floats.height))
    }

    /// The shown float's pane and where its terminal is drawn.
    pub(super) fn float_rect(&self) -> Option<(String, Rect)> {
        Some((self.float_pane()?, float::inner(self.float_area())))
    }

    /// The shown float, ready to draw.
    pub(super) fn float_view(&self) -> Option<FloatView<'_>> {
        let (float, window) = self.shown_float()?;
        let terminal = self.terminals.get(&window.active)?;
        Some(FloatView {
            title: &float.command,
            accent: self.tab_colors.current,
            emulator: terminal.emulator(),
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
        let shown = Shown {
            space: space.id.clone(),
            index,
        };
        if self.float.as_ref() == Some(&shown) && self.shown_float().is_some() {
            self.float = None;
            return Ok(());
        }
        let key = float.key.name();
        if space.float(&key).is_none() {
            let command = float.command.clone();
            let dir = self.new_dir();
            self.server.new_float(&shown.space, &dir, &command, &key)?;
            self.refresh(&shown.space)?;
        }
        self.float = Some(shown);
        self.resize_float()
    }

    /// Give the shown float's pane the inside of its box; tmux keeps that
    /// size for its window apart from the tabs'.
    pub(super) fn resize_float(&mut self) -> Result<()> {
        let Some(window) = self.shown_float().map(|(_, w)| w.id.clone()) else {
            return Ok(());
        };
        let inside = float::inner(self.float_area());
        match self.current_space_mut() {
            Some(space) => space.resize_float(&window, (inside.width, inside.height)),
            None => Ok(()),
        }
    }

    /// Hide the shown float, if any; its program keeps running.
    pub(super) fn hide_float(&mut self) {
        self.float = None;
    }

    /// Forget the shown float once it has closed (its program exited) or
    /// its space has ended.
    pub(super) fn forget_ended_float(&mut self) {
        let Some(shown) = &self.float else {
            return;
        };
        let runs = self.spaces.iter().any(|s| {
            s.id == shown.space
                && self
                    .floats
                    .list
                    .get(shown.index)
                    .is_some_and(|f| s.float(&f.key.name()).is_some())
        });
        if !runs {
            self.float = None;
        }
    }

    /// Whether pane `pane` is in a float of any space: paddock does not
    /// follow Claude there.
    pub(super) fn is_float_pane(&self, pane: &str) -> bool {
        self.spaces.iter().any(|s| s.holds_float(pane))
    }
}
