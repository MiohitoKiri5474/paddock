//! A tab's tree of splits as `layout.json` keeps it (see
//! [`crate::backend::native::saved`]): the same rows and columns with
//! their weights, each terminal as whatever the file keeps of it (its
//! directory), never its id. A restored tree gets new terminals.

use super::{Axis, Cell, Layout, Node};
use crate::backend::TerminalId;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// One node of a saved tree: a terminal, as `T`, or a row or column of
/// cells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedTree<T> {
    Terminal(T),
    Split {
        axis: Axis,
        cells: Vec<SavedCell<T>>,
    },
}

/// A node in a saved row or column, with its weight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedCell<T> {
    pub weight: u32,
    pub node: SavedTree<T>,
}

impl<T> SavedTree<T> {
    /// The same tree, each terminal made `f` of it, in the tree's order.
    pub fn map<U>(self, f: &mut impl FnMut(T) -> U) -> SavedTree<U> {
        match self {
            Self::Terminal(t) => SavedTree::Terminal(f(t)),
            Self::Split { axis, cells } => SavedTree::Split {
                axis,
                cells: cells
                    .into_iter()
                    .map(|c| SavedCell {
                        weight: c.weight,
                        node: c.node.map(f),
                    })
                    .collect(),
            },
        }
    }

    /// Its terminals, in the tree's order.
    pub fn terminals(&self) -> Vec<&T> {
        let mut out = Vec::new();
        self.walk(&mut out);
        out
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a T>) {
        match self {
            Self::Terminal(t) => out.push(t),
            Self::Split { cells, .. } => cells.iter().for_each(|c| c.node.walk(out)),
        }
    }

    /// An error unless the tree is one a layout can hold: every row of
    /// two cells or more, as a row left with one gives way to it.
    pub fn check(&self) -> Result<()> {
        match self {
            Self::Terminal(_) => Ok(()),
            Self::Split { cells, .. } if cells.len() < 2 => {
                bail!("a split of {} cells", cells.len())
            }
            Self::Split { cells, .. } => cells.iter().try_for_each(|c| c.node.check()),
        }
    }
}

impl Layout {
    /// The tree as `layout.json` keeps it, each terminal as `f` makes it.
    pub fn save<T>(&self, f: &mut impl FnMut(&TerminalId) -> T) -> SavedTree<T> {
        save(&self.root, f)
    }

    /// The layout `saved` holds, its terminals already given their ids;
    /// an error if it is no tree a layout can hold (see
    /// [`SavedTree::check`]).
    pub fn restore(saved: SavedTree<TerminalId>) -> Result<Self> {
        saved.check()?;
        Ok(Self {
            root: restore(saved),
        })
    }
}

fn save<T>(node: &Node, f: &mut impl FnMut(&TerminalId) -> T) -> SavedTree<T> {
    match node {
        Node::Leaf(t) => SavedTree::Terminal(f(t)),
        Node::Split { axis, cells } => SavedTree::Split {
            axis: *axis,
            cells: cells
                .iter()
                .map(|c| SavedCell {
                    weight: c.weight,
                    node: save(&c.node, f),
                })
                .collect(),
        },
    }
}

fn restore(saved: SavedTree<TerminalId>) -> Node {
    match saved {
        SavedTree::Terminal(t) => Node::Leaf(t),
        SavedTree::Split { axis, cells } => Node::Split {
            axis,
            cells: cells
                .into_iter()
                .map(|c| Cell {
                    weight: c.weight,
                    node: restore(c.node),
                })
                .collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(n: u32) -> TerminalId {
        TerminalId::new(format!("%{n}"))
    }

    #[test]
    fn a_saved_tree_comes_back_with_its_shares_and_new_terminals() {
        let size = (68, 29);
        let mut layout = Layout::new(t(1));
        layout.split(&t(1), t(2), Axis::Columns, size).unwrap();
        layout.split(&t(2), t(3), Axis::Rows, size).unwrap();
        layout.resize_to(&t(1), Axis::Columns, 20, size).unwrap();
        let saved = layout.save(&mut |id| format!("dir of {id}"));
        let dirs: Vec<_> = saved.terminals().into_iter().cloned().collect();
        assert_eq!(dirs, ["dir of %1", "dir of %2", "dir of %3"]);

        let mut n = 10;
        let restored = Layout::restore(saved.map(&mut |_| {
            n += 1;
            t(n)
        }))
        .unwrap();

        assert_eq!(restored.terminals(), [t(11), t(12), t(13)]);
        let rects = |l: &Layout| {
            l.place(size)
                .into_iter()
                .map(|(_, r)| r)
                .collect::<Vec<_>>()
        };
        assert_eq!(rects(&restored), rects(&layout));
    }

    #[test]
    fn a_split_of_fewer_than_two_cells_is_refused() {
        let one = SavedTree::Split {
            axis: Axis::Rows,
            cells: vec![SavedCell {
                weight: 1,
                node: SavedTree::Terminal(t(1)),
            }],
        };
        assert!(Layout::restore(one).is_err());
        let none: SavedTree<TerminalId> = SavedTree::Split {
            axis: Axis::Rows,
            cells: vec![],
        };
        assert!(Layout::restore(none).is_err());
    }
}
