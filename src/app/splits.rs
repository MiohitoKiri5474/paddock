//! Splitting a tab into several panes, moving focus between them and
//! zooming one, resizing them by key or by dragging a border, through
//! the backend, which keeps the layout and the zoom (ADR 0006); and keeping each
//! space's size and the saved state in step.

use super::mouse::Press;
use super::saved::{self, Saved};
use super::space::clamp;
use super::state::App;
use crate::backend::Border;
use crate::input::prefix::RESIZE_STEP;
use crate::input::Direction;
use anyhow::Result;
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

impl App {
    /// Split the focused pane: side by side if `columns`, else top and
    /// bottom. A new shell takes the second half and the focus.
    pub fn split(&mut self, columns: bool) -> Result<()> {
        let (Some(pane), Some(id)) = (
            self.current_pane(),
            self.current_space().map(|i| i.id.clone()),
        ) else {
            return Ok(());
        };
        let dir = self.tab_dir();
        self.backend.split(&pane, columns, &dir)?;
        self.refresh(&id)
    }

    /// Focus the pane next to the focused one in `dir`, if there is one.
    pub fn move_focus(&mut self, dir: Direction) -> Result<()> {
        let (Some(pane), Some(id)) = (
            self.current_pane(),
            self.current_space().map(|i| i.id.clone()),
        ) else {
            return Ok(());
        };
        self.backend.focus_towards(&pane, dir)?;
        self.refresh(&id)
    }

    /// Move the focused pane's border in `dir` by [`RESIZE_STEP`]: the
    /// border after it, or before it for the last pane of a row or column.
    /// The backend keeps the layout, and its change event redraws.
    pub fn resize_split(&mut self, dir: Direction) -> Result<()> {
        match self.current_pane() {
            Some(pane) => self.backend.resize(&pane, dir, RESIZE_STEP),
            None => Ok(()),
        }
    }

    /// Give the focused pane and the panes beside it the same size.
    pub fn equalise(&mut self) -> Result<()> {
        match self.current_pane() {
            Some(pane) => self.backend.spread(&pane),
            None => Ok(()),
        }
    }

    /// Make the focused pane fill its tab, or, if it does, give the
    /// others their places back. The backend keeps the zoom; moving focus,
    /// splitting and closing a pane undo it there.
    pub fn zoom(&mut self) -> Result<()> {
        match self.current_pane() {
            Some(pane) => self.backend.zoom(&pane),
            None => Ok(()),
        }
    }

    /// Show every space at the size of the session area, and the float
    /// shown at the inside of its box; the backend resizes the tabs and
    /// reports the new layout.
    pub(super) fn relayout(&mut self) -> Result<()> {
        let area = self.area();
        let size = clamp((area.width, area.height));
        // One space that cannot be resized (it just ended) stops no other.
        let mut result = Ok(());
        for space in &mut self.spaces {
            result = result.and(space.resize(size));
        }
        result.and(self.resize_float())
    }

    /// After a command or click: sizes follow (the sidebar may have come,
    /// gone or changed width), and the state is saved, in case paddock does
    /// not get to detach.
    pub(super) fn relayout_and_save(&mut self) {
        self.report(|app| app.relayout());
        self.report(|app| app.save());
    }

    /// Write the current space to the state directory.
    pub(super) fn save(&self) -> Result<()> {
        let saved = Saved {
            space: self.current_space().map(|i| i.id.clone()),
            sidebar: self.sidebar_wish,
        };
        saved::save(&self.state_dir, &saved)
    }
}

/// Least time between two resizes sent while a split's border is
/// dragged; the last size always goes on release.
const DRAG_INTERVAL: Duration = Duration::from_millis(40);

/// A split's border held by the mouse.
#[derive(Debug)]
pub struct SplitDrag {
    border: Border,
    /// The size last sent to the backend (at first, the one it has), and when.
    size: u16,
    sent: Option<Instant>,
}

impl SplitDrag {
    /// `border`, pressed at screen position `pos` over the session
    /// area `area`.
    fn new(border: Border, area: Rect, pos: (u16, u16)) -> Self {
        let mut drag = Self {
            border,
            size: 0,
            sent: None,
        };
        drag.size = drag.border.size(drag.along(area, pos));
        drag
    }

    /// Where screen position `(col, row)` is on the border's axis, in
    /// the coordinates of the window drawn in `area`.
    fn along(&self, area: Rect, (col, row): (u16, u16)) -> u16 {
        if self.border.columns() {
            col.saturating_sub(area.x)
        } else {
            row.saturating_sub(area.y)
        }
    }

    /// The size to send for the border dragged to screen position `pos`
    /// at `now`: none if it is the size last sent or, while the button is
    /// held, if one went less than [`DRAG_INTERVAL`] ago.
    fn due(&mut self, area: Rect, pos: (u16, u16), now: Instant, released: bool) -> Option<u16> {
        let size = self.border.size(self.along(area, pos));
        let soon = self
            .sent
            .is_some_and(|sent| now.duration_since(sent) < DRAG_INTERVAL);
        if size == self.size || (soon && !released) {
            return None;
        }
        self.size = size;
        self.sent = Some(now);
        Some(size)
    }
}

impl App {
    /// A left press at screen `(col, row)`: if it is on a border between
    /// two of the current tab's splits, hold that border to drag it, and
    /// return true. A zoomed tab shows no borders.
    pub(super) fn press_split_border(&mut self, col: u16, row: u16) -> bool {
        let area = self.area();
        if !area.contains(Position::new(col, row)) {
            return false;
        }
        let tab = match self.current_space().and_then(|s| s.current_tab()) {
            Some(t) if !t.zoomed && t.splits.len() > 1 => t.id.clone(),
            _ => return false,
        };
        let (x, y) = (col - area.x, row - area.y);
        let border = match self.backend.border_at(&tab, x, y) {
            Ok(Some(border)) => border,
            Ok(None) => return false,
            Err(e) => {
                self.notice = Some(format!("{e:#}"));
                return false;
            }
        };
        self.press = Some(Press::Split(SplitDrag::new(border, area, (col, row))));
        true
    }

    /// The button held on a split's border moved or let go: the border
    /// follows the pointer through the backend, at most every
    /// [`DRAG_INTERVAL`], and its last place goes on release. The
    /// backend's change event redraws.
    pub(super) fn split_border_held(&mut self, m: MouseEvent) -> bool {
        let released = matches!(m.kind, MouseEventKind::Up(_));
        let area = self.area();
        let Some(Press::Split(drag)) = &mut self.press else {
            return false;
        };
        let due = drag.due(area, (m.column, m.row), Instant::now(), released);
        let border = drag.border.clone();
        if released {
            self.press = None;
        }
        if let Some(size) = due {
            self.report(|app| app.backend.drag_border(&border, size));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The border of a 34 | 33 split, pressed.
    fn pressed() -> SplitDrag {
        let border = Border::ending("%1", true, 0);
        SplitDrag::new(border, AREA, (AREA.x + 34, 5))
    }

    /// The session area: right of a 32-column sidebar, under the tab bar.
    const AREA: Rect = Rect::new(32, 1, 68, 29);

    /// What `drag` sends for the border dragged to window column `x`.
    fn due(drag: &mut SplitDrag, x: u16, now: Instant, released: bool) -> Option<u16> {
        drag.due(AREA, (AREA.x + x, 7), now, released)
    }

    #[test]
    fn a_drag_sends_at_most_one_size_per_interval_and_the_last_on_release() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut drag = pressed();
        assert_eq!(due(&mut drag, 36, t0, false), Some(36));
        assert_eq!(due(&mut drag, 37, t0 + ms(10), false), None, "too soon");
        assert_eq!(due(&mut drag, 38, t0 + ms(39), false), None, "too soon");
        assert_eq!(due(&mut drag, 39, t0 + ms(40), false), Some(39));
        assert_eq!(due(&mut drag, 39, t0 + ms(200), false), None, "no change");
        assert_eq!(due(&mut drag, 41, t0 + ms(210), true), Some(41), "release");
    }

    #[test]
    fn a_click_or_a_release_where_the_last_size_went_sends_nothing() {
        let t0 = Instant::now();
        assert_eq!(due(&mut pressed(), 34, t0, true), None, "a click");
        let mut drag = pressed();
        assert_eq!(due(&mut drag, 30, t0, false), Some(30));
        assert_eq!(due(&mut drag, 30, t0, true), None);
        // Back where it was pressed: the backend has another size by now.
        let mut back = pressed();
        assert_eq!(due(&mut back, 30, t0, false), Some(30));
        assert_eq!(due(&mut back, 34, t0, true), Some(34));
    }
}
