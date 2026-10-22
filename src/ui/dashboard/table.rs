//! The dashboard list as a table: one cell per column on every line, so
//! values line up, and columns dropped whole when the list is too narrow.

use crate::app::{Row, TaskRow};
use crate::git::{age, Details};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The columns, left to right: declared in [`COLUMNS`]' order, which
/// [`Column::index`] and [`SPECS`] rely on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    /// The session's label, or a task's indented under it.
    Name,
    /// A session's or a subagent's state.
    State,
    /// Commits ahead of and behind upstream.
    AheadBehind,
    /// Uncommitted lines added and removed.
    Changes,
    /// Lines added and removed against the base branch.
    Base,
    /// Files changed against the base branch.
    Files,
    /// The last commit's age.
    Age,
    /// Merged into the base branch.
    Merged,
    /// A session's agent kind, as a tag (`claude`).
    Agent,
}

/// How many columns there are.
const COUNT: usize = 9;
/// Every column, left to right.
const COLUMNS: [Column; COUNT] = [
    Column::Name,
    Column::State,
    Column::AheadBehind,
    Column::Changes,
    Column::Base,
    Column::Files,
    Column::Age,
    Column::Merged,
    Column::Agent,
];
/// What a column shows and how.
struct Spec {
    /// The column's header.
    header: &'static str,
    /// Whether values sit at the column's right edge: numbers do.
    right_aligned: bool,
}
/// Each column's [`Spec`], in [`COLUMNS`]' order.
const SPECS: [Spec; COUNT] = [
    Spec {
        header: "NAME",
        right_aligned: false,
    },
    Spec {
        header: "STATE",
        right_aligned: false,
    },
    Spec {
        header: "↑↓",
        right_aligned: true,
    },
    Spec {
        header: "CHANGES",
        right_aligned: true,
    },
    Spec {
        header: "BASE",
        right_aligned: true,
    },
    Spec {
        header: "FILES",
        right_aligned: true,
    },
    Spec {
        header: "AGE",
        right_aligned: true,
    },
    Spec {
        header: "MERGED",
        right_aligned: false,
    },
    Spec {
        header: "AGENT",
        right_aligned: false,
    },
];
/// The columns dropped, in this order, while the name does not fit. Name
/// and state always stay; the agent, last on the line, goes first.
const DROP_ORDER: [Column; 7] = [
    Column::Agent,
    Column::Merged,
    Column::Files,
    Column::Base,
    Column::AheadBehind,
    Column::Age,
    Column::Changes,
];
/// Spaces between two columns.
const GAP: usize = 2;
/// A space before the first column and after the last.
const MARGIN: usize = 1;
/// The name column's width wanted before more columns are dropped.
const NAME_WANTED: usize = 32;
/// Every other column's width at most.
const CELL_CAP: usize = 12;
/// How a task's name sits under its session's.
const TASK_INDENT: &str = "  ";
/// Marks text cut to fit.
const ELLIPSIS: char = '…';
/// The merged column's value for a merged branch.
const MERGED: &str = "✓";

impl Column {
    /// The column's place, left to right.
    fn index(self) -> usize {
        self as usize
    }

    /// The column's header and alignment.
    fn spec(self) -> &'static Spec {
        &SPECS[self.index()]
    }
}

/// One line's cells, a column each; empty when unknown.
pub type Cells = [String; COUNT];

/// Each column's width; `None` once dropped.
pub type Widths = [Option<usize>; COUNT];

/// The header line's cells.
pub fn header() -> Cells {
    COLUMNS.map(|c| c.spec().header.to_string())
}

/// A session row's cells: its label, state and agent.
pub fn session(row: &Row) -> Cells {
    let mut cells = Cells::default();
    cells[Column::Name.index()] = row.label.clone();
    cells[Column::State.index()] = row.state.label().to_string();
    cells[Column::Agent.index()] = row.kind.tag().to_string();
    cells
}

/// A task row's cells: its name indented (a subagent's description before
/// its branch), a subagent's state, and its git fields; what is zero or
/// unknown is left empty.
pub fn task(task: &TaskRow, details: Option<&Details>, now: u64) -> Cells {
    let status = task.status.unwrap_or_default();
    let name = match task.subagent.as_ref().and_then(|s| s.description.as_ref()) {
        Some(d) => format!("{d} · {}", task.branch),
        None => task.branch.clone(),
    };
    let arrows: String = status
        .ahead_behind
        .map(|(ahead, behind)| [("↑", ahead), ("↓", behind)])
        .into_iter()
        .flatten()
        .filter(|&(_, n)| n > 0)
        .map(|(arrow, n)| format!("{arrow}{n}"))
        .collect();
    let changes = status
        .changes
        .filter(|&c| c != (0, 0))
        .map(|(added, removed)| format!("+{added} −{removed}"));
    let base = details.and_then(|d| d.base_diff).filter(|b| b.files > 0);
    let commit = details
        .and_then(|d| d.last_commit)
        .map(|t| age(now.saturating_sub(t)));
    let mut cells = Cells::default();
    cells[Column::Name.index()] = format!("{TASK_INDENT}{name}");
    cells[Column::State.index()] = task
        .subagent
        .as_ref()
        .map(|s| s.state.label().to_string())
        .unwrap_or_default();
    cells[Column::AheadBehind.index()] = arrows;
    cells[Column::Changes.index()] = changes.unwrap_or_default();
    if let Some(b) = base {
        cells[Column::Base.index()] = format!("+{} −{}", b.added, b.removed);
        cells[Column::Files.index()] = b.files.to_string();
    }
    cells[Column::Age.index()] = commit.unwrap_or_default();
    if details.is_some_and(|d| d.merged) {
        cells[Column::Merged.index()] = MERGED.to_string();
    }
    cells
}

/// The columns' widths for `lines` (the header included) in `width`
/// terminal cells (two for a wide character: CJK, most emoji): each fits its widest value, at most [`CELL_CAP`], the name the
/// rest. Columns go in [`DROP_ORDER`] until the name has
/// [`NAME_WANTED`] cells, or all it needs.
pub fn widths(lines: &[&Cells], width: usize) -> Widths {
    let widest = |c: Column| {
        lines
            .iter()
            .map(|l| l[c.index()].width())
            .max()
            .unwrap_or(0)
    };
    let fitted = COLUMNS.map(|c| widest(c).min(CELL_CAP));
    let name_wanted = widest(Column::Name).min(NAME_WANTED);
    let room = |dropped: &[Column]| {
        let others: usize = COLUMNS
            .iter()
            .filter(|&&c| c != Column::Name && !dropped.contains(&c))
            .map(|&c| fitted[c.index()] + GAP)
            .sum();
        width.saturating_sub(2 * MARGIN + others)
    };
    let dropped = (0..=DROP_ORDER.len())
        .map(|n| &DROP_ORDER[..n])
        .find(|d| room(d) >= name_wanted)
        .unwrap_or(&DROP_ORDER);
    let name = widest(Column::Name).min(room(dropped));
    COLUMNS.map(|c| match c {
        Column::Name => Some(name),
        _ if dropped.contains(&c) => None,
        _ => Some(fitted[c.index()]),
    })
}

/// `text` in exactly `width` cells: cut with [`ELLIPSIS`] if longer, else
/// padded on the left if `right`, on the right otherwise. Text is cut
/// between grapheme clusters, each cut measured as a whole string, so an
/// emoji with VS16 or a ZWJ sequence is kept whole or dropped whole.
fn fit(text: &str, width: usize, right: bool) -> String {
    let n = text.width();
    if n > width {
        if width == 0 {
            return String::new();
        }
        let room = width.saturating_sub(ELLIPSIS.width().unwrap_or(1));
        let end = text
            .grapheme_indices(true)
            .map(|(i, g)| i + g.len())
            .take_while(|&end| text[..end].width() <= room)
            .last()
            .unwrap_or(0);
        let kept = &text[..end];
        // A wide grapheme that did not fit leaves a cell: pad it.
        let pad = " ".repeat(room.saturating_sub(kept.width()));
        return format!("{kept}{ELLIPSIS}{pad}");
    }
    let pad = " ".repeat(width.saturating_sub(n));
    if right {
        format!("{pad}{text}")
    } else {
        format!("{text}{pad}")
    }
}

/// A line of `cells` laid out in `widths`, exactly `width` cells.
pub fn line(cells: &Cells, widths: &Widths, width: usize) -> String {
    let shown: Vec<String> = COLUMNS
        .iter()
        .filter_map(|&c| {
            widths[c.index()].map(|w| fit(&cells[c.index()], w, c.spec().right_aligned))
        })
        .collect();
    let text = format!("{}{}", " ".repeat(MARGIN), shown.join(&" ".repeat(GAP)));
    fit(&text, width, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{BaseDiff, Status};
    use crate::sessions::{SubagentTask, TaskState};

    /// A task row on `branch` with git status `status`.
    fn task_row(branch: &str, status: Option<Status>) -> TaskRow {
        TaskRow {
            branch: branch.into(),
            cwd: "/r".into(),
            status,
            subagent: None,
        }
    }

    #[test]
    fn task_cells_cover_status_base_diff_files_age_and_merged() {
        let status = Status {
            ahead_behind: Some((2, 0)),
            changes: Some((30, 144)),
        };
        let details = Details {
            base_diff: Some(BaseDiff {
                added: 12,
                removed: 3,
                files: 4,
            }),
            last_commit: Some(1_000),
            merged: true,
        };
        let t = task_row("feat/a", Some(status));
        assert_eq!(
            task(&t, Some(&details), 1_000 + 7_200),
            [
                "  feat/a",
                "",
                "↑2",
                "+30 −144",
                "+12 −3",
                "4",
                "2h",
                MERGED,
                ""
            ]
            .map(String::from)
        );
        let none = Details {
            base_diff: Some(BaseDiff {
                added: 0,
                removed: 0,
                files: 0,
            }),
            ..Details::default()
        };
        assert_eq!(
            task(&task_row("b", None), Some(&none), 0),
            ["  b", "", "", "", "", "", "", "", ""].map(String::from),
            "zero and unknown stay empty"
        );
        let sub = TaskRow {
            subagent: Some(SubagentTask {
                agent_id: "a1".into(),
                description: Some("Fix c".into()),
                state: TaskState::Done,
            }),
            ..task_row("c", Some(Status::default()))
        };
        assert_eq!(
            task(&sub, None, 0),
            ["  Fix c · c", "done", "", "", "", "", "", "", ""].map(String::from)
        );
    }

    /// Cells with `name` and the rest empty but for `values`.
    fn cells(name: &str, values: &[(Column, &str)]) -> Cells {
        let mut c = Cells::default();
        c[Column::Name.index()] = name.into();
        for (column, v) in values {
            c[column.index()] = (*v).into();
        }
        c
    }

    #[test]
    fn values_line_up_in_their_columns_numbers_to_the_right() {
        let head = header();
        let a = cells("api", &[(Column::State, "idle")]);
        let b = cells(
            "  feat/a",
            &[(Column::Changes, "+3 −1"), (Column::Files, "2")],
        );
        let c = cells("  feat/b", &[(Column::Changes, "+30 −144")]);
        let lines = [&head, &a, &b, &c];
        let w = widths(&lines, 80);
        let got: Vec<String> = lines
            .iter()
            .map(|l| line(l, &w, 80).trim_end().to_string())
            .collect();
        assert_eq!(
            got,
            [
                " NAME      STATE  ↑↓   CHANGES  BASE  FILES  AGE  MERGED  AGENT",
                " api       idle",
                "   feat/a                +3 −1            2",
                "   feat/b             +30 −144",
            ]
        );
    }

    #[test]
    fn every_column_is_at_its_place_in_columns() {
        assert!(COLUMNS.iter().enumerate().all(|(i, c)| c.index() == i));
    }

    /// Where `needle` starts in `text`, in cells.
    fn cell_at(text: &str, needle: &str) -> usize {
        text[..text.find(needle).expect("in the line")].width()
    }

    #[test]
    fn wide_names_keep_the_columns_aligned() {
        let head = header();
        let cjk = cells("修正バグ", &[(Column::State, "idle")]);
        let emoji = cells("🚀 ship", &[(Column::State, "done")]);
        let plain = cells("api", &[(Column::State, "running")]);
        let lines = [&head, &cjk, &emoji, &plain];
        let w = widths(&lines, 80);
        let laid: Vec<String> = lines.iter().map(|l| line(l, &w, 80)).collect();
        let state = cell_at(&laid[0], "STATE");
        assert_eq!(cell_at(&laid[1], "idle"), state, "CJK");
        assert_eq!(cell_at(&laid[2], "done"), state, "emoji");
        assert_eq!(cell_at(&laid[3], "running"), state);
        assert!(laid.iter().all(|l| l.width() == 80), "every line fills");
    }

    #[test]
    fn wide_text_is_cut_by_cells() {
        assert_eq!(fit("修正バグ", 4, false), "修… ");
        assert_eq!(fit("修正バグ", 5, false), "修正…");
        assert_eq!(fit("修正", 6, true), "  修正");
        assert_eq!("修正バグ".width(), 8);
        // VS16 makes `❤` two cells wide; counting it one character at a
        // time would keep `❤️x` (3 cells) in 2 and underflow.
        assert_eq!(fit("❤️xyz", 3, false), "❤️…");
        assert_eq!(fit("✔️ ship", 4, false), "✔️ …");
        // A ZWJ family is one grapheme: kept whole or not at all.
        assert_eq!(fit("👨‍👩‍👧 home", 4, false), "👨‍👩‍👧 …");
        assert_eq!(fit("👨‍👩‍👧 home", 2, false), "… ");
        for text in ["❤️xyz", "✔️ ship", "👨‍👩‍👧 home", "a❤️b✔️c👨‍👩‍👧d", "修正バグ"]
        {
            for width in 0..=text.width() + 1 {
                let got = fit(text, width, false);
                assert_eq!(got.width(), width, "{text:?} in {width}");
            }
        }
    }

    #[test]
    fn narrow_lists_drop_whole_columns_and_cut_the_name() {
        let head = header();
        let long = cells("a-very-long-session-name", &[(Column::State, "waiting")]);
        let lines = [&head, &long];
        let shown = |width| {
            let w = widths(&lines, width);
            COLUMNS
                .iter()
                .filter(|c| w[c.index()].is_some())
                .map(|c| c.spec().header)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(100).len(), COUNT, "all fit");
        assert_eq!(
            shown(70),
            ["NAME", "STATE", "↑↓", "CHANGES", "BASE", "FILES", "AGE"],
            "the agent, then merged, go first"
        );
        assert_eq!(
            shown(60),
            ["NAME", "STATE", "↑↓", "CHANGES", "BASE", "AGE"],
            "then files"
        );
        assert_eq!(shown(50), ["NAME", "STATE", "CHANGES", "AGE"]);
        assert_eq!(shown(20), ["NAME", "STATE"]);
        let w = widths(&lines, 20);
        assert_eq!(line(&long, &w, 20), " a-very-l…  waiting ");
    }
}
