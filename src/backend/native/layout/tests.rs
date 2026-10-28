use super::*;

/// A tab of 68x29, as the test suite's paddock shows one.
const SIZE: (u16, u16) = (68, 29);
const COLUMNS: Axis = Axis::Columns;
const ROWS: Axis = Axis::Rows;

fn t(n: u32) -> TerminalId {
    TerminalId::new(format!("%{n}"))
}

/// Every terminal's size, `WxH`, in the tree's order.
fn sizes(layout: &Layout, size: (u16, u16)) -> Vec<String> {
    let placed = layout.place(size);
    placed
        .iter()
        .map(|(_, r)| format!("{}x{}", r.width, r.height))
        .collect()
}

/// Every cell's place adds up: the cells and the borders between them
/// fill each row and column, every terminal has at least one cell, and
/// all of them lie in the area laid out (the tab, or its smallest size).
fn assert_fills(layout: &Layout, size: (u16, u16)) {
    fn check(node: &Node, area: Rect) {
        if let Node::Split { axis, cells } = node {
            let (rects, _) = cell_rects(cells, *axis, area);
            let used: u32 = rects.iter().map(|r| u32::from(axis.len(*r))).sum();
            let borders = u32::try_from(cells.len() - 1).unwrap();
            assert_eq!(used + borders, u32::from(axis.len(area)), "{node:?}");
            for (cell, rect) in cells.iter().zip(rects) {
                check(&cell.node, rect);
            }
        }
    }
    let area = layout.area(size);
    check(&layout.root, area);
    for (t, r) in layout.place(size) {
        assert!(r.width >= 1 && r.height >= 1, "{t} is {r:?}");
        assert_eq!(r.intersection(area), r, "{t} is {r:?}, outside {area:?}");
    }
}

/// `%0 | %1`: one side-by-side split of the whole tab.
fn side_by_side() -> Layout {
    let mut l = Layout::new(t(0));
    l.split(&t(0), t(1), COLUMNS, SIZE).unwrap();
    l
}

/// `%0 | (%1 over (%2 | (%3 over %4)))`: splits nested four deep.
fn deep() -> Layout {
    let mut l = side_by_side();
    l.split(&t(1), t(2), ROWS, SIZE).unwrap();
    l.split(&t(2), t(3), COLUMNS, SIZE).unwrap();
    l.split(&t(3), t(4), ROWS, SIZE).unwrap();
    l
}

#[test]
fn a_split_gives_the_second_half_to_the_new_terminal_with_a_border_between() {
    let l = side_by_side();
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x29"]);
    let mut rows = Layout::new(t(0));
    rows.split(&t(0), t(1), ROWS, SIZE).unwrap();
    assert_eq!(sizes(&rows, SIZE), ["68x14", "68x14"]);
    let placed = rows.place(SIZE);
    assert_eq!(placed[1], (t(1), Rect::new(0, 15, 68, 14)));
    assert_fills(&rows, SIZE);
}

#[test]
fn splitting_the_way_a_row_runs_adds_to_that_row() {
    let mut l = side_by_side();
    l.split(&t(1), t(2), COLUMNS, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["34x29", "16x29", "16x29"]);
    assert!(matches!(&l.root, Node::Split { cells, .. } if cells.len() == 3));
    // Across it: a column inside the row, in the tree's order.
    l.split(&t(0), t(3), ROWS, SIZE).unwrap();
    assert_eq!(l.terminals(), [t(0), t(3), t(1), t(2)]);
    assert_eq!(sizes(&l, SIZE), ["34x14", "34x14", "16x29", "16x29"]);
    assert_fills(&l, SIZE);
}

#[test]
fn a_cell_too_small_to_split_is_an_error() {
    let mut l = Layout::new(t(0));
    assert!(l.split(&t(0), t(1), COLUMNS, (2, 5)).is_err());
    assert_eq!(l, Layout::new(t(0)), "unchanged");
    assert!(
        l.split(&t(9), t(1), COLUMNS, SIZE).is_err(),
        "no such terminal"
    );
}

#[test]
fn a_closed_cell_goes_to_the_one_before_it_or_after_it_for_the_first() {
    let mut l = side_by_side();
    l.split(&t(1), t(2), COLUMNS, SIZE).unwrap();
    assert!(l.remove(&t(2), SIZE).unwrap());
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x29"]);
    assert!(l.remove(&t(0), SIZE).unwrap());
    assert_eq!(sizes(&l, SIZE), ["68x29"]);
    assert_eq!(
        l,
        Layout::new(t(1)),
        "the row is gone with its last but one"
    );
    assert!(!l.remove(&t(1), SIZE).unwrap(), "the last terminal stays");
}

#[test]
fn a_column_left_with_one_cell_joins_the_row_around_it() {
    // `%0 | (%1 over %2)`, then `%2` closes: `%0 | %1`, one row again.
    let mut l = side_by_side();
    l.split(&t(1), t(2), ROWS, SIZE).unwrap();
    l.split(&t(1), t(3), COLUMNS, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["34x29", "16x14", "16x14", "33x14"]);
    l.remove(&t(2), SIZE).unwrap();
    // `%1 | %3` replaced the column, and joined the outer row.
    assert!(matches!(&l.root, Node::Split { cells, .. } if cells.len() == 3));
    assert_eq!(sizes(&l, SIZE), ["34x29", "16x29", "16x29"]);
    assert_fills(&l, SIZE);
}

#[test]
fn resize_keys_move_the_border_after_the_cell_or_before_the_last() {
    let mut l = side_by_side();
    // The right one is last: its left border moves.
    l.resize(&t(1), Direction::Left, 2, SIZE).unwrap();
    l.resize(&t(1), Direction::Left, 2, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["30x29", "37x29"]);
    l.resize(&t(0), Direction::Right, 4, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x29"]);
    let mut rows = Layout::new(t(0));
    rows.split(&t(0), t(1), ROWS, SIZE).unwrap();
    rows.resize(&t(1), Direction::Up, 4, SIZE).unwrap();
    assert_eq!(sizes(&rows, SIZE), ["68x10", "68x18"]);
    rows.resize(&t(1), Direction::Down, 2, SIZE).unwrap();
    assert_eq!(sizes(&rows, SIZE), ["68x12", "68x16"]);
    // No column to move a border in: nothing happens.
    rows.resize(&t(1), Direction::Left, 2, SIZE).unwrap();
    assert_eq!(sizes(&rows, SIZE), ["68x12", "68x16"]);
}

#[test]
fn a_resize_stops_at_one_cell() {
    let mut l = side_by_side();
    for _ in 0..25 {
        l.resize(&t(1), Direction::Left, 2, SIZE).unwrap();
    }
    assert_eq!(sizes(&l, SIZE), ["1x29", "66x29"]);
    for _ in 0..40 {
        l.resize(&t(0), Direction::Right, 2, SIZE).unwrap();
    }
    assert_eq!(sizes(&l, SIZE), ["66x29", "1x29"]);
    assert_fills(&l, SIZE);
}

#[test]
fn many_resizes_in_a_row_land_exactly_where_their_sum_does() {
    let mut l = deep();
    let start = sizes(&l, SIZE);
    for _ in 0..50 {
        l.resize(&t(3), Direction::Left, 1, SIZE).unwrap();
        l.resize(&t(3), Direction::Right, 1, SIZE).unwrap();
        l.resize(&t(4), Direction::Up, 1, SIZE).unwrap();
        l.resize(&t(4), Direction::Down, 1, SIZE).unwrap();
    }
    assert_eq!(sizes(&l, SIZE), start, "no drift");
    for _ in 0..3 {
        l.resize(&t(0), Direction::Right, 1, SIZE).unwrap();
    }
    assert_eq!(sizes(&l, SIZE)[0], "37x29", "three cells, one at a time");
    assert_fills(&l, SIZE);
}

#[test]
fn equal_spreads_the_nearest_uneven_row_the_first_cells_taking_the_rest() {
    let mut l = side_by_side();
    l.resize(&t(1), Direction::Left, 6, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["28x29", "39x29"]);
    l.spread(&t(1), SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x29"]);
    l.split(&t(1), t(2), COLUMNS, SIZE).unwrap();
    l.spread(&t(2), SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["22x29", "22x29", "22x29"]);
    // An even column inside the row: the row around it evens out.
    let mut l = side_by_side();
    l.split(&t(1), t(2), ROWS, SIZE).unwrap();
    l.resize(&t(1), Direction::Left, 6, SIZE).unwrap();
    l.spread(&t(2), SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x14", "33x14"]);
}

#[test]
fn equal_climbs_past_a_row_a_smallest_size_keeps_from_changing() {
    // `%0 | (%1 over (%2 | (%3 over %4 over %5)))`, made at 40x20, shown
    // 8 rows high: the inner column needs 5 rows (three terminals, two
    // borders) under %1, so evening out the column around %1 cannot
    // change what is drawn. `=` climbs to the uneven row around it.
    let made = (40, 20);
    let mut l = Layout::new(t(0));
    l.split(&t(0), t(1), COLUMNS, made).unwrap();
    l.split(&t(1), t(2), ROWS, made).unwrap();
    l.split(&t(2), t(3), COLUMNS, made).unwrap();
    l.split(&t(3), t(4), ROWS, made).unwrap();
    l.split(&t(4), t(5), ROWS, made).unwrap();
    let size = (40, 8);
    l.resize(&t(0), Direction::Left, 6, size).unwrap();
    let before = sizes(&l, size);
    assert_eq!(before[..2], ["14x8", "25x2"]);
    l.spread(&t(1), size).unwrap();
    let after = sizes(&l, size);
    assert_eq!(after[..2], ["20x8", "19x2"], "the outer row evened");
    assert_fills(&l, size);
    // At full height again, the column kept its share.
    assert_eq!(sizes(&l, made)[1], "19x10");
}

#[test]
fn a_drag_sizes_the_cell_before_the_border_even_beside_a_nested_row() {
    let mut l = side_by_side();
    let hit = l.border_at(SIZE, 34, 10).expect("the border");
    assert_eq!(
        (hit.terminal.clone(), hit.axis, hit.start),
        (t(0), COLUMNS, 0)
    );
    l.resize_to(&hit.terminal, COLUMNS, 40, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["40x29", "27x29"]);
    assert_eq!(l.border_at(SIZE, 5, 5), None, "inside a terminal");
    assert_eq!(l.border_at(SIZE, 40, 29), None, "below the tab");
    assert_eq!(l.border_at(SIZE, u16::MAX, u16::MAX), None, "far away");

    // `{A | B}` over C, beside D.
    let mut l = side_by_side();
    l.split(&t(0), t(2), ROWS, SIZE).unwrap();
    l.split(&t(0), t(3), COLUMNS, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["17x14", "16x14", "34x14", "33x29"]);
    let hit = l.border_at(SIZE, 34, 5).expect("the outer border");
    assert_eq!((hit.terminal.clone(), hit.start), (t(2), 0));
    l.resize_to(&hit.terminal, COLUMNS, 40, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["20x14", "19x14", "40x14", "27x29"]);
    let hit = l.border_at(SIZE, 20, 5).expect("A | B");
    assert_eq!((hit.terminal.clone(), hit.start), (t(0), 0));
    let hit = l.border_at(SIZE, 10, 14).expect("{A | B} over C");
    assert_eq!((hit.terminal.clone(), hit.axis), (t(3), ROWS));
    l.resize_to(&hit.terminal, ROWS, 19, SIZE).unwrap();
    assert_eq!(sizes(&l, SIZE), ["20x19", "19x19", "40x9", "27x29"]);
    assert_fills(&l, SIZE);
}

#[test]
fn the_tab_resized_keeps_each_cells_share() {
    let mut l = side_by_side();
    l.split(&t(1), t(2), ROWS, SIZE).unwrap();
    let wide = (137, 59);
    assert_eq!(sizes(&l, wide), ["69x59", "67x29", "67x29"]);
    assert_fills(&l, wide);
    // Back at the first size: the same sizes again.
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x14", "33x14"]);
    // A change made at another size keeps what that size drew.
    l.resize(&t(0), Direction::Right, 1, wide).unwrap();
    assert_eq!(sizes(&l, wide), ["70x59", "66x29", "66x29"]);
}

#[test]
fn a_tab_smaller_than_its_splits_lays_them_out_at_their_smallest() {
    let l = deep();
    for tiny in [(0, 0), (1, 1), (3, 3), (4, 2), (68, 1), (1, 29)] {
        let placed = l.place(tiny);
        assert_eq!(placed.len(), 5, "{tiny:?}");
        assert_fills(&l, tiny);
    }
    // One cell each and a border between, along each axis.
    let size = l.area((0, 0));
    assert_eq!((size.width, size.height), (5, 5));
    assert_eq!(sizes(&l, (0, 0)), ["1x5", "3x1", "1x3", "1x1", "1x1"]);
}

#[test]
fn shares_come_back_when_a_tab_squeezed_to_its_smallest_grows_again() {
    let mut l = deep();
    l.resize(&t(0), Direction::Right, 7, SIZE).unwrap();
    let before = sizes(&l, SIZE);
    // Changes made while squeezed keep the shares they could not draw.
    l.resize(&t(3), Direction::Left, 1, (4, 3)).unwrap();
    l.spread(&t(4), (4, 3)).unwrap();
    assert_fills(&l, (4, 3));
    assert_eq!(sizes(&l, SIZE), before);
    // A terminal closed while squeezed: the others keep their shares.
    l.remove(&t(4), (5, 3)).unwrap();
    assert_eq!(sizes(&l, SIZE)[..2], before[..2]);
    assert_fills(&l, SIZE);
}

#[test]
fn deep_nesting_lays_out_every_level_and_closes_back_up() {
    let mut l = deep();
    assert_eq!(l.terminals(), [t(0), t(1), t(2), t(3), t(4)]);
    assert_eq!(sizes(&l, SIZE), ["34x29", "33x14", "16x14", "16x7", "16x6"]);
    assert_fills(&l, SIZE);
    for gone in [t(3), t(2), t(4), t(1)] {
        assert!(l.remove(&gone, SIZE).unwrap());
        assert_fills(&l, SIZE);
    }
    assert_eq!(l, Layout::new(t(0)));
}

#[test]
fn focus_moves_across_the_border_to_the_terminal_focused_last() {
    // `%0 | (%1 over %2)`.
    let mut l = side_by_side();
    l.split(&t(1), t(2), ROWS, SIZE).unwrap();
    let none = |_: &TerminalId| 0;
    assert_eq!(l.neighbour(&t(1), Direction::Down, SIZE, none), Some(t(2)));
    assert_eq!(l.neighbour(&t(2), Direction::Up, SIZE, none), Some(t(1)));
    assert_eq!(l.neighbour(&t(2), Direction::Left, SIZE, none), Some(t(0)));
    // From %0, both on the right touch it: the first, or the one ranked.
    assert_eq!(l.neighbour(&t(0), Direction::Right, SIZE, none), Some(t(1)));
    let two_last = |t: &TerminalId| u64::from(t.as_str() == "%2");
    assert_eq!(
        l.neighbour(&t(0), Direction::Right, SIZE, two_last),
        Some(t(2))
    );
    // Past the edge, from the other side, as tmux does.
    assert_eq!(l.neighbour(&t(0), Direction::Left, SIZE, none), Some(t(1)));
    assert_eq!(l.neighbour(&t(0), Direction::Up, SIZE, none), None);
    assert_eq!(l.neighbour(&t(9), Direction::Up, SIZE, none), None);
    // At the largest size there is: no overflow.
    let max = (u16::MAX, u16::MAX);
    assert_eq!(l.neighbour(&t(1), Direction::Right, max, none), Some(t(0)));
}
