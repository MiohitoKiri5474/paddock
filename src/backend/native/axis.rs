//! Which way a row of splits runs, and so which way its borders do: the
//! one switch the layout tree, its borders and the drag turn on.

use crate::input::Direction;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

/// The way cells sit next to each other: side by side, along the width
/// (`Columns`, a row of cells with borders between columns), or one above
/// another, along the height (`Rows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Columns,
    Rows,
}

impl Axis {
    /// `Columns` if `columns`, as the backend's interface says it.
    pub fn of(columns: bool) -> Self {
        if columns {
            Self::Columns
        } else {
            Self::Rows
        }
    }

    /// The axis a border moved in `dir` runs across.
    pub fn of_direction(dir: Direction) -> Self {
        Self::of(matches!(dir, Direction::Left | Direction::Right))
    }

    pub fn is_columns(self) -> bool {
        self == Self::Columns
    }

    /// `r`'s length along this axis.
    pub fn len(self, r: Rect) -> u16 {
        match self {
            Self::Columns => r.width,
            Self::Rows => r.height,
        }
    }

    /// Where `r` starts along this axis.
    pub fn start(self, r: Rect) -> u16 {
        match self {
            Self::Columns => r.x,
            Self::Rows => r.y,
        }
    }

    /// Where `r` ends (exclusive) along this axis.
    pub fn end(self, r: Rect) -> u32 {
        u32::from(self.start(r)) + u32::from(self.len(r))
    }

    /// The coordinate of `(x, y)` along this axis.
    pub fn pick(self, x: u16, y: u16) -> u16 {
        match self {
            Self::Columns => x,
            Self::Rows => y,
        }
    }

    /// The other axis.
    pub fn across(self) -> Self {
        match self {
            Self::Columns => Self::Rows,
            Self::Rows => Self::Columns,
        }
    }

    /// The part of `area` from `at`, `len` long along this axis, its full
    /// length across.
    pub fn cut(self, area: Rect, at: u16, len: u16) -> Rect {
        match self {
            Self::Columns => Rect::new(at, area.y, len, area.height),
            Self::Rows => Rect::new(area.x, at, area.width, len),
        }
    }
}
