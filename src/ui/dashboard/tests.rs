use super::*;

const KEYS: &str = "j down  k up";
/// The selection's background in these tests.
const SELECTED: Color = Color::Indexed(236);
use crate::git::{BaseDiff, Details, Status};
use crate::sessions::SubagentTask;

fn row(label: &str, state: State, tasks: Vec<TaskRow>) -> Row {
    Row {
        pane: format!("%{label}"),
        space: "$1".into(),
        window: "@1".into(),
        label: label.into(),
        state,
        tasks,
        external: false,
    }
}

#[test]
fn external_sessions_come_dim_under_their_line_with_no_screen() {
    let area = Rect::new(0, 0, 50, 30);
    let mut buf = Buffer::empty(area);
    let ext = Row {
        pane: "ext:e1".into(),
        space: String::new(),
        window: String::new(),
        external: true,
        ..row("web", State::Waiting, vec![task("feat/x", None)])
    };
    let rows = [row("api", State::Idle, vec![]), ext];
    let selected = RowId {
        pane: "ext:e1".into(),
    };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: Some(&selected),
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    let screen: Vec<String> = (0..30).map(|y| text(&buf, y)).collect();
    let at = |s: &str| screen.iter().position(|l| l.starts_with(s));
    let group = at(" external").unwrap_or_else(|| panic!("{screen:#?}"));
    assert!(at(" api").unwrap() < group, "{screen:#?}");
    let web = at(" web").expect("web");
    assert_eq!(web, group + 1);
    assert!(screen[web + 1].contains("feat/x"));
    let y = u16::try_from(web).unwrap();
    assert!(buf[(1, y)].modifier.contains(Modifier::DIM), "dim");
    let api = u16::try_from(at(" api").unwrap()).unwrap();
    assert!(!buf[(1, api)].modifier.contains(Modifier::DIM));
    let preview = at("╭ web ─").expect("preview titled web");
    assert!(
        screen[preview + 1].contains(NO_SCREEN),
        "{}",
        screen[preview + 1]
    );
}

fn task(branch: &str, status: Option<Status>) -> TaskRow {
    TaskRow {
        branch: branch.into(),
        cwd: "/r".into(),
        status,
        subagent: None,
    }
}

fn text(buf: &Buffer, y: u16) -> String {
    (buf.area.x..buf.area.right())
        .map(|x| buf[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn lines_follow_rows_each_with_its_tasks_and_a_blank_line_between() {
    let rows = [
        row("w", State::Waiting, vec![task("a", None), task("b", None)]),
        row("i", State::Idle, vec![]),
        row("r", State::Running, vec![]),
    ];
    let got: Vec<String> = lines(&rows)
        .into_iter()
        .map(|l| match l {
            ListLine::Blank => String::new(),
            ListLine::Session(r) => r.label.clone(),
            ListLine::Task(_, t) => format!("  {}", t.branch),
            ListLine::External => "external".into(),
        })
        .collect();
    assert_eq!(got, ["w", "  a", "  b", "", "i", "", "r"]);
}

/// A layout of `list`, `preview` and `keys` rows stacked from row 1 of
/// an area 80 columns wide.
fn stacked(list: u16, preview: u16, keys: u16) -> Layout {
    Layout {
        list: Rect::new(0, 1, 80, list),
        preview: Rect::new(0, 1 + list, 80, preview),
        keys: Rect::new(0, 1 + list + preview, 80, keys),
    }
}

#[test]
fn the_list_sits_on_top_sized_to_its_lines_with_the_keys_at_the_bottom() {
    let area = |height| Rect::new(0, 1, 80, height);
    assert_eq!(
        layout(area(40), 7, false),
        stacked(11, 28, 1),
        "the title, the header, seven lines and blank lines around them"
    );
    assert_eq!(
        layout(area(40), 50, false),
        stacked(16, 23, 1),
        "capped at 40%"
    );
    assert_eq!(
        layout(area(40), 1, false),
        stacked(6, 33, 1),
        "a few rows at least"
    );
    assert_eq!(
        layout(area(40), 0, false),
        stacked(6, 33, 1),
        "the line saying there are none"
    );
}

#[test]
fn the_filter_line_takes_a_row_under_the_title() {
    let area = Rect::new(0, 1, 80, 40);
    assert_eq!(layout(area, 7, true), stacked(12, 27, 1));
    let mut buf = Buffer::empty(area);
    let rows = [row("api", State::Idle, vec![])];
    let filter = DashboardFilter {
        line: crate::input::line::LineEdit::new("ap"),
        typing: true,
    };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: None,
        preview: None,
        interactive: false,
        filter: &filter,
        now: 0,
        selection: SELECTED,
    };
    let cursor = render(&view, area, &mut buf);
    assert_eq!(text(&buf, 1), " dashboard");
    assert_eq!(text(&buf, 2), " / ap");
    assert!(text(&buf, 3).starts_with(" NAME"), "the header moved down");
    assert_eq!(text(&buf, 4), "");
    assert!(text(&buf, 5).starts_with(" api"));
    assert_eq!(cursor, Some(Position::new(5, 2)), "typing the filter");
    let kept_filter = DashboardFilter {
        typing: false,
        ..filter.clone()
    };
    let kept = View {
        filter: &kept_filter,
        ..view
    };
    assert_eq!(render(&kept, area, &mut buf), None, "the filter kept");
}

#[test]
fn a_short_dashboard_loses_its_preview_before_its_list() {
    let area = |height| Rect::new(0, 1, 80, height);
    assert_eq!(
        layout(area(10), 9, false),
        stacked(6, 3, 1),
        "the minimum wins"
    );
    assert_eq!(
        layout(area(8), 9, false),
        stacked(7, 0, 1),
        "no room for a bordered terminal line: the list keeps it"
    );
    assert_eq!(layout(area(3), 9, false), stacked(2, 0, 1));
    assert_eq!(
        layout(area(1), 9, false),
        stacked(1, 0, 0),
        "only the title"
    );
    assert_eq!(layout(area(0), 9, false), stacked(0, 0, 0));
}

#[test]
fn the_preview_keeps_the_last_text_in_sight() {
    let mut e = Emulator::new(20, 10, 0);
    e.feed(b"one\r\ntwo\r\n");
    assert_eq!(first_row(&e, 4), 0, "fits: from the top");
    for i in 0..7 {
        e.feed(format!("line {i}\r\n").as_bytes());
    }
    // Text on rows 0..=8, the cursor on row 9.
    assert_eq!(first_row(&e, 4), 6);
    let mut claude = Emulator::new(20, 10, 0);
    claude.feed(b"\x1b[3;1Hprompt\x1b[10;1Hstatus\x1b[3;7H");
    assert_eq!(first_row(&claude, 4), 6, "text below the cursor");
}

#[test]
fn renders_rows_tasks_and_a_live_preview() {
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    let status = Status {
        ahead_behind: None,
        changes: Some((3, 1)),
    };
    let mut sub = task("fix/b", None);
    sub.subagent = Some(SubagentTask {
        agent_id: "a1".into(),
        description: Some("Fix b".into()),
        state: TaskState::Waiting,
    });
    let rows = [row(
        "api",
        State::Idle,
        vec![task("feat/a", Some(status)), sub],
    )];
    let details = TaskDetails::from([(
        ("/r".to_string(), "feat/a".to_string()),
        Details {
            base_diff: Some(BaseDiff {
                added: 5,
                removed: 0,
                files: 2,
            }),
            last_commit: None,
            merged: true,
        },
    )]);
    let mut emu = Emulator::new(20, 4, 0);
    emu.feed(b"hello preview");
    let selected = RowId {
        pane: "%api".into(),
    };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &details,
        selected: Some(&selected),
        preview: Some(&emu),
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    assert_eq!(render(&view, area, &mut buf), None);
    assert_eq!(text(&buf, 0), " dashboard");
    assert_eq!(
        [1, 2, 3, 4, 5, 6].map(|y| text(&buf, y)),
        [
            " NAME             STATE    ↑↓  CHANGES   BASE  FILES  AGE  MERGED",
            "",
            " api              idle",
            "   feat/a                        +3 −1  +5 −0      2       ✓",
            "   Fix b · fix/b  waiting",
            "",
        ]
    );
    assert!(buf[(1, 1)].modifier.contains(Modifier::DIM), "a dim header");
    // The selected session's block, its task rows included, has the
    // selection's background; a waiting row keeps its yellow as text.
    assert_eq!([3, 4, 5].map(|y| buf[(3, y)].bg), [SELECTED; 3]);
    assert_eq!(buf[(3, 3)].fg, Color::Green, "idle keeps its colour");
    assert_eq!(buf[(3, 5)].fg, Color::Yellow, "waiting keeps its colour");
    assert_ne!(buf[(1, 1)].bg, SELECTED, "not the header");
    // The preview in a rounded border, its title in the top border.
    assert!(text(&buf, 7).starts_with("╭ preview: api  I to type ──"));
    assert!(text(&buf, 7).ends_with("─╮"));
    assert_eq!(
        text(&buf, 8),
        "│hello preview".to_string() + &" ".repeat(65) + "│"
    );
    assert!(text(&buf, 18).starts_with("╰──"));
    assert_eq!(text(&buf, 19), " j down  k up", "the keys at the bottom");
    assert!(buf[(1, 19)].modifier.contains(Modifier::DIM));

    let typing = View {
        interactive: true,
        ..view
    };
    let cursor = render(&typing, area, &mut Buffer::empty(area));
    assert_eq!(cursor, Some(Position::new(14, 8)), "inside the border");
}

#[test]
fn only_the_selected_sessions_block_is_highlighted() {
    let area = Rect::new(0, 0, 30, 30);
    let mut buf = Buffer::empty(area);
    let rows = [
        row("r0", State::Running, vec![task("b1", None), subagent("s1")]),
        row("r1", State::Running, vec![]),
    ];
    let selected = RowId { pane: "%r0".into() };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: Some(&selected),
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    assert!(text(&buf, 3).starts_with(" r0"));
    assert!(text(&buf, 7).starts_with(" r1"));
    let bg = |y: u16| buf[(1, y)].bg == SELECTED;
    assert_eq!([3, 4, 5, 6, 7].map(bg), [true, true, true, false, false]);
}

#[test]
fn a_rows_preview_without_its_terminal_says_so() {
    let area = Rect::new(0, 0, 40, 12);
    let mut buf = Buffer::empty(area);
    let rows = [row("gone", State::Idle, vec![])];
    let selected = RowId {
        pane: "%gone".into(),
    };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: Some(&selected),
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    assert!(text(&buf, 6).starts_with("╭ gone: no terminal ─"));
}

#[test]
fn without_rows_it_says_there_are_no_sessions() {
    let area = Rect::new(0, 0, 40, 12);
    let mut buf = Buffer::empty(area);
    let view = View {
        keys: KEYS,
        rows: &[],
        details: &TaskDetails::new(),
        selected: None,
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    assert_eq!(text(&buf, 2), "");
    assert_eq!(text(&buf, 3), " no Claude sessions yet");
    assert_eq!(text(&buf, 6), format!("╭{}╮", "─".repeat(38)));
    assert_eq!(text(&buf, 11), " j down  k up");
}

#[test]
fn the_selected_line_scrolls_into_sight() {
    let area = Rect::new(0, 0, 30, 20);
    let mut buf = Buffer::empty(area);
    let rows: Vec<Row> = (0..6)
        .map(|i| row(&format!("r{i}"), State::Running, vec![]))
        .collect();
    let selected = RowId { pane: "%r5".into() };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: Some(&selected),
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    // The list is capped at eight rows: four lines between its heads and
    // its foot, a blank one between two sessions. A session line is on
    // top, not the blank line above it.
    assert!(text(&buf, 3).starts_with(" r4"));
    assert_eq!(text(&buf, 4), "");
    assert!(text(&buf, 5).starts_with(" r5"));
    assert_eq!(text(&buf, 6), "");
    assert_eq!(text(&buf, 7), "", "a blank line above the preview");
    assert!(text(&buf, 8).starts_with("╭ r5: no terminal"));
}

/// A subagent's task row on `branch`.
fn subagent(branch: &str) -> TaskRow {
    TaskRow {
        subagent: Some(SubagentTask {
            agent_id: branch.into(),
            description: None,
            state: TaskState::Running,
        }),
        ..task(branch, None)
    }
}

#[test]
fn the_selected_line_scrolls_into_sight_with_its_subagents() {
    let area = Rect::new(0, 0, 30, 20);
    let mut buf = Buffer::empty(area);
    let rows = [
        row("r0", State::Running, vec![]),
        row("r1", State::Running, vec![]),
        row("r2", State::Running, vec![subagent("s1"), subagent("s2")]),
    ];
    let selected = RowId { pane: "%r2".into() };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: Some(&selected),
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    assert!(text(&buf, 3).starts_with(" r2"));
    assert!(text(&buf, 4).starts_with("   s1"));
    assert!(text(&buf, 5).starts_with("   s2"));
    // The whole block is highlighted, subagents included.
    assert_eq!([3, 4, 5].map(|y| buf[(1, y)].bg), [SELECTED; 3]);
}

#[test]
fn the_list_never_starts_with_another_sessions_task_rows() {
    let area = Rect::new(0, 0, 30, 20);
    let mut buf = Buffer::empty(area);
    let rows = [
        row(
            "r0",
            State::Running,
            vec![subagent("a"), subagent("b"), subagent("c")],
        ),
        row("r1", State::Running, vec![]),
    ];
    let selected = RowId { pane: "%r1".into() };
    let view = View {
        keys: KEYS,
        rows: &rows,
        details: &TaskDetails::new(),
        selected: Some(&selected),
        preview: None,
        interactive: false,
        filter: &DashboardFilter::default(),
        now: 0,
        selection: SELECTED,
    };
    render(&view, area, &mut buf);
    // Four lines of room: r0's block does not fit whole with r1's, so
    // r1 is on top rather than r0's last task rows.
    assert!(text(&buf, 3).starts_with(" r1"));
    assert_eq!([4, 5, 6].map(|y| text(&buf, y)), ["", "", ""]);
}
