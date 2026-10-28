//! A tab's layout on the native server: a tree of splits, each a row of
//! cells side by side ([`Axis::Columns`]) or a column of cells one above
//! another ([`Axis::Rows`]), with one cell between neighbours for the
//! border, and each cell's share of its parent (its weight). Laid out at
//! a size, the tree gives every terminal its place; resizing the tab
//! keeps the shares.
//!
//! It does what tmux does with its own tree (ADR 0006), down to the cell:
//! a split takes the second half of its terminal (`(size - 1) / 2`), into
//! the same row when splitting the way the row runs; a closed cell's
//! place goes to the cell before it (after it, for the first); `=`
//! evens out the nearest row or column it can, the first cells taking
//! what is left over; and the resize keys move the border after the
//! focused cell (before it, for the last). A drag sizes the cell before
//! the border.
//!
//! Every change works on cell sizes: the tree's weights are first made
//! the sizes it has at that size ([`Node::settle`]), so whatever was left
//! over when it was last laid out at another size stays where it was
//! drawn. A row too small to give each cell its share (each needs at
//! least one cell) keeps its weights through `=` and a closed terminal,
//! so the shares come back once the tab grows again; a change that moves
//! cells (a split, the resize keys, a drag) takes the sizes drawn there,
//! so it moves what is drawn.
//!
//! A tab smaller than its smallest layout (one cell per terminal, plus
//! the borders) is laid out at that smallest size: no terminal is ever
//! left with no cell, and the client draws what fits.

use super::axis::Axis;
use crate::backend::TerminalId;
use crate::input::Direction;
use anyhow::{bail, Result};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

/// One node of the tree: a terminal, or a row or column of cells.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Leaf(TerminalId),
    Split { axis: Axis, cells: Vec<Cell> },
}

/// A node in a row or column, with its weight: its share of the
/// row's width (or column's height), borders left out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    weight: u32,
    node: Node,
}

/// The border between two cells found at a place ([`Layout::border_at`]):
/// it moves by sizing the cell before it, the one that ends in `terminal`
/// (its last terminal) and starts at `start` along `axis`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BorderHit {
    pub terminal: TerminalId,
    pub axis: Axis,
    pub start: u16,
}

/// A tab's tree of splits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: Node,
}

impl Node {
    /// The smallest this node gets along `axis`.
    fn min(&self, axis: Axis) -> u16 {
        match self {
            Node::Leaf(_) => 1,
            Node::Split { axis: a, cells } => {
                let mins = cells.iter().map(|cell| cell.node.min(axis));
                if *a == axis {
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
            Node::Split { axis, cells } => {
                let rects = cell_rects(cells, *axis, area);
                for (cell, rect) in cells.iter().zip(rects) {
                    cell.node.place(rect, out);
                }
            }
        }
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a TerminalId>) {
        match self {
            Node::Leaf(t) => out.push(t),
            Node::Split { cells, .. } => cells.iter().for_each(|c| c.node.walk(out)),
        }
    }

    /// Make every weight below this node, laid out in `area`, the size
    /// its cell has there. With [`Squeezed::Keep`], a row that had to give
    /// a cell more than its share to keep it at its smallest keeps its
    /// weights instead.
    fn settle(&mut self, area: Rect, squeezed: Squeezed) {
        if let Node::Split { axis, cells } = self {
            let (rects, short) = lay_cells(cells, *axis, area);
            let keep = short && squeezed == Squeezed::Keep;
            for (cell, rect) in cells.iter_mut().zip(rects) {
                if !keep {
                    cell.weight = u32::from(axis.len(rect));
                }
                cell.node.settle(rect, squeezed);
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
            (Some((&i, rest)), Node::Split { axis, cells }) => {
                let rect = cell_rects(cells, *axis, area)[i];
                cells[i].node.area_of(rest, rect)
            }
            _ => area,
        }
    }

    fn border_at(&self, area: Rect, x: u16, y: u16) -> Option<BorderHit> {
        let Node::Split { axis, cells } = self else {
            return None;
        };
        let rects = cell_rects(cells, *axis, area);
        if let Some(i) = rects.iter().position(|r| contains(*r, x, y)) {
            return cells[i].node.border_at(rects[i], x, y);
        }
        let across = axis.across();
        let at_across = u32::from(across.pick(x, y));
        if at_across < u32::from(across.start(area)) || at_across >= across.end(area) {
            return None;
        }
        let at = u32::from(axis.pick(x, y));
        let i = rects.iter().position(|r| axis.end(*r) == at)?;
        (i + 1 < cells.len()).then(|| BorderHit {
            terminal: cells[i].node.last_leaf().clone(),
            axis: *axis,
            start: axis.start(rects[i]),
        })
    }
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    let (x, y) = (u32::from(x), u32::from(y));
    let (left, top) = (u32::from(r.x), u32::from(r.y));
    left <= x && x < left + u32::from(r.width) && top <= y && y < top + u32::from(r.height)
}

/// What [`Node::settle`] does with a squeezed row: one too small to give
/// each cell its share, so a cell got more to keep it at its smallest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Squeezed {
    /// Keep its weights, so the shares come back when the tab grows: for
    /// changes that work on shares (`=`, closing a terminal).
    Keep,
    /// Make its weights the sizes drawn: for changes that work on cells
    /// (a split, the resize keys, a drag), which must move what is drawn.
    Drawn,
}

/// Where each cell of a row or column running along `axis` sits in
/// `area`.
fn cell_rects(cells: &[Cell], axis: Axis, area: Rect) -> Vec<Rect> {
    lay_cells(cells, axis, area).0
}

/// [`cell_rects`], and whether a cell got more than its share to keep it
/// at its smallest (the row is squeezed).
fn lay_cells(cells: &[Cell], axis: Axis, area: Rect) -> (Vec<Rect>, bool) {
    let weights: Vec<u32> = cells.iter().map(|c| c.weight).collect();
    let mins: Vec<u16> = cells.iter().map(|c| c.node.min(axis)).collect();
    let (sizes, squeezed) = distribute(&weights, &mins, axis.len(area));
    let mut at = axis.start(area);
    let rects = sizes
        .into_iter()
        .map(|size| {
            let rect = axis.cut(area, at, size);
            at = at.saturating_add(size).saturating_add(1);
            rect
        })
        .collect();
    (rects, squeezed)
}

/// Share `total` out between cells by `weights`, a border between each
/// two, each at least its `min`: the sizes add up to `total` less the
/// borders, which the caller makes room for (see [`Layout::area`]).
/// Weights that are the sizes a row had at `total` give those sizes
/// back. Also whether a cell below its smallest took from another.
fn distribute(weights: &[u32], mins: &[u16], total: u16) -> (Vec<u16>, bool) {
    let n = weights.len();
    let borders = u16::try_from(n.saturating_sub(1)).unwrap_or(u16::MAX);
    let room = u64::from(total.saturating_sub(borders));
    let sum: u64 = weights.iter().map(|&w| u64::from(w)).sum();
    let mut sizes = Vec::with_capacity(n);
    let (mut cum, mut placed) = (0u64, 0u64);
    for (i, &w) in weights.iter().enumerate() {
        cum += if sum == 0 { 1 } else { u64::from(w) };
        let whole = if sum == 0 { n as u64 } else { sum };
        let edge = if i + 1 == n {
            room
        } else {
            (cum * room + whole / 2) / whole
        };
        sizes.push(u16::try_from(edge - placed).unwrap_or(u16::MAX));
        placed = edge;
    }
    // Cells below their smallest take from the biggest above theirs.
    let mut squeezed = false;
    for i in 0..n {
        while sizes[i] < mins[i] {
            let donor = (0..n)
                .filter(|&j| j != i && sizes[j] > mins[j])
                .max_by_key(|&j| sizes[j] - mins[j]);
            let Some(j) = donor else { break };
            sizes[j] -= 1;
            sizes[i] += 1;
            squeezed = true;
        }
    }
    (sizes, squeezed)
}

/// Even sizes for `n` cells in `total`, as tmux's `select-layout -E`
/// gives them: the first cells take one more until what is left over is
/// gone. `None` if there is no room for one cell each.
fn even(n: usize, total: u16) -> Option<Vec<u16>> {
    let n16 = u16::try_from(n).ok()?;
    let room = total.checked_sub(n16.checked_sub(1)?)?;
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

    /// Where a tab of `(cols, rows)` lays its terminals out: the tab, or
    /// the tree's smallest size along an axis where the tab is smaller.
    fn area(&self, (cols, rows): (u16, u16)) -> Rect {
        let cols = cols.max(self.root.min(Axis::Columns));
        let rows = rows.max(self.root.min(Axis::Rows));
        Rect::new(0, 0, cols, rows)
    }

    /// Every terminal with its place in a tab of `size`, in the tree's
    /// order (left to right, top to bottom).
    pub fn place(&self, size: (u16, u16)) -> Vec<(TerminalId, Rect)> {
        let mut out = Vec::new();
        self.root.place(self.area(size), &mut out);
        out
    }

    /// The terminals, in the tree's order.
    pub fn terminals(&self) -> Vec<TerminalId> {
        let mut out = Vec::new();
        self.root.walk(&mut out);
        out.into_iter().cloned().collect()
    }

    pub fn contains(&self, terminal: &TerminalId) -> bool {
        self.root.contains(terminal)
    }

    fn path(&self, terminal: &TerminalId) -> Result<Vec<usize>> {
        match self.root.path(terminal) {
            Some(path) => Ok(path),
            None => bail!("no terminal {terminal} in this tab"),
        }
    }

    /// Split `target`'s cell, in a tab of `size`, for `new`, along
    /// `axis`. `new` takes the second half; an error if there is no room
    /// for both.
    pub fn split(
        &mut self,
        target: &TerminalId,
        new: TerminalId,
        axis: Axis,
        size: (u16, u16),
    ) -> Result<()> {
        let area = self.area(size);
        let path = self.path(target)?;
        let len = axis.len(self.root.area_of(&path, area));
        if len < 3 {
            bail!("no room for a new split");
        }
        self.root.settle(area, Squeezed::Drawn);
        let second = (len - 1) / 2;
        let first = len - 1 - second;
        let leaf = Cell {
            weight: u32::from(second),
            node: Node::Leaf(new),
        };
        if let Some((&i, parent)) = path.split_last() {
            if let Node::Split { axis: a, cells } = self.root.at_mut(parent) {
                if *a == axis {
                    cells[i].weight = u32::from(first);
                    cells.insert(i + 1, leaf);
                    return Ok(());
                }
            }
        }
        let node = self.root.at_mut(&path);
        let old = std::mem::replace(node, Node::Leaf(target.clone()));
        *node = Node::Split {
            axis,
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

    /// Take `terminal`'s cell out, in a tab of `size`: its place goes to
    /// the cell before it, or after it for the first. False if it was the
    /// last terminal, which stays (the tab goes with it).
    pub fn remove(&mut self, terminal: &TerminalId, size: (u16, u16)) -> Result<bool> {
        let path = self.path(terminal)?;
        let Some((&i, parent)) = path.split_last() else {
            return Ok(false);
        };
        self.root.settle(self.area(size), Squeezed::Keep);
        let Node::Split { cells, .. } = self.root.at_mut(parent) else {
            unreachable!("a leaf's parent is a split");
        };
        let gone = cells.remove(i);
        let heir = i.saturating_sub(1);
        cells[heir].weight = cells[heir].weight.saturating_add(gone.weight + 1);
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
        let Node::Split { axis: outer, cells } = self.root.at_mut(parent) else {
            return;
        };
        let inner = match &mut cells[i].node {
            Node::Split { axis, cells: inner } if axis == outer => std::mem::take(inner),
            _ => return,
        };
        cells.splice(i..=i, inner);
    }

    /// The row or column along `axis` nearest to `terminal`'s cell that
    /// holds it, and the index of the cell there that holds it;
    /// `skip_last` passes over those where it is the last cell.
    fn holder(
        &self,
        terminal: &TerminalId,
        axis: Axis,
        skip_last: bool,
    ) -> Result<Option<(Vec<usize>, usize)>> {
        let path = self.path(terminal)?;
        for depth in (0..path.len()).rev() {
            let parent = &path[..depth];
            let Node::Split { axis: a, cells } = self.root.at(parent) else {
                continue;
            };
            if *a == axis && !(skip_last && path[depth] + 1 == cells.len()) {
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
        let Node::Split { axis, cells } = self.root.at_mut(parent) else {
            return;
        };
        let room = |c: &Cell| c.weight.saturating_sub(u32::from(c.node.min(*axis)));
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

    /// Move `terminal`'s border in `dir` by `cells`, in a tab of `size`,
    /// as tmux's `resize-pane`: the border after its cell in the nearest
    /// row (or column), or before it for the last cell. Stops at the
    /// smallest size; nothing to move is no error.
    pub fn resize(
        &mut self,
        terminal: &TerminalId,
        dir: Direction,
        cells: u16,
        size: (u16, u16),
    ) -> Result<()> {
        let Some((parent, i)) = self.holder(terminal, Axis::of_direction(dir), false)? else {
            return Ok(());
        };
        self.root.settle(self.area(size), Squeezed::Drawn);
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

    /// Size the cell ending in `terminal` (see [`BorderHit`]) to `len`
    /// along `axis`, in a tab of `size`, moving the border after it.
    pub fn resize_to(
        &mut self,
        terminal: &TerminalId,
        axis: Axis,
        len: u16,
        size: (u16, u16),
    ) -> Result<()> {
        let Some((parent, i)) = self.holder(terminal, axis, true)? else {
            return Ok(());
        };
        self.root.settle(self.area(size), Squeezed::Drawn);
        let Node::Split { cells, .. } = self.root.at(&parent) else {
            return Ok(());
        };
        let now = i64::from(cells[i].weight);
        let change = i32::try_from(i64::from(len.max(1)) - now).unwrap_or(0);
        self.shift(&parent, i, change);
        Ok(())
    }

    /// The border at `(x, y)` of a tab of `size`; `None` if no border is
    /// there.
    pub fn border_at(&self, size: (u16, u16), x: u16, y: u16) -> Option<BorderHit> {
        self.root.border_at(self.area(size), x, y)
    }

    /// Even out the row or column holding `terminal`, in a tab of `size`,
    /// or, if that changes nothing drawn (it is even already, or a
    /// smallest size inside it is in the way), the nearest one around it
    /// that it changes, as tmux's `select-layout -E`.
    pub fn spread(&mut self, terminal: &TerminalId, size: (u16, u16)) -> Result<()> {
        let path = self.path(terminal)?;
        let area = self.area(size);
        self.root.settle(area, Squeezed::Keep);
        let before = self.place(size);
        for depth in (0..path.len()).rev() {
            let parent = &path[..depth];
            let parent_area = self.root.area_of(parent, area);
            let saved = self.root.clone();
            let Node::Split { axis, cells } = self.root.at_mut(parent) else {
                continue;
            };
            let Some(sizes) = even(cells.len(), axis.len(parent_area)) else {
                continue;
            };
            for (cell, len) in cells.iter_mut().zip(sizes) {
                cell.weight = u32::from(len);
            }
            if self.place(size) != before {
                return Ok(());
            }
            self.root = saved;
        }
        Ok(())
    }

    /// The terminal next to `terminal` in `dir`, in a tab of `size`, as
    /// tmux's `select-pane` finds it: across the border, overlapping it,
    /// the one `rank` puts first (the most recently focused); past the
    /// tab's edge, from the other edge.
    pub fn neighbour(
        &self,
        terminal: &TerminalId,
        dir: Direction,
        size: (u16, u16),
        rank: impl Fn(&TerminalId) -> u64,
    ) -> Option<TerminalId> {
        let area = self.area(size);
        let placed = self.place(size);
        let (_, me) = placed.iter().find(|(t, _)| t == terminal)?;
        let axis = Axis::of_direction(dir);
        let across = axis.across();
        let overlaps = |r: &Rect| {
            u32::from(across.start(*r)) < across.end(*me)
                && u32::from(across.start(*me)) < across.end(*r)
        };
        let (first, last) = (u32::from(axis.start(area)), axis.end(area));
        let backwards = matches!(dir, Direction::Left | Direction::Up);
        let next = |r: &Rect| {
            let hit = if backwards {
                // The cell whose far border is ours, or the last one.
                let edge = u32::from(axis.start(*me));
                let edge = if edge == first { last + 1 } else { edge };
                axis.end(*r) + 1 == edge
            } else {
                let edge = axis.end(*me) + 1;
                let edge = if edge >= last { first } else { edge };
                u32::from(axis.start(*r)) == edge
            };
            hit && overlaps(r)
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
