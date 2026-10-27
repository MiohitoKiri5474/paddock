//! A tab's layout on the native server: a tree of splits, each a row of
//! cells side by side (`columns`) or a column of cells one above another,
//! with one cell between neighbours for the border, and each cell's share
//! of its parent (its weight). Laid out at a size, the tree gives every
//! terminal its place; resizing the tab keeps the shares.
//!
//! It does what tmux does with its own tree (ADR 0006), down to the cell:
//! a split takes the second half of its terminal (`(size - 1) / 2`), into
//! the same row when splitting the way the row runs; a closed cell's
//! place goes to the cell before it (after it, for the first); `=`
//! evens out the nearest row or column not even yet, the first cells
//! taking what is left over; and the resize keys move the border after
//! the focused cell (before it, for the last). A drag sizes the cell
//! before the border.
//!
//! Every change works on cell sizes: the tree's weights are first made
//! the sizes it has at that size ([`Layout::settle`]), so whatever was
//! left over when it was last laid out at another size stays where it was
//! drawn.

use crate::backend::TerminalId;
use crate::input::Direction;
use anyhow::{bail, Result};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

/// One node of the tree: a terminal, or a row or column of cells.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Leaf(TerminalId),
    Split { columns: bool, cells: Vec<Cell> },
}

/// A node in a row or column, with its weight: its share of the
/// row's width (or column's height), borders left out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    weight: u32,
    node: Node,
}

/// A border between two cells, as [`Layout::border_at`] finds it: moved
/// by sizing the cell that ends in `terminal` (its last terminal), which
/// starts at `start` on the border's axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    pub terminal: TerminalId,
    pub columns: bool,
    pub start: u16,
}

/// A tab's tree of splits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: Node,
}

impl Node {
    /// The smallest this node gets on the `columns` axis (width if true).
    fn min(&self, columns: bool) -> u16 {
        match self {
            Node::Leaf(_) => 1,
            Node::Split { columns: c, cells } => {
                let mins = cells.iter().map(|cell| cell.node.min(columns));
                if *c == columns {
                    let borders = u16::try_from(cells.len().saturating_sub(1)).unwrap_or(u16::MAX);
                    mins.fold(borders, u16::saturating_add)
                } else {
                    mins.max().unwrap_or(1)
                }
            }
        }
    }

    fn place(&self, area: Rect, out: &mut Vec<(TerminalId, Rect)>) {
        match self {
            Node::Leaf(t) => out.push((t.clone(), area)),
            Node::Split { columns, cells } => {
                for (cell, rect) in cells.iter().zip(cell_rects(cells, *columns, area)) {
                    cell.node.place(rect, out);
                }
            }
        }
    }

    /// Make every weight below this node, laid out in `area`, the size
    /// its cell has there.
    fn settle(&mut self, area: Rect) {
        if let Node::Split { columns, cells } = self {
            let rects = cell_rects(cells, *columns, area);
            for (cell, rect) in cells.iter_mut().zip(rects) {
                cell.weight = u32::from(along(rect, *columns));
                cell.node.settle(rect);
            }
        }
    }

    fn contains(&self, t: &TerminalId) -> bool {
        match self {
            Node::Leaf(id) => id == t,
            Node::Split { cells, .. } => cells.iter().any(|c| c.node.contains(t)),
        }
    }

    fn last_leaf(&self) -> &TerminalId {
        match self {
            Node::Leaf(t) => t,
            Node::Split { cells, .. } => cells.last().map_or_else(
                || unreachable!("a split always has cells"),
                |c| c.node.last_leaf(),
            ),
        }
    }

    /// The path (cell indices) from this node to `t`'s leaf.
    fn path(&self, t: &TerminalId) -> Option<Vec<usize>> {
        match self {
            Node::Leaf(id) => (id == t).then(Vec::new),
            Node::Split { cells, .. } => cells.iter().enumerate().find_map(|(i, c)| {
                let mut rest = c.node.path(t)?;
                rest.insert(0, i);
                Some(rest)
            }),
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> &mut Node {
        match (path.split_first(), self) {
            (None, node) => node,
            (Some((&i, rest)), Node::Split { cells, .. }) => cells[i].node.at_mut(rest),
            (Some(_), Node::Leaf(_)) => unreachable!("a path goes through splits"),
        }
    }

    fn at(&self, path: &[usize]) -> &Node {
        match (path.split_first(), self) {
            (None, node) => node,
            (Some((&i, rest)), Node::Split { cells, .. }) => cells[i].node.at(rest),
            (Some(_), Node::Leaf(_)) => unreachable!("a path goes through splits"),
        }
    }

    /// The area of the node at `path`, when this one is in `area`.
    fn area_of(&self, path: &[usize], area: Rect) -> Rect {
        match (path.split_first(), self) {
            (Some((&i, rest)), Node::Split { columns, cells }) => {
                let rect = cell_rects(cells, *columns, area)[i];
                cells[i].node.area_of(rest, rect)
            }
            _ => area,
        }
    }

    fn border_at(&self, area: Rect, x: u16, y: u16) -> Option<Found> {
        let Node::Split { columns, cells } = self else {
            return None;
        };
        let rects = cell_rects(cells, *columns, area);
        if let Some(i) = rects.iter().position(|r| contains(*r, x, y)) {
            return cells[i].node.border_at(rects[i], x, y);
        }
        let across = if *columns { y } else { x };
        let (from, len) = if *columns {
            (area.y, area.height)
        } else {
            (area.x, area.width)
        };
        if across < from || across >= from.saturating_add(len) {
            return None;
        }
        let at = if *columns { x } else { y };
        let i = rects.iter().position(|r| end(*r, *columns) == at)?;
        (i + 1 < cells.len()).then(|| Found {
            terminal: cells[i].node.last_leaf().clone(),
            columns: *columns,
            start: start(rects[i], *columns),
        })
    }
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x.saturating_add(r.width) && y >= r.y && y < r.y.saturating_add(r.height)
}

fn along(r: Rect, columns: bool) -> u16 {
    if columns {
        r.width
    } else {
        r.height
    }
}

fn start(r: Rect, columns: bool) -> u16 {
    if columns {
        r.x
    } else {
        r.y
    }
}

fn end(r: Rect, columns: bool) -> u16 {
    start(r, columns).saturating_add(along(r, columns))
}

/// Where each cell of a row (`columns`) or column sits in `area`.
fn cell_rects(cells: &[Cell], columns: bool, area: Rect) -> Vec<Rect> {
    let weights: Vec<u32> = cells.iter().map(|c| c.weight).collect();
    let mins: Vec<u16> = cells.iter().map(|c| c.node.min(columns)).collect();
    let sizes = distribute(&weights, &mins, along(area, columns));
    let mut at = start(area, columns);
    sizes
        .into_iter()
        .map(|size| {
            let rect = if columns {
                Rect::new(at, area.y, size, area.height)
            } else {
                Rect::new(area.x, at, area.width, size)
            };
            at = at.saturating_add(size).saturating_add(1);
            rect
        })
        .collect()
}

/// Share `total` out between cells by `weights`, a border between each
/// two, each at least its `min` while there is room: the sizes add up to
/// `total` less the borders. Weights that are the sizes a row had at
/// `total` give those sizes back.
fn distribute(weights: &[u32], mins: &[u16], total: u16) -> Vec<u16> {
    let n = weights.len();
    let borders = u16::try_from(n.saturating_sub(1)).unwrap_or(u16::MAX);
    let room = u64::from(total.saturating_sub(borders));
    let sum: u64 = weights.iter().map(|&w| u64::from(w)).sum();
    let mut sizes = Vec::with_capacity(n);
    let (mut cum, mut placed) = (0u64, 0u64);
    for (i, &w) in weights.iter().enumerate() {
        cum += if sum == 0 { 1 } else { u64::from(w) };
        let whole = if sum == 0 { n as u64 } else { sum };
        let edge = (cum * room + whole / 2) / whole;
        let edge = if i + 1 == n { room } else { edge };
        sizes.push(u16::try_from(edge - placed).unwrap_or(u16::MAX));
        placed = edge;
    }
    // Cells below their smallest take from the biggest above theirs.
    for i in 0..n {
        while sizes[i] < mins[i] {
            let donor = (0..n)
                .filter(|&j| j != i && sizes[j] > mins[j])
                .max_by_key(|&j| sizes[j] - mins[j]);
            let Some(j) = donor else { break };
            sizes[j] -= 1;
            sizes[i] += 1;
        }
    }
    sizes
}

/// Even sizes for `n` cells in `total`, as tmux's `select-layout -E`
/// gives them: the first cells take one more until what is left over is
/// gone. `None` if there is no room for one cell each.
fn even(n: usize, total: u16) -> Option<Vec<u16>> {
    let n16 = u16::try_from(n).ok()?;
    let room = total.checked_sub(n16 - 1)?;
    let each = room / n16;
    if each == 0 {
        return None;
    }
    let extra = room % n16;
    Some((0..n16).map(|i| each + u16::from(i < extra)).collect())
}

impl Layout {
    /// One terminal filling the tab.
    pub fn new(terminal: TerminalId) -> Self {
        Self {
            root: Node::Leaf(terminal),
        }
    }

    /// Every terminal with its place, in `area`, in the tree's order
    /// (left to right, top to bottom).
    pub fn place(&self, area: Rect) -> Vec<(TerminalId, Rect)> {
        let mut out = Vec::new();
        self.root.place(area, &mut out);
        out
    }

    /// The terminals, in the tree's order.
    pub fn terminals(&self) -> Vec<TerminalId> {
        let area = Rect::new(0, 0, u16::MAX, u16::MAX);
        self.place(area).into_iter().map(|(t, _)| t).collect()
    }

    pub fn contains(&self, terminal: &TerminalId) -> bool {
        self.root.contains(terminal)
    }

    /// Make the weights the sizes the cells have in `area`.
    fn settle(&mut self, area: Rect) {
        self.root.settle(area);
    }

    fn path(&self, terminal: &TerminalId) -> Result<Vec<usize>> {
        match self.root.path(terminal) {
            Some(path) => Ok(path),
            None => bail!("no terminal {terminal} in this tab"),
        }
    }

    /// Split `target`'s cell, laid out in `area`, for `new`: side by side
    /// if `columns`, else top and bottom. `new` takes the second half;
    /// an error if there is no room for both.
    pub fn split(
        &mut self,
        target: &TerminalId,
        new: TerminalId,
        columns: bool,
        area: Rect,
    ) -> Result<()> {
        self.settle(area);
        let path = self.path(target)?;
        let size = along(self.root.area_of(&path, area), columns);
        if size < 3 {
            bail!("no room for a new split");
        }
        let second = (size - 1) / 2;
        let first = size - 1 - second;
        let leaf = Cell {
            weight: u32::from(second),
            node: Node::Leaf(new),
        };
        if let Some((&i, parent)) = path.split_last() {
            if let Node::Split { columns: c, cells } = self.root.at_mut(parent) {
                if *c == columns {
                    cells[i].weight = u32::from(first);
                    cells.insert(i + 1, leaf);
                    return Ok(());
                }
            }
        }
        let node = self.root.at_mut(&path);
        let old = std::mem::replace(node, Node::Leaf(target.clone()));
        *node = Node::Split {
            columns,
            cells: vec![
                Cell {
                    weight: u32::from(first),
                    node: old,
                },
                leaf,
            ],
        };
        Ok(())
    }

    /// Take `terminal`'s cell out, laid out in `area`: its place goes to
    /// the cell before it, or after it for the first. False if it was the
    /// last terminal, which stays (the tab goes with it).
    pub fn remove(&mut self, terminal: &TerminalId, area: Rect) -> Result<bool> {
        let path = self.path(terminal)?;
        let Some((&i, parent)) = path.split_last() else {
            return Ok(false);
        };
        self.settle(area);
        let Node::Split { cells, .. } = self.root.at_mut(parent) else {
            unreachable!("a leaf's parent is a split");
        };
        let gone = cells.remove(i);
        let heir = if i == 0 { 0 } else { i - 1 };
        cells[heir].weight += gone.weight + 1;
        if cells.len() == 1 {
            let only = cells.remove(0).node;
            *self.root.at_mut(parent) = only;
            self.merge(parent);
        }
        Ok(true)
    }

    /// The node at `path` took its parent's place: if it runs the same
    /// way as its new parent, its cells join the parent's.
    fn merge(&mut self, path: &[usize]) {
        let Some((&i, parent)) = path.split_last() else {
            return;
        };
        let Node::Split {
            columns: outer,
            cells,
        } = self.root.at_mut(parent)
        else {
            return;
        };
        let inner = match &mut cells[i].node {
            Node::Split {
                columns,
                cells: inner,
            } if columns == outer => std::mem::take(inner),
            _ => return,
        };
        cells.splice(i..=i, inner);
    }

    /// The row (`columns`) or column holding `terminal`'s cell nearest to
    /// it, and the index of the cell there that holds it; `skip_last`
    /// passes over those where it is the last cell.
    fn holder(
        &self,
        terminal: &TerminalId,
        columns: bool,
        skip_last: bool,
    ) -> Result<Option<(Vec<usize>, usize)>> {
        let path = self.path(terminal)?;
        for depth in (0..path.len()).rev() {
            let parent = &path[..depth];
            let Node::Split { columns: c, cells } = self.root.at(parent) else {
                continue;
            };
            if *c == columns && !(skip_last && path[depth] + 1 == cells.len()) {
                return Ok(Some((parent.to_vec(), path[depth])));
            }
        }
        Ok(None)
    }

    /// Move the border after cell `i` of the row or column at `parent` by
    /// `change` cells (towards the end if positive): cell `i` grows from
    /// the cells after it, or it and those before it shrink into the cell
    /// after it, none below its smallest.
    fn shift(&mut self, parent: &[usize], i: usize, change: i32) {
        let Node::Split { columns, cells } = self.root.at_mut(parent) else {
            return;
        };
        let room = |c: &Cell| c.weight.saturating_sub(u32::from(c.node.min(*columns)));
        let mut need = change.unsigned_abs();
        if change > 0 {
            for j in i + 1..cells.len() {
                let take = need.min(room(&cells[j]));
                cells[j].weight -= take;
                cells[i].weight += take;
                need -= take;
            }
        } else if i + 1 < cells.len() {
            for j in (0..=i).rev() {
                let take = need.min(room(&cells[j]));
                cells[j].weight -= take;
                cells[i + 1].weight += take;
                need -= take;
            }
        }
    }

    /// Move `terminal`'s border in `dir` by `cells`, as tmux's
    /// `resize-pane`: the border after its cell in the nearest row (or
    /// column), or before it for the last cell. Stops at the smallest
    /// size; nothing to move is no error.
    pub fn resize(
        &mut self,
        terminal: &TerminalId,
        dir: Direction,
        cells: u16,
        area: Rect,
    ) -> Result<()> {
        let columns = matches!(dir, Direction::Left | Direction::Right);
        let Some((parent, i)) = self.holder(terminal, columns, false)? else {
            return Ok(());
        };
        self.settle(area);
        let Node::Split { cells: row, .. } = self.root.at(&parent) else {
            return Ok(());
        };
        let i = if i + 1 == row.len() { i - 1 } else { i };
        let change = i32::from(cells);
        let change = match dir {
            Direction::Left | Direction::Up => -change,
            Direction::Right | Direction::Down => change,
        };
        self.shift(&parent, i, change);
        Ok(())
    }

    /// Size the cell ending in `terminal` (see [`Found`]) to `size`
    /// along the border's axis, moving the border after it.
    pub fn resize_to(
        &mut self,
        terminal: &TerminalId,
        columns: bool,
        size: u16,
        area: Rect,
    ) -> Result<()> {
        let Some((parent, i)) = self.holder(terminal, columns, true)? else {
            return Ok(());
        };
        self.settle(area);
        let Node::Split { cells, .. } = self.root.at(&parent) else {
            return Ok(());
        };
        let now = i64::from(cells[i].weight);
        let change = i32::try_from(i64::from(size.max(1)) - now).unwrap_or(0);
        self.shift(&parent, i, change);
        Ok(())
    }

    /// The border at `(x, y)` in `area`; `None` if no border is there.
    pub fn border_at(&self, area: Rect, x: u16, y: u16) -> Option<Found> {
        self.root.border_at(area, x, y)
    }

    /// Even out the row or column holding `terminal`, or, if it is even
    /// already, the nearest one around it that is not, as tmux's
    /// `select-layout -E`.
    pub fn spread(&mut self, terminal: &TerminalId, area: Rect) -> Result<()> {
        let path = self.path(terminal)?;
        self.settle(area);
        for depth in (0..path.len()).rev() {
            let parent = &path[..depth];
            let size = self.root.area_of(parent, area);
            let Node::Split { columns, cells } = self.root.at_mut(parent) else {
                continue;
            };
            let Some(sizes) = even(cells.len(), along(size, *columns)) else {
                continue;
            };
            let now: Vec<u32> = cells.iter().map(|c| c.weight).collect();
            let wanted: Vec<u32> = sizes.into_iter().map(u32::from).collect();
            if now != wanted {
                for (cell, w) in cells.iter_mut().zip(wanted) {
                    cell.weight = w;
                }
                return Ok(());
            }
        }
        Ok(())
    }

    /// The terminal next to `terminal` in `dir`, laid out in `area`, as
    /// tmux's `select-pane` finds it: across the border, overlapping it,
    /// the one `rank` puts first (the most recently focused); past the
    /// tab's edge, from the other edge.
    pub fn neighbour(
        &self,
        terminal: &TerminalId,
        dir: Direction,
        area: Rect,
        rank: impl Fn(&TerminalId) -> u64,
    ) -> Option<TerminalId> {
        let placed = self.place(area);
        let (_, me) = placed.iter().find(|(t, _)| t == terminal)?;
        let me = *me;
        let (w, h) = (area.x + area.width, area.y + area.height);
        let overlaps_rows = |r: &Rect| r.y < me.y + me.height && me.y < r.y + r.height;
        let overlaps_cols = |r: &Rect| r.x < me.x + me.width && me.x < r.x + r.width;
        let next = |r: &Rect| match dir {
            Direction::Left => {
                let edge = if me.x == area.x { w + 1 } else { me.x };
                r.x + r.width + 1 == edge && overlaps_rows(r)
            }
            Direction::Right => {
                let edge = me.x + me.width + 1;
                let edge = if edge >= w { area.x } else { edge };
                r.x == edge && overlaps_rows(r)
            }
            Direction::Up => {
                let edge = if me.y == area.y { h + 1 } else { me.y };
                r.y + r.height + 1 == edge && overlaps_cols(r)
            }
            Direction::Down => {
                let edge = me.y + me.height + 1;
                let edge = if edge >= h { area.y } else { edge };
                r.y == edge && overlaps_cols(r)
            }
        };
        let candidates = placed.iter().filter(|(t, r)| t != terminal && next(r));
        // The first of those ranked highest.
        candidates
            .rev()
            .max_by_key(|(t, _)| rank(t))
            .map(|(t, _)| t.clone())
    }
}

#[cfg(test)]
mod tests;
