//! The dashboard list as a table: one cell per column on every line, so
//! values line up, and columns dropped whole when the list is too narrow
//! (the PR column shortened first).

use crate::app::{Row, TaskRow};
use crate::git::{age, Details};
use crate::github::Pr;
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
    /// The branch's pull request (`#123 open ✓ approved`), else `merged`
    /// once git finds it in its base branch.
    Pr,
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
    Column::Pr,
    Column::Agent,
];
/// What a column shows and how.
struct Spec {
    /// The column's header.
    header: &'static str,
    /// Whether values sit at the column's right edge: numbers do.
    right_aligned: bool,
    /// Its width at most.
    cap: usize,
}
/// Each column's [`Spec`], in [`COLUMNS`]' order.
const SPECS: [Spec; COUNT] = [
    Spec {
        header: "NAME",
        right_aligned: false,
        cap: CELL_CAP,
    },
    Spec {
        header: "STATE",
        right_aligned: false,
        cap: CELL_CAP,
    },
    Spec {
        header: "↑↓",
        right_aligned: true,
        cap: CELL_CAP,
    },
    Spec {
        header: "CHANGES",
        right_aligned: true,
        cap: CELL_CAP,
    },
    Spec {
        header: "BASE",
        right_aligned: true,
        cap: CELL_CAP,
    },
    Spec {
        header: "FILES",
        right_aligned: true,
        cap: CELL_CAP,
    },
    Spec {
        header: "AGE",
        right_aligned: true,
        cap: CELL_CAP,
    },
    Spec {
        header: "PR",
        right_aligned: false,
        cap: PR_CAP,
    },
    Spec {
        header: "AGENT",
        right_aligned: false,
        cap: CELL_CAP,
    },
];
/// A step taken to make room for the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The column goes.
    Drop(Column),
    /// The PR column shows only the number and the checks (`#123 ✓`).
    ShortenPr,
}

/// The steps taken, in this order, while the name does not fit. Name and
/// state always stay; the agent, last on the line, goes first; the PR
/// column shortens before it goes.
const NARROWING: [Step; 8] = [
    Step::Drop(Column::Agent),
    Step::ShortenPr,
    Step::Drop(Column::Pr),
    Step::Drop(Column::Files),
    Step::Drop(Column::Base),
    Step::Drop(Column::AheadBehind),
    Step::Drop(Column::Age),
    Step::Drop(Column::Changes),
];
/// Spaces between two columns.
const GAP: usize = 2;
/// A space before the first column and after the last.
const MARGIN: usize = 1;
/// The name column's width wanted before more columns are dropped.
const NAME_WANTED: usize = 32;
/// Every other column's width at most, but the PR column's.
const CELL_CAP: usize = 12;
/// The PR column's width at most: `#12345 closed ✗ changes requested`.
const PR_CAP: usize = 34;
/// How a task's name sits under its session's.
const TASK_INDENT: &str = "  ";
/// Marks text cut to fit.
const ELLIPSIS: char = '…';
/// The PR column's value for a branch without a PR that git finds in its
/// base branch.
const MERGED: &str = "merged";

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

/// Each column's width (`None` once dropped), and whether the PR column
/// shows its short form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Widths {
    columns: [Option<usize>; COUNT],
    short_pr: bool,
}

/// A PR cell's text: `#123 open ✓ approved`, a check mark and review only
/// when there are some.
fn pr_text(pr: &Pr) -> String {
    let words = [
        Some(format!("#{}", pr.number)),
        Some(pr.state.label().to_string()),
        pr.checks.map(|c| c.mark().to_string()),
        pr.review.map(|r| r.label().to_string()),
    ];
    words.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// A PR cell's short form: its number and check mark (`#123 ✓`); `merged`
/// (no PR) as it is.
fn shortened(cell: &str) -> String {
    if !cell.starts_with('#') {
        return cell.to_string();
    }
    cell.split(' ')
        .enumerate()
        .filter(|&(i, w)| i == 0 || ["✓", "✗", "•"].contains(&w))
        .map(|(_, w)| w)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the PR number of a line laid out in `widths` is: its first cell
/// and its width (`#123`), if the column shows and `cells` has a PR.
pub fn pr_number_at(cells: &Cells, widths: &Widths) -> Option<(usize, usize)> {
    let cell = &cells[Column::Pr.index()];
    let number = cell.split(' ').next().filter(|w| w.starts_with('#'))?;
    let width = widths.columns[Column::Pr.index()]?;
    let x = MARGIN
        + COLUMNS
            .iter()
            .take_while(|&&c| c != Column::Pr)
            .filter_map(|c| widths.columns[c.index()])
            .map(|w| w + GAP)
            .sum::<usize>();
    Some((x, number.width().min(width)))
}

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
/// its branch), a subagent's state, its git fields and its pull request
/// (else `merged` if git finds it in its base); what is zero or unknown is
/// left empty.
pub fn task(task: &TaskRow, details: Option<&Details>, pr: Option<&Pr>, now: u64) -> Cells {
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
    cells[Column::Pr.index()] = match pr {
        Some(pr) => pr_text(pr),
        None if details.is_some_and(|d| d.merged) => MERGED.to_string(),
        None => String::new(),
    };
    cells
}

/// `cells`' text in column `c`, in its short form if `short_pr`.
fn text_of(cells: &Cells, c: Column, short_pr: bool) -> String {
    let text = &cells[c.index()];
    if c == Column::Pr && short_pr {
        shortened(text)
    } else {
        text.clone()
    }
}

/// The columns' widths for `lines` (the header included) in `width`
/// terminal cells (two for a wide character: CJK, most emoji): each fits
/// its widest value, at most its cap ([`CELL_CAP`], the PR column's
/// [`PR_CAP`]), the name the rest. The [`NARROWING`] steps are taken until
/// the name has [`NAME_WANTED`] cells, or all it needs.
pub fn widths(lines: &[&Cells], width: usize) -> Widths {
    let widest = |c: Column, short: bool| {
        lines
            .iter()
            .map(|l| text_of(l, c, short).width())
            .max()
            .unwrap_or(0)
    };
    let name_wanted = widest(Column::Name, false).min(NAME_WANTED);
    let laid = |taken: &[Step]| {
        let short = taken.contains(&Step::ShortenPr);
        let columns = COLUMNS
            .map(|c| (!taken.contains(&Step::Drop(c))).then(|| widest(c, short).min(c.spec().cap)));
        let others: usize = COLUMNS
            .iter()
            .filter(|&&c| c != Column::Name)
            .filter_map(|c| columns[c.index()])
            .map(|w| w + GAP)
            .sum();
        (columns, short, width.saturating_sub(2 * MARGIN + others))
    };
    let taken = (0..=NARROWING.len())
        .map(|n| &NARROWING[..n])
        .find(|t| laid(t).2 >= name_wanted)
        .unwrap_or(&NARROWING);
    let (mut columns, short_pr, room) = laid(taken);
    columns[Column::Name.index()] = Some(widest(Column::Name, false).min(room));
    Widths { columns, short_pr }
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
            widths.columns[c.index()].map(|w| {
                fit(
                    &text_of(cells, c, widths.short_pr),
                    w,
                    c.spec().right_aligned,
                )
            })
        })
        .collect();
    let text = format!("{}{}", " ".repeat(MARGIN), shown.join(&" ".repeat(GAP)));
    fit(&text, width, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{BaseDiff, Status};
    use crate::github::{Checks, PrState, Review};
    use crate::sessions::{SubagentTask, TaskState};

    fn pr(number: u64, state: PrState, checks: Option<Checks>, review: Option<Review>) -> Pr {
        Pr {
            number,
            state,
            checks,
            review,
            url: String::new(),
        }
    }

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
            task(&t, Some(&details), None, 1_000 + 7_200),
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
            task(&task_row("b", None), Some(&none), None, 0),
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
            task(&sub, None, None, 0),
            ["  Fix c · c", "done", "", "", "", "", "", "", ""].map(String::from)
        );
    }

    #[test]
    fn a_pr_cell_shows_number_state_checks_and_review_over_merged() {
        let merged = Details {
            merged: true,
            ..Details::default()
        };
        let t = task_row("feat/a", None);
        let cell = |details: Option<&Details>, p: Option<&Pr>| {
            task(&t, details, p, 0)[Column::Pr.index()].clone()
        };
        let open = pr(
            123,
            PrState::Open,
            Some(Checks::Passing),
            Some(Review::Approved),
        );
        assert_eq!(cell(None, Some(&open)), "#123 open ✓ approved");
        assert_eq!(
            cell(Some(&merged), Some(&open)),
            "#123 open ✓ approved",
            "the PR wins"
        );
        assert_eq!(cell(Some(&merged), None), "merged", "no PR: git's word");
        assert_eq!(cell(None, None), "");
        let cases = [
            (
                pr(7, PrState::Draft, Some(Checks::Pending), None),
                "#7 draft •",
            ),
            (
                pr(
                    8,
                    PrState::Closed,
                    Some(Checks::Failing),
                    Some(Review::ChangesRequested),
                ),
                "#8 closed ✗ changes requested",
            ),
            (
                pr(9, PrState::Merged, None, Some(Review::Required)),
                "#9 merged review required",
            ),
            (pr(10, PrState::Open, None, None), "#10 open"),
        ];
        for (p, want) in cases {
            assert_eq!(cell(None, Some(&p)), want);
        }
        assert_eq!(shortened("#123 open ✓ approved"), "#123 ✓");
        assert_eq!(shortened("#8 closed ✗ changes requested"), "#8 ✗");
        assert_eq!(shortened("#9 merged review required"), "#9");
        assert_eq!(shortened("merged"), "merged");
        assert_eq!(shortened(""), "");
    }

    #[test]
    fn a_narrow_list_shortens_the_pr_cell_before_dropping_it() {
        let head = header();
        let name = "a-session-name-of-32-cells-wide!";
        let session = cells(
            name,
            &[(Column::State, "waiting"), (Column::Agent, "claude")],
        );
        let t = cells(
            "  feat/a",
            &[
                (Column::Changes, "+3 −1"),
                (Column::Pr, "#123 open ✓ changes requested"),
            ],
        );
        let lines = [&head, &session, &t];
        let laid = |width| {
            let w = widths(&lines, width);
            let shown: Vec<&str> = COLUMNS
                .iter()
                .filter(|c| w.columns[c.index()].is_some())
                .map(|c| c.spec().header)
                .collect();
            let pr_line = line(&t, &w, width);
            (
                shown,
                w.short_pr,
                pr_line
                    .split_whitespace()
                    .skip(3)
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        };
        let (all, short, text) = laid(120);
        assert_eq!(all.len(), COUNT);
        assert!(!short);
        assert_eq!(text, "#123 open ✓ changes requested");
        let (shown, short, text) = laid(90);
        assert!(!shown.contains(&"AGENT"), "the agent goes first");
        assert!(shown.contains(&"PR"));
        assert!(short, "then the PR cell shortens");
        assert_eq!(text, "#123 ✓");
        let (shown, _, _) = laid(76);
        assert!(!shown.contains(&"PR"), "then it goes");
        assert!(shown.contains(&"FILES"));
        let w = widths(&lines, 90);
        let at = pr_number_at(&t, &w).expect("the number shows");
        let number: String = line(&t, &w, 90).chars().skip(at.0).take(at.1).collect();
        assert_eq!(number, "#123", "one cell a character here");
        assert_eq!(pr_number_at(&session, &w), None, "no PR on a session line");
        assert_eq!(pr_number_at(&t, &widths(&lines, 76)), None, "dropped");
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
                " NAME      STATE  ↑↓   CHANGES  BASE  FILES  AGE  PR  AGENT",
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
                .filter(|c| w.columns[c.index()].is_some())
                .map(|c| c.spec().header)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(100).len(), COUNT, "all fit");
        assert_eq!(
            shown(70),
            ["NAME", "STATE", "↑↓", "CHANGES", "BASE", "FILES", "AGE", "PR"],
            "the agent goes first"
        );
        assert_eq!(
            shown(66),
            ["NAME", "STATE", "↑↓", "CHANGES", "BASE", "FILES", "AGE"],
            "then the PR"
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
