//! How a tab's terminals are arranged: a tree of splits over terminal ids,
//! the focused one, and the rectangles they get inside a given area. Generic
//! over the id so the rules can be tested without tmux.

pub use crate::input::Direction;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

/// Columns or rows between two split terminals, where the border is drawn.
pub const BORDER: u16 = 1;
/// The smallest terminal paddock gives tmux, as `(cols, rows)`.
pub const MIN_SIZE: (u16, u16) = (2, 1);

/// Which way a split arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Side by side (`C-a |`).
    Columns,
    /// Top and bottom (`C-a -`).
    Rows,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Node<T> {
    Leaf(T),
    Split {
        split: Split,
        children: Vec<Child<T>>,
    },
}

/// One part of a split; it gets space in proportion to its weight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child<T> {
    pub weight: f64,
    pub node: Node<T>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout<T> {
    pub focus: T,
    pub root: Node<T>,
}

/// Where a split's border runs: its cells and which way it divides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Border {
    pub area: Rect,
    pub split: Split,
}

impl<T: Clone + PartialEq> Layout<T> {
    pub fn new(id: T) -> Self {
        Self {
            focus: id.clone(),
            root: Node::Leaf(id),
        }
    }

    /// Every terminal, top-left first.
    pub fn leaves(&self) -> Vec<&T> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    pub fn contains(&self, id: &T) -> bool {
        self.leaves().contains(&id)
    }

    /// Whether every split has parts and every part a positive weight, as a
    /// layout read from a file might not.
    pub fn well_formed(&self) -> bool {
        self.root.well_formed()
    }

    /// Split the focused terminal in two; `new` takes the second half and focus.
    /// Splitting the same way as the surrounding split adds a sibling instead
    /// of nesting.
    pub fn split(&mut self, split: Split, new: T) {
        let focus = self.focus.clone();
        if !self.root.split_in_parent(&focus, split, &new) {
            let old = std::mem::replace(&mut self.root, Node::Leaf(new.clone()));
            self.root = old.split_leaf(&focus, split, &new);
        }
        self.focus = new;
    }

    /// Take terminal `id` out; its space goes to a neighbour, which also gets
    /// focus if `id` had it. Returns false if no terminal is left.
    pub fn remove(&mut self, id: &T) -> bool {
        if matches!(&self.root, Node::Leaf(l) if l == id) {
            return false;
        }
        if let Some(neighbour) = self.root.remove(id) {
            if self.focus == *id {
                self.focus = neighbour;
            }
        }
        true
    }

    /// Give every part of every split the same share.
    pub fn equalise(&mut self) {
        self.root.equalise();
    }

    /// Each terminal's rectangle inside `area`, top-left first, and the borders
    /// between them.
    pub fn rects(&self, area: Rect) -> (Vec<(T, Rect)>, Vec<Border>) {
        let (mut panes, mut borders) = (Vec::new(), Vec::new());
        self.root.place(area, &mut panes, &mut borders);
        (panes, borders)
    }

    /// The focused terminal's rectangle inside `area`.
    pub fn focus_rect(&self, area: Rect) -> Option<Rect> {
        self.rects(area)
            .0
            .into_iter()
            .find(|(id, _)| *id == self.focus)
            .map(|(_, r)| r)
    }

    /// Focus the nearest terminal in `dir`, if there is one. Returns whether
    /// focus moved.
    pub fn move_focus(&mut self, dir: Direction, area: Rect) -> bool {
        let (panes, _) = self.rects(area);
        let Some(from) = self.focus_rect(area) else {
            return false;
        };
        match neighbour(&panes, from, dir) {
            Some(id) => {
                self.focus = id;
                true
            }
            None => false,
        }
    }
}

impl<T: Clone + PartialEq> Node<T> {
    fn well_formed(&self) -> bool {
        match self {
            Node::Leaf(_) => true,
            Node::Split { children, .. } => {
                !children.is_empty()
                    && children
                        .iter()
                        .all(|c| c.weight.is_finite() && c.weight > 0.0 && c.node.well_formed())
            }
        }
    }

    fn leaves<'a>(&'a self, out: &mut Vec<&'a T>) {
        match self {
            Node::Leaf(id) => out.push(id),
            Node::Split { children, .. } => children.iter().for_each(|c| c.node.leaves(out)),
        }
    }

    fn first_leaf(&self) -> &T {
        match self {
            Node::Leaf(id) => id,
            Node::Split { children, .. } => children[0].node.first_leaf(),
        }
    }

    /// If leaf `id` sits directly in a split of kind `split`, add `new` right
    /// after it, sharing its weight. Returns whether it did.
    fn split_in_parent(&mut self, id: &T, split: Split, new: &T) -> bool {
        let Node::Split {
            split: kind,
            children,
        } = self
        else {
            return false;
        };
        let here = children
            .iter()
            .position(|c| matches!(&c.node, Node::Leaf(l) if l == id));
        match here {
            Some(i) if *kind == split => {
                let half = children[i].weight / 2.0;
                children[i].weight = half;
                children.insert(
                    i + 1,
                    Child {
                        weight: half,
                        node: Node::Leaf(new.clone()),
                    },
                );
                true
            }
            Some(_) => false,
            None => children
                .iter_mut()
                .any(|c| c.node.split_in_parent(id, split, new)),
        }
    }

    /// Replace leaf `id` with a split of it and `new`, half each.
    fn split_leaf(self, id: &T, split: Split, new: &T) -> Self {
        match self {
            Node::Leaf(l) if l == *id => Node::Split {
                split,
                children: vec![
                    Child {
                        weight: 1.0,
                        node: Node::Leaf(l),
                    },
                    Child {
                        weight: 1.0,
                        node: Node::Leaf(new.clone()),
                    },
                ],
            },
            Node::Split { split: s, children } => Node::Split {
                split: s,
                children: children
                    .into_iter()
                    .map(|c| Child {
                        weight: c.weight,
                        node: c.node.split_leaf(id, split, new),
                    })
                    .collect(),
            },
            leaf => leaf,
        }
    }

    /// Remove leaf `id` from below this split. Returns a terminal of the
    /// neighbour that took its space.
    fn remove(&mut self, id: &T) -> Option<T> {
        let Node::Split { children, .. } = self else {
            return None;
        };
        let here = children
            .iter()
            .position(|c| matches!(&c.node, Node::Leaf(l) if l == id));
        let Some(i) = here else {
            let found = children.iter_mut().find_map(|c| c.node.remove(id));
            self.collapse();
            return found;
        };
        let gone = children.remove(i);
        let next = if i > 0 { i - 1 } else { 0 };
        children[next].weight += gone.weight;
        let neighbour = children[next].node.first_leaf().clone();
        self.collapse();
        Some(neighbour)
    }

    /// A split left with one part becomes that part, and a split directly
    /// inside one of the same kind is merged into it (keeping every size), so
    /// equalising treats its parts as siblings.
    fn collapse(&mut self) {
        if let Node::Split { split, children } = self {
            for c in children.iter_mut() {
                c.node.collapse();
            }
            let kind = *split;
            *children = std::mem::take(children)
                .into_iter()
                .flat_map(|c| match c.node {
                    Node::Split {
                        split,
                        children: inner,
                    } if split == kind => {
                        let sum: f64 = inner.iter().map(|g| g.weight).sum();
                        inner
                            .into_iter()
                            .map(|g| Child {
                                weight: c.weight * g.weight / sum,
                                node: g.node,
                            })
                            .collect()
                    }
                    node => vec![Child {
                        weight: c.weight,
                        node,
                    }],
                })
                .collect();
            if children.len() == 1 {
                let only = children.remove(0).node;
                *self = only;
            }
        }
    }

    fn equalise(&mut self) {
        if let Node::Split { children, .. } = self {
            for c in children.iter_mut() {
                c.weight = 1.0;
                c.node.equalise();
            }
        }
    }

    fn place(&self, area: Rect, panes: &mut Vec<(T, Rect)>, borders: &mut Vec<Border>) {
        let (split, children) = match self {
            Node::Leaf(id) => {
                panes.push((id.clone(), area));
                return;
            }
            Node::Split { split, children } => (*split, children),
        };
        let extent = match split {
            Split::Columns => area.width,
            Split::Rows => area.height,
        };
        let weights: Vec<f64> = children.iter().map(|c| c.weight).collect();
        let sizes = shares(extent, &weights);
        let mut offset = 0;
        for (i, (child, size)) in children.iter().zip(sizes).enumerate() {
            if i > 0 {
                // A too small area has no room for every border: clip them.
                let line = along(area, split, offset, BORDER).intersection(area);
                borders.push(Border { area: line, split });
                offset += BORDER;
            }
            child.node.place(
                along(area, split, offset, size).intersection(area),
                panes,
                borders,
            );
            offset += size;
        }
    }
}

/// The part of `area` from `offset` for `size` cells along `split`.
fn along(area: Rect, split: Split, offset: u16, size: u16) -> Rect {
    match split {
        Split::Columns => Rect {
            x: area.x + offset,
            width: size,
            ..area
        },
        Split::Rows => Rect {
            y: area.y + offset,
            height: size,
            ..area
        },
    }
}

/// Split `extent` cells among parts of the given weights, with a border
/// between each two. Boundaries are rounded from the running total, so equal
/// weights differ by at most one cell.
pub fn shares(extent: u16, weights: &[f64]) -> Vec<u16> {
    let n = u16::try_from(weights.len()).unwrap_or(u16::MAX);
    let total = extent.saturating_sub(n.saturating_sub(1) * BORDER);
    let sum: f64 = weights.iter().sum();
    let mut out = Vec::with_capacity(weights.len());
    let (mut done, mut acc) = (0u16, 0.0);
    for w in weights {
        acc += w;
        // Bounded by `total`, which fits in u16.
        let end = (f64::from(total) * acc / sum).round() as u16;
        out.push(end.saturating_sub(done));
        done = done.max(end);
    }
    out
}

/// The terminal nearest to `from` in `dir`: the closest one beyond its edge
/// that overlaps it across, preferring the one most in line with it.
fn neighbour<T: Clone>(panes: &[(T, Rect)], from: Rect, dir: Direction) -> Option<T> {
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| a0 < b1 && b0 < a1;
    panes
        .iter()
        .filter_map(|(id, r)| {
            let (gap, offset) = match dir {
                Direction::Left if r.right() <= from.x => (
                    overlap(r.y, r.bottom(), from.y, from.bottom()).then(|| from.x - r.right())?,
                    r.y.abs_diff(from.y),
                ),
                Direction::Right if r.x >= from.right() => (
                    overlap(r.y, r.bottom(), from.y, from.bottom()).then(|| r.x - from.right())?,
                    r.y.abs_diff(from.y),
                ),
                Direction::Up if r.bottom() <= from.y => (
                    overlap(r.x, r.right(), from.x, from.right()).then(|| from.y - r.bottom())?,
                    r.x.abs_diff(from.x),
                ),
                Direction::Down if r.y >= from.bottom() => (
                    overlap(r.x, r.right(), from.x, from.right()).then(|| r.y - from.bottom())?,
                    r.x.abs_diff(from.x),
                ),
                _ => return None,
            };
            Some(((gap, offset), id))
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, id)| id.clone())
}

/// The size a terminal gets for `rect`: at least [`MIN_SIZE`].
pub fn terminal_size(rect: Rect) -> (u16, u16) {
    (rect.width.max(MIN_SIZE.0), rect.height.max(MIN_SIZE.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 1,
        width: 99,
        height: 29,
    };

    fn sizes(l: &Layout<u32>) -> Vec<(u32, u16, u16, u16, u16)> {
        l.rects(AREA)
            .0
            .into_iter()
            .map(|(id, r)| (id, r.x, r.y, r.width, r.height))
            .collect()
    }

    #[test]
    fn one_terminal_fills_the_area() {
        let l = Layout::new(1);
        assert_eq!(sizes(&l), [(1, 0, 1, 99, 29)]);
        assert!(l.rects(AREA).1.is_empty());
        assert_eq!(l.leaves(), [&1]);
    }

    #[test]
    fn splitting_side_by_side_halves_the_focused_terminal() {
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        assert_eq!(l.focus, 2);
        assert_eq!(sizes(&l), [(1, 0, 1, 49, 29), (2, 50, 1, 49, 29)]);
        let borders = l.rects(AREA).1;
        assert_eq!(
            borders,
            [Border {
                area: Rect::new(49, 1, 1, 29),
                split: Split::Columns
            }]
        );
    }

    #[test]
    fn splitting_top_and_bottom_nests_inside_a_side_by_side_split() {
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Rows, 3);
        assert_eq!(
            sizes(&l),
            [(1, 0, 1, 49, 29), (2, 50, 1, 49, 14), (3, 50, 16, 49, 14)]
        );
        assert_eq!(l.leaves(), [&1, &2, &3]);
    }

    #[test]
    fn splitting_the_same_way_adds_a_sibling_and_equalise_evens_them() {
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Columns, 3);
        // 97 columns: 1 keeps half, 2 and 3 share the other half.
        let widths: Vec<u16> = sizes(&l).iter().map(|s| s.3).collect();
        assert_eq!(widths, [49, 24, 24]);
        l.equalise();
        let widths: Vec<u16> = sizes(&l).iter().map(|s| s.3).collect();
        assert_eq!(widths, [32, 33, 32]);
    }

    #[test]
    fn a_split_left_inside_one_of_its_own_kind_is_merged_into_it() {
        // 1 | (2 over (3 | 4)); removing 2 leaves 1 | (3 | 4).
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Rows, 3);
        l.split(Split::Columns, 4);
        l.remove(&2);
        let widths: Vec<u16> = sizes(&l).iter().map(|s| s.3).collect();
        assert_eq!(widths, [49, 24, 24], "sizes do not change");
        l.equalise();
        let widths: Vec<u16> = sizes(&l).iter().map(|s| s.3).collect();
        assert_eq!(widths, [32, 33, 32]);
    }

    #[test]
    fn shares_fill_the_extent_minus_borders() {
        assert_eq!(shares(100, &[1.0, 1.0, 1.0]), [33, 32, 33]);
        assert_eq!(shares(10, &[1.0]), [10]);
        assert_eq!(shares(3, &[1.0, 1.0, 1.0]), [0, 1, 0]);
        for n in 1..6 {
            let w = vec![1.0; n];
            let total: u16 = shares(80, &w).iter().sum();
            assert_eq!(usize::from(total) + n - 1, 80);
        }
    }

    #[test]
    fn removing_gives_the_space_to_a_neighbour_and_moves_focus() {
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Rows, 3);
        assert!(l.remove(&3));
        assert_eq!(l.focus, 2);
        assert_eq!(sizes(&l), [(1, 0, 1, 49, 29), (2, 50, 1, 49, 29)]);
        assert!(l.remove(&1));
        assert_eq!(l.root, Node::Leaf(2));
        assert!(!l.remove(&2), "the last terminal leaves nothing");
    }

    #[test]
    fn removing_an_unfocused_terminal_keeps_focus() {
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Columns, 3);
        l.focus = 1;
        assert!(l.remove(&2));
        assert_eq!(l.focus, 1);
        assert_eq!(l.leaves(), [&1, &3]);
        assert!(l.remove(&9), "unknown terminals are ignored");
    }

    #[test]
    fn focus_moves_to_the_nearest_terminal_in_line() {
        // 1 | 2
        //   | 3
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Rows, 3);
        assert!(!l.move_focus(Direction::Right, AREA));
        assert!(l.move_focus(Direction::Up, AREA));
        assert_eq!(l.focus, 2);
        assert!(!l.move_focus(Direction::Up, AREA));
        assert!(l.move_focus(Direction::Left, AREA));
        assert_eq!(l.focus, 1);
        assert!(l.move_focus(Direction::Right, AREA));
        assert_eq!(l.focus, 2, "the top one is most in line");
        assert!(l.move_focus(Direction::Down, AREA));
        assert_eq!(l.focus, 3);
    }

    #[test]
    fn nothing_is_placed_outside_a_too_small_area() {
        let mut l = Layout::new(1);
        l.split(Split::Columns, 2);
        l.split(Split::Columns, 3);
        l.split(Split::Rows, 4);
        for area in [
            Rect::new(20, 1, 0, 5),
            Rect::new(0, 1, 1, 5),
            Rect::new(0, 1, 9, 0),
        ] {
            let (panes, borders) = l.rects(area);
            let rects = panes
                .iter()
                .map(|(_, r)| *r)
                .chain(borders.iter().map(|b| b.area));
            for r in rects {
                assert!(
                    r.is_empty() || area.intersection(r) == r,
                    "{r:?} in {area:?}"
                );
            }
        }
    }

    #[test]
    fn terminals_never_get_less_than_the_minimum_size() {
        assert_eq!(terminal_size(Rect::new(0, 0, 0, 0)), MIN_SIZE);
        assert_eq!(terminal_size(Rect::new(0, 0, 40, 10)), (40, 10));
    }

    #[test]
    fn layouts_round_trip_through_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct File {
            tabs: Vec<Layout<String>>,
        }
        let mut l = Layout::new("$1".to_string());
        l.split(Split::Columns, "$2".into());
        l.split(Split::Rows, "$3".into());
        l.split(Split::Rows, "$4".into());
        let file = File { tabs: vec![l] };
        let text = toml::to_string(&file).unwrap();
        assert_eq!(toml::from_str::<File>(&text).unwrap(), file);
    }
}
