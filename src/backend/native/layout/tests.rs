use super::*;

/// A tab of 68x29, as the test suite's paddock shows one.
const AREA: Rect = Rect::new(0, 0, 68, 29);

fn t(n: u32) -> TerminalId {
    TerminalId::new(format!("%{n}"))
}

/// Every terminal's size, `WxH`, in the tree's order.
fn sizes(layout: &Layout, area: Rect) -> Vec<String> {
    let placed = layout.place(area);
    placed
        .iter()
        .map(|(_, r)| format!("{}x{}", r.width, r.height))
        .collect()
}

/// Every cell's place adds up: the cells and the borders between them
/// fill each row and column, and no terminal is smaller than one cell.
fn assert_fills(layout: &Layout, area: Rect) {
    fn check(node: &Node, area: Rect) {
        if let Node::Split { columns, cells } = node {
            let rects = cell_rects(cells, *columns, area);
            let used: u32 = rects.iter().map(|r| u32::from(along(*r, *columns))).sum();
            let borders = u32::try_from(cells.len() - 1).unwrap();
            assert_eq!(used + borders, u32::from(along(area, *columns)), "{node:?}");
            for (cell, rect) in cells.iter().zip(rects) {
                check(&cell.node, rect);
            }
        }
    }
    check(&layout.root, area);
    for (t, r) in layout.place(area) {
        assert!(r.width >= 1 && r.height >= 1, "{t} is {r:?}");
    }
}

/// `%0 | %1`: one side-by-side split of the whole tab.
fn side_by_side() -> Layout {
    let mut l = Layout::new(t(0));
    l.split(&t(0), t(1), true, AREA).unwrap();
    l
}

#[test]
fn a_split_gives_the_second_half_to_the_new_terminal_with_a_border_between() {
    let l = side_by_side();
    assert_eq!(sizes(&l, AREA), ["34x29", "33x29"]);
    let mut rows = Layout::new(t(0));
    rows.split(&t(0), t(1), false, AREA).unwrap();
    assert_eq!(sizes(&rows, AREA), ["68x14", "68x14"]);
    let placed = rows.place(AREA);
    assert_eq!(placed[1], (t(1), Rect::new(0, 15, 68, 14)));
    assert_fills(&rows, AREA);
}

#[test]
fn splitting_the_way_a_row_runs_adds_to_that_row() {
    let mut l = side_by_side();
    l.split(&t(1), t(2), true, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["34x29", "16x29", "16x29"]);
    assert!(matches!(&l.root, Node::Split { cells, .. } if cells.len() == 3));
    // Across it: a column inside the row, in the tree's order.
    l.split(&t(0), t(3), false, AREA).unwrap();
    assert_eq!(l.terminals(), [t(0), t(3), t(1), t(2)]);
    assert_eq!(sizes(&l, AREA), ["34x14", "34x14", "16x29", "16x29"]);
    assert_fills(&l, AREA);
}

#[test]
fn a_cell_too_small_to_split_is_an_error() {
    let mut l = Layout::new(t(0));
    let tiny = Rect::new(0, 0, 2, 5);
    assert!(l.split(&t(0), t(1), true, tiny).is_err());
    assert_eq!(l, Layout::new(t(0)), "unchanged");
    assert!(
        l.split(&t(9), t(1), true, AREA).is_err(),
        "no such terminal"
    );
}

#[test]
fn a_closed_cell_goes_to_the_one_before_it_or_after_it_for_the_first() {
    let mut l = side_by_side();
    l.split(&t(1), t(2), true, AREA).unwrap();
    assert!(l.remove(&t(2), AREA).unwrap());
    assert_eq!(sizes(&l, AREA), ["34x29", "33x29"]);
    assert!(l.remove(&t(0), AREA).unwrap());
    assert_eq!(sizes(&l, AREA), ["68x29"]);
    assert_eq!(
        l,
        Layout::new(t(1)),
        "the row is gone with its last but one"
    );
    assert!(!l.remove(&t(1), AREA).unwrap(), "the last terminal stays");
}

#[test]
fn a_column_left_with_one_cell_joins_the_row_around_it() {
    // `%0 | (%1 over %2)`, then `%2` closes: `%0 | %1`, one row again.
    let mut l = side_by_side();
    l.split(&t(1), t(2), false, AREA).unwrap();
    l.split(&t(1), t(3), true, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["34x29", "16x14", "16x14", "33x14"]);
    l.remove(&t(2), AREA).unwrap();
    // `%1 | %3` replaced the column, and joined the outer row.
    assert!(matches!(&l.root, Node::Split { cells, .. } if cells.len() == 3));
    assert_eq!(sizes(&l, AREA), ["34x29", "16x29", "16x29"]);
    assert_fills(&l, AREA);
}

#[test]
fn resize_keys_move_the_border_after_the_cell_or_before_the_last() {
    let mut l = side_by_side();
    // The right one is last: its left border moves.
    l.resize(&t(1), Direction::Left, 2, AREA).unwrap();
    l.resize(&t(1), Direction::Left, 2, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["30x29", "37x29"]);
    l.resize(&t(0), Direction::Right, 4, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["34x29", "33x29"]);
    let mut rows = Layout::new(t(0));
    rows.split(&t(0), t(1), false, AREA).unwrap();
    rows.resize(&t(1), Direction::Up, 4, AREA).unwrap();
    assert_eq!(sizes(&rows, AREA), ["68x10", "68x18"]);
    rows.resize(&t(1), Direction::Down, 2, AREA).unwrap();
    assert_eq!(sizes(&rows, AREA), ["68x12", "68x16"]);
    // No column to move a border in: nothing happens.
    rows.resize(&t(1), Direction::Left, 2, AREA).unwrap();
    assert_eq!(sizes(&rows, AREA), ["68x12", "68x16"]);
}

#[test]
fn a_resize_stops_at_one_cell() {
    let mut l = side_by_side();
    for _ in 0..25 {
        l.resize(&t(1), Direction::Left, 2, AREA).unwrap();
    }
    assert_eq!(sizes(&l, AREA), ["1x29", "66x29"]);
    for _ in 0..40 {
        l.resize(&t(0), Direction::Right, 2, AREA).unwrap();
    }
    assert_eq!(sizes(&l, AREA), ["66x29", "1x29"]);
    assert_fills(&l, AREA);
}

#[test]
fn equal_spreads_the_nearest_uneven_row_the_first_cells_taking_the_rest() {
    let mut l = side_by_side();
    l.resize(&t(1), Direction::Left, 6, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["28x29", "39x29"]);
    l.spread(&t(1), AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["34x29", "33x29"]);
    l.split(&t(1), t(2), true, AREA).unwrap();
    l.spread(&t(2), AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["22x29", "22x29", "22x29"]);
    // An even column inside the row: the row around it evens out.
    let mut l = side_by_side();
    l.split(&t(1), t(2), false, AREA).unwrap();
    l.resize(&t(1), Direction::Left, 6, AREA).unwrap();
    l.spread(&t(2), AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["34x29", "33x14", "33x14"]);
}

#[test]
fn a_drag_sizes_the_cell_before_the_border_even_beside_a_nested_row() {
    let mut l = side_by_side();
    let found = l.border_at(AREA, 34, 10).expect("the border");
    assert_eq!(
        (found.terminal.clone(), found.columns, found.start),
        (t(0), true, 0)
    );
    l.resize_to(&found.terminal, true, 40, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["40x29", "27x29"]);
    assert_eq!(l.border_at(AREA, 5, 5), None, "inside a terminal");
    assert_eq!(l.border_at(AREA, 40, 29), None, "below the tab");

    // `{A | B}` over C, beside D.
    let mut l = side_by_side();
    l.split(&t(0), t(2), false, AREA).unwrap();
    l.split(&t(0), t(3), true, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["17x14", "16x14", "34x14", "33x29"]);
    let found = l.border_at(AREA, 34, 5).expect("the outer border");
    assert_eq!((found.terminal.clone(), found.start), (t(2), 0));
    l.resize_to(&found.terminal, true, 40, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["20x14", "19x14", "40x14", "27x29"]);
    let found = l.border_at(AREA, 20, 5).expect("A | B");
    assert_eq!((found.terminal.clone(), found.start), (t(0), 0));
    let found = l.border_at(AREA, 10, 14).expect("{A | B} over C");
    assert_eq!((found.terminal.clone(), found.columns), (t(3), false));
    l.resize_to(&found.terminal, false, 19, AREA).unwrap();
    assert_eq!(sizes(&l, AREA), ["20x19", "19x19", "40x9", "27x29"]);
    assert_fills(&l, AREA);
}

#[test]
fn the_tab_resized_keeps_each_cells_share() {
    let mut l = side_by_side();
    l.split(&t(1), t(2), false, AREA).unwrap();
    let wide = Rect::new(0, 0, 137, 59);
    assert_eq!(sizes(&l, wide), ["69x59", "67x29", "67x29"]);
    assert_fills(&l, wide);
    // Back at the first size: the same sizes again.
    assert_eq!(sizes(&l, AREA), ["34x29", "33x14", "33x14"]);
    // Too small for the shares: still one cell each.
    let tiny = Rect::new(0, 0, 3, 3);
    assert_eq!(sizes(&l, tiny), ["1x3", "1x1", "1x1"]);
    assert_fills(&l, tiny);
    // A change made at another size keeps what that size drew.
    l.resize(&t(0), Direction::Right, 1, wide).unwrap();
    assert_eq!(sizes(&l, wide), ["70x59", "66x29", "66x29"]);
}

#[test]
fn focus_moves_across_the_border_to_the_terminal_focused_last() {
    // `%0 | (%1 over %2)`.
    let mut l = side_by_side();
    l.split(&t(1), t(2), false, AREA).unwrap();
    let none = |_: &TerminalId| 0;
    assert_eq!(l.neighbour(&t(1), Direction::Down, AREA, none), Some(t(2)));
    assert_eq!(l.neighbour(&t(2), Direction::Up, AREA, none), Some(t(1)));
    assert_eq!(l.neighbour(&t(2), Direction::Left, AREA, none), Some(t(0)));
    // From %0, both on the right touch it: the first, or the one ranked.
    assert_eq!(l.neighbour(&t(0), Direction::Right, AREA, none), Some(t(1)));
    let two_last = |t: &TerminalId| u64::from(t.as_str() == "%2");
    assert_eq!(
        l.neighbour(&t(0), Direction::Right, AREA, two_last),
        Some(t(2))
    );
    // Past the edge, from the other side, as tmux does.
    assert_eq!(l.neighbour(&t(0), Direction::Left, AREA, none), Some(t(1)));
    assert_eq!(l.neighbour(&t(0), Direction::Up, AREA, none), None);
    assert_eq!(l.neighbour(&t(9), Direction::Up, AREA, none), None);
}
