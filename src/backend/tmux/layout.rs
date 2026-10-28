//! A window's layout tree as tmux 3.8 writes it (`#{window_layout}`, JSON),
//! and which pane to resize, and how, to move the [`Border`] at a given cell.
//!
//! tmux keeps a tab's panes in a tree of rows and columns. `resize-pane`
//! on a pane moves a border of the cell under its nearest row (or column)
//! ancestor: the cell's far border, or its near one when the cell is the
//! last of that row. So the pane beside a border is not always the one
//! that moves it; the tree says which one does.

use crate::backend::border::{Anchor, Border};
use crate::backend::TerminalId;
use serde::Deserialize;

/// What a cell of the layout is (`t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Kind {
    #[serde(rename = "p")]
    Pane,
    /// Cells side by side (tmux's `LAYOUT_LEFTRIGHT`).
    #[serde(rename = "h")]
    Row,
    /// Cells one above another (tmux's `LAYOUT_TOPBOTTOM`).
    #[serde(rename = "v")]
    Column,
}

/// A cell of the layout: a pane, or a row or column of cells.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Cell {
    #[serde(rename = "t")]
    kind: Kind,
    x: u16,
    y: u16,
    #[serde(rename = "w")]
    width: u16,
    #[serde(rename = "h")]
    height: u16,
    /// A pane's id (`%N`).
    #[serde(rename = "I", default)]
    pane: Option<String>,
    #[serde(rename = "c", default)]
    cells: Vec<Cell>,
}

#[derive(Deserialize)]
struct Layout {
    #[serde(rename = "L")]
    root: Cell,
}

/// Parse `#{window_layout}`; `None` if it is not tmux 3.8's JSON.
pub fn parse(text: &str) -> Option<Cell> {
    serde_json::from_str::<Layout>(text).ok().map(|l| l.root)
}

impl Cell {
    fn contains(&self, x: u16, y: u16) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }

    /// The border at `(x, y)` in window coordinates, if that cell is one,
    /// and how to move it.
    pub fn border_at(&self, x: u16, y: u16) -> Option<Border> {
        if self.kind == Kind::Pane || !self.contains(x, y) {
            return None;
        }
        if let Some(child) = self.cells.iter().find(|c| c.contains(x, y)) {
            return child.border_at(x, y);
        }
        let columns = self.kind == Kind::Row;
        let i = self.cells.windows(2).position(|pair| {
            let before = &pair[0];
            if columns {
                x == before.x + before.width
            } else {
                y == before.y + before.height
            }
        })?;
        self.mover(i, columns)
    }

    /// How to move the border after `self.cells[i]`: `resize-pane` on a
    /// pane whose nearest row (or column) ancestor is `self` resizes the
    /// cell of `self` holding it, so a pane in the cell before the border
    /// moves it, or, failing that, one in the cell after it when that is
    /// the last (tmux moves the last cell's near border).
    fn mover(&self, i: usize, columns: bool) -> Option<Border> {
        let (before, after) = (&self.cells[i], &self.cells[i + 1]);
        let start = |c: &Cell| if columns { c.x } else { c.y };
        let len = |c: &Cell| if columns { c.width } else { c.height };
        let border = |pane: String, anchor| Border::new(TerminalId::new(pane), columns, anchor);
        if let Some(pane) = before.reached(self.kind) {
            return Some(border(pane, Anchor::Before(start(before))));
        }
        let last = i + 2 == self.cells.len();
        let pane = after.reached(self.kind).filter(|_| last)?;
        Some(border(pane, Anchor::After(start(after) + len(after))))
    }

    /// A pane in this cell whose nearest ancestor of `kind` is this
    /// cell's parent: this pane, or one reached through cells of the
    /// other kind only.
    fn reached(&self, kind: Kind) -> Option<String> {
        if self.kind == Kind::Pane {
            return self.pane.clone();
        }
        if self.kind == kind {
            return None;
        }
        self.cells.iter().find_map(|c| c.reached(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row `{A | B}` above `C`, all beside `D`, in a 69x29 window, as
    /// tmux 3.8 writes it: the border between B and D is moved by C, as
    /// B is the last of its own row.
    const NESTED: &str = r#"{"V":2,"L":{"t":"h","w":69,"h":29,"x":0,"y":0,"c":[{"t":"v","w":34,"h":29,"x":0,"y":0,"c":[{"t":"h","w":34,"h":14,"x":0,"y":0,"c":[{"t":"p","w":17,"h":14,"x":0,"y":0,"l":2,"i":0,"I":"%0"},{"t":"p","w":16,"h":14,"x":18,"y":0,"a":true,"i":1,"I":"%4"}]},{"t":"p","w":34,"h":14,"x":0,"y":15,"l":0,"i":2,"I":"%3"}]},{"t":"p","w":34,"h":29,"x":35,"y":0,"l":1,"i":3,"I":"%2"}]}}"#;

    fn nested() -> Cell {
        parse(NESTED).expect("tmux 3.8 layout")
    }

    #[test]
    fn a_single_pane_or_a_cell_inside_a_pane_is_no_border() {
        let one = parse(r#"{"V":2,"L":{"t":"p","w":80,"h":24,"x":0,"y":0,"I":"%1"}}"#).unwrap();
        assert_eq!(one.border_at(3, 3), None);
        assert_eq!(nested().border_at(5, 5), None);
        assert_eq!(nested().border_at(50, 5), None);
        assert_eq!(nested().border_at(200, 5), None);
    }

    #[test]
    fn the_border_between_two_panes_is_moved_by_the_first() {
        let b = nested().border_at(17, 3).expect("A | B");
        assert_eq!((b.terminal.as_str(), b.columns()), ("%0", true));
        assert_eq!(b.size(20), 20);
        // A reaches the column through its row, so it resizes that row.
        let b = nested().border_at(10, 14).expect("{A B} over C");
        assert_eq!((b.terminal.as_str(), b.columns()), ("%0", false));
        assert_eq!(b.size(10), 10);
        assert_eq!(b.size(0), 1, "never below one row");
    }

    #[test]
    fn a_border_beside_a_nested_row_is_moved_through_a_pane_that_reaches_it() {
        // B (%4) is beside it, but is the last of the top row: resizing
        // it would move A | B. C reaches the outer row through the column.
        for y in [3, 20] {
            let b = nested().border_at(34, y).expect("left column | D");
            assert_eq!((b.terminal.as_str(), b.columns()), ("%3", true));
            assert_eq!(b.size(40), 40);
            assert_eq!(b.size(0), 1);
        }
    }

    #[test]
    fn the_last_cell_moves_the_border_when_the_first_reaches_no_pane() {
        // `{ [ {A|B} / {C|E} ] | D }`: no pane reaches the outer row from
        // the left column, so D, the last, moves the border.
        let text = r#"{"V":2,"L":{"t":"h","w":69,"h":29,"x":0,"y":0,"c":[
            {"t":"v","w":34,"h":29,"x":0,"y":0,"c":[
                {"t":"h","w":34,"h":14,"x":0,"y":0,"c":[
                    {"t":"p","w":17,"h":14,"x":0,"y":0,"I":"%0"},
                    {"t":"p","w":16,"h":14,"x":18,"y":0,"I":"%1"}]},
                {"t":"h","w":34,"h":14,"x":0,"y":15,"c":[
                    {"t":"p","w":17,"h":14,"x":0,"y":15,"I":"%2"},
                    {"t":"p","w":16,"h":14,"x":18,"y":15,"I":"%3"}]}]},
            {"t":"p","w":34,"h":29,"x":35,"y":0,"I":"%4"}]}}"#;
        let b = parse(text).unwrap().border_at(34, 5).expect("border");
        assert_eq!((b.terminal.as_str(), b.columns()), ("%4", true));
        // D ends at column 69: a border at 40 leaves it 28 wide.
        assert_eq!(b.size(40), 28);
    }

    #[test]
    fn a_border_no_pane_can_move_is_none() {
        // Three columns, the middle one holding only rows: neither the
        // middle cell nor the last-but-not-last cell after it moves the
        // border between them.
        let text = r#"{"V":2,"L":{"t":"h","w":50,"h":20,"x":0,"y":0,"c":[
            {"t":"v","w":20,"h":20,"x":0,"y":0,"c":[
                {"t":"h","w":20,"h":10,"x":0,"y":0,"c":[
                    {"t":"p","w":10,"h":10,"x":0,"y":0,"I":"%0"},
                    {"t":"p","w":9,"h":10,"x":11,"y":0,"I":"%1"}]},
                {"t":"h","w":20,"h":9,"x":0,"y":11,"c":[
                    {"t":"p","w":10,"h":9,"x":0,"y":11,"I":"%2"},
                    {"t":"p","w":9,"h":9,"x":11,"y":11,"I":"%3"}]}]},
            {"t":"v","w":14,"h":20,"x":21,"y":0,"c":[
                {"t":"h","w":14,"h":20,"x":21,"y":0,"c":[
                    {"t":"p","w":7,"h":20,"x":21,"y":0,"I":"%4"},
                    {"t":"p","w":6,"h":20,"x":29,"y":0,"I":"%5"}]}]},
            {"t":"p","w":14,"h":20,"x":36,"y":0,"I":"%6"}]}}"#;
        assert_eq!(parse(text).unwrap().border_at(20, 5), None);
    }

    #[test]
    fn other_formats_do_not_parse() {
        assert_eq!(parse("a87d,100x30,0,0,0"), None);
        assert_eq!(parse(""), None);
        let unknown = r#"{"V":2,"L":{"t":"q","w":8,"h":2,"x":0,"y":0,"I":"%1"}}"#;
        assert_eq!(parse(unknown), None);
    }
}
