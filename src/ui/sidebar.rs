//! The sidebar, with a border on its right edge: a SPACES section with one
//! line per space (tmux session, ADR 0006), then an AGENTS section with two
//! lines per main Claude session, across every space:
//!
//! ```text
//!  SPACES
//!  ● api-server
//!    ~/project/api  main
//!    paddock
//!    ~/project/paddock  feat/x
//!
//!  AGENTS
//!  ● api-server  waiting
//!    api-server · claude
//!
//!  EXTERNAL
//!  ● web  idle
//!    feat/login
//! ```
//!
//! A space's dot shows the most urgent state of the agents in it (none
//! without agents); its second line, dim, shows its directory and the git
//! branch checked out there. A space in a linked worktree is listed one
//! cell further in, under its repository's main-checkout space, or under
//! a dim header with the repository's name when no space is in the main
//! checkout:
//!
//! ```text
//!  ● api-server
//!    ~/project/api  main
//!     api-server/feat/x
//!     …es/api-server/feat-x  feat/x
//!    paddock
//!     paddock/feat/y
//!     …trees/paddock/feat-y  feat/y
//! ```
//!
//! The current space has its own background over both lines. An agent's
//! second line is dim and names its space and tab; the current agent (the
//! one in the focused terminal) has the current space's background too.
//! While the sidebar has the keyboard, the selected space or agent is drawn
//! reversed. External sessions (Claude outside paddock's terminals) close
//! AGENTS under their own header, dim, their second line naming their branch
//! (else their directory).

use super::STATE_DOT;
use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

const SPACES: &str = "SPACES";
const AGENTS: &str = "AGENTS";
const EXTERNAL: &str = "EXTERNAL";
/// Columns before a name: one space, the dot, one space.
const INDENT: u16 = 3;

/// One space as the sidebar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceLine {
    pub name: String,
    /// The most urgent state of the agents in it; `None` without agents.
    pub state: Option<State>,
    /// Its tabs are the ones shown.
    pub current: bool,
    /// Its directory as shown (home as `~`); `None` until read, and the
    /// second line stays empty until then.
    pub path: Option<String>,
    /// The branch (or detached commit) checked out there, if a repository.
    pub branch: Option<String>,
    /// Its directory is a linked worktree: drawn one cell further in,
    /// under its repository.
    pub linked: bool,
}

/// A line of SPACES: a space, or the header over the worktree spaces of
/// a repository whose main checkout has no space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Space(SpaceLine),
    /// The repository's name.
    RepoHeader(String),
}

/// One agent (main Claude session) as the sidebar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLine {
    pub label: String,
    pub state: State,
    /// Where it runs: `space · tab`; an external session's branch, else
    /// its directory.
    pub place: String,
    /// Its pane is the focused terminal of the page shown now.
    pub current: bool,
    /// It runs outside paddock's terminals: drawn dim, in the external
    /// group after the others.
    pub external: bool,
}

/// A line of the sidebar, below the top of its area. Drawing and clicks
/// both go through [`lines`], so they always agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Header(&'static str),
    Space(usize),
    /// A space's second line: its directory and branch.
    SpacePlace(usize),
    /// A repository's header over its worktree spaces.
    RepoHeader(usize),
    /// An agent's first line: dot, label and state.
    Agent(usize),
    /// An agent's second line: its space and tab.
    AgentPlace(usize),
    Blank,
}

/// What a sidebar line picks: space `i` or agent `i`, as an index into the
/// spaces followed by the agents.
pub fn pick(line: Line, spaces: usize) -> Option<usize> {
    match line {
        Line::Space(i) | Line::SpacePlace(i) => Some(i),
        Line::Agent(i) | Line::AgentPlace(i) => Some(spaces + i),
        Line::Header(_) | Line::RepoHeader(_) | Line::Blank => None,
    }
}

/// The sidebar's lines for `entries` (spaces and repository headers) and
/// `agents` agents, top first, the last `external` of them under their own
/// header. Spaces and headers are numbered apart, in their order among
/// `entries`.
pub fn lines(entries: &[Entry], agents: usize, external: usize) -> Vec<Line> {
    let mut out = vec![Line::Header(SPACES)];
    let (mut spaces, mut repos) = (0, 0);
    for entry in entries {
        match entry {
            Entry::Space(_) => {
                out.extend([Line::Space(spaces), Line::SpacePlace(spaces)]);
                spaces += 1;
            }
            Entry::RepoHeader(_) => {
                out.push(Line::RepoHeader(repos));
                repos += 1;
            }
        }
    }
    out.extend([Line::Blank, Line::Header(AGENTS)]);
    let first_external = agents.saturating_sub(external);
    for i in 0..agents {
        if i == first_external {
            out.extend([Line::Blank, Line::Header(EXTERNAL)]);
        } else if i > 0 {
            out.push(Line::Blank);
        }
        out.extend([Line::Agent(i), Line::AgentPlace(i)]);
    }
    out
}

/// How a state (of an agent, or of a tab holding it) stands out, in the
/// terminal's own ANSI colours: strongly while waiting, softly once done
/// (bold) or idle.
pub fn highlight(state: State) -> Style {
    match state {
        State::Waiting => Style::default().fg(Color::Black).bg(Color::Yellow),
        State::Done => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        State::Idle => Style::default().fg(Color::Green),
        State::Running | State::Stopped | State::Unknown => Style::default(),
    }
}

/// The colour of a state's dot.
fn dot(state: State) -> Style {
    match state {
        State::Waiting => Style::default().fg(Color::Yellow),
        State::Done | State::Idle => highlight(state),
        State::Running => Style::default().fg(Color::Blue),
        State::Stopped | State::Unknown => Style::default().add_modifier(Modifier::DIM),
    }
}

/// Draw the sidebar into `area`. `selected` is the pick (see [`pick`]) the
/// keyboard is on while the sidebar has it; `current_bg` the background of
/// the current space and the current agent.
pub fn render(
    entries: &[Entry],
    agents: &[AgentLine],
    selected: Option<usize>,
    current_bg: Color,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width < 2 || area.height == 0 {
        return;
    }
    for y in area.top()..area.bottom() {
        buf.set_string(area.right() - 1, y, "│", Style::default());
    }
    let sidebar = Sidebar {
        spaces: entries
            .iter()
            .filter_map(|e| match e {
                Entry::Space(s) => Some(s),
                Entry::RepoHeader(_) => None,
            })
            .collect(),
        repos: entries
            .iter()
            .filter_map(|e| match e {
                Entry::RepoHeader(name) => Some(name.as_str()),
                Entry::Space(_) => None,
            })
            .collect(),
        agents,
        selected,
        current_bg,
    };
    let external = agents.iter().filter(|a| a.external).count();
    for (line, y) in lines(entries, agents.len(), external)
        .into_iter()
        .zip(area.top()..area.bottom())
    {
        sidebar.draw_line(line, Rect::new(area.x, y, area.width - 1, 1), buf);
    }
}

/// What [`render`] draws, line by line.
struct Sidebar<'a> {
    spaces: Vec<&'a SpaceLine>,
    /// The repository headers, in order.
    repos: Vec<&'a str>,
    agents: &'a [AgentLine],
    selected: Option<usize>,
    current_bg: Color,
}

impl Sidebar<'_> {
    /// The style a line starts from: the current background on the
    /// current space's and agent's lines, and reversed if selected.
    fn base(&self, line: Line) -> Style {
        let mut base = Style::default();
        let current = match line {
            Line::Space(i) | Line::SpacePlace(i) => self.spaces[i].current,
            Line::Agent(i) | Line::AgentPlace(i) => self.agents[i].current,
            Line::Header(_) | Line::RepoHeader(_) | Line::Blank => false,
        };
        if current {
            base = base.bg(self.current_bg);
        }
        if self.selected.is_some() && pick(line, self.spaces.len()) == self.selected {
            base = base.add_modifier(Modifier::REVERSED);
        }
        base
    }

    /// Draw `line` over `row` (the sidebar's width, less its border).
    fn draw_line(&self, line: Line, row: Rect, buf: &mut Buffer) {
        let base = self.base(line);
        buf.set_style(row, base);
        let mut put = |x: u16, text: &str, style: Style| {
            let room = usize::from(row.right().saturating_sub(x));
            buf.set_stringn(x, row.y, text, room, base.patch(style)).0
        };
        let dim = Style::default().add_modifier(Modifier::DIM);
        let (dot_x, name_x) = (row.x + 1, row.x + INDENT);
        let end = row.right().saturating_sub(1);
        // A worktree space sits one cell further in, under its repository.
        let indent = |x: u16, linked: bool| x + u16::from(linked);
        match line {
            Line::Header(title) => {
                put(dot_x, title, dim.add_modifier(Modifier::BOLD));
            }
            Line::RepoHeader(i) => {
                put(name_x, self.repos[i], dim);
            }
            Line::Space(i) => {
                let s = &self.spaces[i];
                if let Some(state) = s.state {
                    put(indent(dot_x, s.linked), STATE_DOT, dot(state));
                }
                put(indent(name_x, s.linked), &s.name, Style::default());
            }
            Line::Agent(i) => {
                let a = &self.agents[i];
                let name = if a.external { dim } else { Style::default() };
                put(dot_x, STATE_DOT, dot(a.state));
                let state = a.state.label();
                let width = u16::try_from(state.chars().count()).unwrap_or(0);
                let state_x = end.saturating_sub(width).max(name_x);
                let room = usize::from(state_x.saturating_sub(name_x + 1));
                let label: String = a.label.chars().take(room).collect();
                put(name_x, &label, name);
                put(state_x, state, highlight(a.state));
            }
            Line::SpacePlace(i) => {
                let s = &self.spaces[i];
                let x = indent(name_x, s.linked);
                let room = usize::from(end.saturating_sub(x));
                if let Some(path) = &s.path {
                    put(x, &place(path, s.branch.as_deref(), room), dim);
                }
            }
            Line::AgentPlace(i) => {
                put(name_x, &self.agents[i].place, dim);
            }
            Line::Blank => {}
        }
    }
}

/// A space's directory and branch in `width` cells: `path  branch`. Too
/// long, the path loses its start first (`…ject/api`), then the branch its
/// end (`feat/lo…`).
pub fn place(path: &str, branch: Option<&str>, width: usize) -> String {
    let len = |t: &str| t.chars().count();
    let tail = |t: &str, n: usize| -> String {
        if len(t) <= n {
            return t.to_string();
        }
        let keep: String = t.chars().skip(len(t) + 1 - n.max(1)).collect();
        format!("…{keep}")
    };
    let head = |t: &str, n: usize| -> String {
        if len(t) <= n {
            return t.to_string();
        }
        let keep: String = t.chars().take(n.saturating_sub(1)).collect();
        format!("{keep}…")
    };
    let Some(branch) = branch else {
        return if width == 0 {
            String::new()
        } else {
            tail(path, width)
        };
    };
    let path_room = width.saturating_sub(len(branch) + 2);
    if path_room >= 1 {
        return format!("{}  {branch}", tail(path, path_room));
    }
    // Even `…` and the branch do not fit: keep `…` and cut the branch.
    let branch_room = width.saturating_sub(3);
    format!("…  {}", head(branch, branch_room))
        .chars()
        .take(width)
        .collect()
}

/// What the sidebar line at screen row `y` of `area` picks, if anything.
/// `agents` and `external` are as [`lines`] takes them.
pub fn pick_at(
    entries: &[Entry],
    (agents, external): (usize, usize),
    area: Rect,
    y: u16,
) -> Option<usize> {
    if !(area.top()..area.bottom()).contains(&y) {
        return None;
    }
    let line = lines(entries, agents, external)
        .into_iter()
        .nth(usize::from(y - area.y))?;
    let spaces = entries
        .iter()
        .filter(|e| matches!(e, Entry::Space(_)))
        .count();
    pick(line, spaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(name: &str, state: Option<State>, current: bool) -> SpaceLine {
        SpaceLine {
            name: name.into(),
            state,
            current,
            path: Some(format!("~/p/{name}")),
            branch: None,
            linked: false,
        }
    }

    fn entries(spaces: &[SpaceLine]) -> Vec<Entry> {
        spaces.iter().cloned().map(Entry::Space).collect()
    }

    fn agent(label: &str, state: State, place: &str) -> AgentLine {
        AgentLine {
            external: false,
            label: label.into(),
            state,
            place: place.into(),
            current: false,
        }
    }

    fn text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    const GREY: Color = Color::Indexed(236);

    fn draw(spaces: &[SpaceLine], agents: &[AgentLine], selected: Option<usize>) -> Buffer {
        draw_entries(&entries(spaces), agents, selected)
    }

    fn draw_entries(entries: &[Entry], agents: &[AgentLine], selected: Option<usize>) -> Buffer {
        let area = Rect::new(0, 0, 24, 12);
        let mut buf = Buffer::empty(area);
        render(entries, agents, selected, GREY, area, &mut buf);
        buf
    }

    #[test]
    fn spaces_then_agents_each_under_a_header() {
        let spaces = [
            SpaceLine {
                path: Some("~/project/api".into()),
                branch: Some("main".into()),
                ..space("api-server", Some(State::Waiting), true)
            },
            space("paddock", None, false),
        ];
        let agents = [
            agent("api", State::Waiting, "api-server · claude"),
            agent("a-very-long-label", State::Running, "api-server · x"),
        ];
        let buf = draw(&spaces, &agents, None);
        let want = [
            " SPACES                │",
            " ● api-server          │",
            "   ~/project/api  main │",
            "   paddock             │",
            "   ~/p/paddock         │",
            "                       │",
            " AGENTS                │",
            " ● api         waiting │",
            "   api-server · claude │",
            "                       │",
            " ● a-very-long running │",
            "   api-server · x      │",
        ];
        for (y, line) in want.iter().enumerate() {
            assert_eq!(text(&buf, y as u16), *line, "row {y}");
        }
        assert!(buf[(3, 2)].modifier.contains(Modifier::DIM), "path is dim");
        assert!(buf[(3, 8)].modifier.contains(Modifier::DIM), "place is dim");
    }

    #[test]
    fn states_stand_out_and_the_current_space_has_its_own_background() {
        let spaces = [
            space("cur", Some(State::Idle), true),
            space("other", Some(State::Done), false),
        ];
        let agents = [agent("w", State::Waiting, "cur · a")];
        let buf = draw(&spaces, &agents, None);
        for (x, y) in [(0, 1), (22, 1), (0, 2), (22, 2)] {
            assert_eq!(buf[(x, y)].bg, GREY, "({x}, {y}): both lines");
        }
        assert_eq!(buf[(23, 1)].bg, Color::Reset, "not the border");
        assert_eq!(buf[(5, 3)].bg, Color::Reset);
        assert_eq!(buf[(1, 1)].fg, Color::Green, "idle dot");
        assert!(buf[(1, 3)].modifier.contains(Modifier::BOLD), "done dot");
        let waiting = &buf[(16, 7)];
        assert_eq!((waiting.fg, waiting.bg), (Color::Black, Color::Yellow));
        assert_eq!(buf[(1, 7)].fg, Color::Yellow);
    }

    #[test]
    fn the_current_agent_has_the_current_background_over_both_its_lines() {
        let spaces = [space("a", None, false)];
        let agents = [
            agent("x", State::Idle, "a · x"),
            AgentLine {
                current: true,
                ..agent("y", State::Running, "a · y")
            },
        ];
        let buf = draw(&spaces, &agents, None);
        // 0 SPACES, 1-2 a, 3 blank, 4 AGENTS, 5-6 x, 7 blank, 8-9 y.
        for (x, y) in [(0, 8), (22, 8), (0, 9), (22, 9)] {
            assert_eq!(buf[(x, y)].bg, GREY, "({x}, {y}): both lines");
        }
        assert_eq!(buf[(23, 8)].bg, Color::Reset, "not the border");
        assert_eq!(buf[(5, 7)].bg, Color::Reset, "not the blank above");
        assert_eq!(buf[(5, 5)].bg, Color::Reset, "not another agent");
        assert_eq!(buf[(5, 1)].bg, Color::Reset, "no current space");
    }

    #[test]
    fn the_selected_space_or_agent_is_reversed() {
        let spaces = [space("a", None, false), space("b", None, false)];
        let agents = [agent("x", State::Idle, "a · x")];
        let reversed = |buf: &Buffer, y| buf[(5, y)].modifier.contains(Modifier::REVERSED);
        let buf = draw(&spaces, &agents, Some(1));
        assert!(reversed(&buf, 3) && reversed(&buf, 4));
        assert!(!reversed(&buf, 1) && !reversed(&buf, 2));
        let buf = draw(&spaces, &agents, Some(2));
        assert!(reversed(&buf, 7) && reversed(&buf, 8));
        assert!(!reversed(&buf, 3));
    }

    #[test]
    fn control_characters_in_names_never_reach_the_screen() {
        // Names and paths come from tmux and Claude; an escape sequence in
        // one must not be written to the outer terminal.
        let evil = "a\x1b]52;c;eA==\x07b";
        let buf = draw(
            &[SpaceLine {
                path: Some(evil.into()),
                branch: Some(evil.into()),
                ..space(evil, None, false)
            }],
            &[agent(evil, State::Idle, evil)],
            None,
        );
        for y in 0..12 {
            let drawn = text(&buf, y);
            assert!(!drawn.contains(char::is_control), "{drawn:?}");
        }
    }

    #[test]
    fn worktree_spaces_sit_under_their_repository_or_a_header() {
        let linked = |name: &str, branch: &str| SpaceLine {
            path: Some(format!("~/w/{branch}")),
            branch: Some(branch.into()),
            linked: true,
            ..space(name, Some(State::Waiting), false)
        };
        let entries = [
            Entry::Space(SpaceLine {
                branch: Some("main".into()),
                ..space("api", None, true)
            }),
            Entry::Space(linked("api/feat/x", "feat/x")),
            Entry::RepoHeader("paddock".into()),
            Entry::Space(linked("paddock/feat/y", "feat/y")),
        ];
        let agents = [agent("x", State::Waiting, "api/feat/x · claude")];
        let buf = draw_entries(&entries, &agents, Some(2));
        let want = [
            " SPACES                │",
            "   api                 │",
            "   ~/p/api  main       │",
            "  ● api/feat/x         │",
            "    ~/w/feat/x  feat/x │",
            "   paddock             │",
            "  ● paddock/feat/y     │",
            "    ~/w/feat/y  feat/y │",
            "                       │",
            " AGENTS                │",
            " ● x           waiting │",
            "   api/feat/x · claude │",
        ];
        for (y, line) in want.iter().enumerate() {
            assert_eq!(text(&buf, y as u16), *line, "row {y}");
        }
        assert!(
            buf[(3, 5)].modifier.contains(Modifier::DIM),
            "header is dim"
        );
        assert!(!buf[(3, 5)].modifier.contains(Modifier::REVERSED));
        let reversed = |y| buf[(5, y)].modifier.contains(Modifier::REVERSED);
        assert!(reversed(6) && reversed(7), "the third space is selected");
        assert!(!reversed(3) && !reversed(5));
        for (x, y) in [(0, 1), (22, 2)] {
            assert_eq!(buf[(x, y)].bg, GREY, "({x}, {y}): current space");
        }
        // Clicks: the header picks nothing; the spaces count without it.
        let area = Rect::new(0, 0, 24, 12);
        let at = |y| pick_at(&entries, (1, 0), area, y);
        assert_eq!((at(1), at(4)), (Some(0), Some(1)));
        assert_eq!(at(5), None, "header");
        assert_eq!((at(6), at(7)), (Some(2), Some(2)));
        assert_eq!(at(10), Some(3), "the agent");
    }

    #[test]
    fn clicks_find_what_is_drawn_under_them() {
        let area = Rect::new(0, 3, 24, 14);
        // 3 SPACES, 4-5 space 0, 6-7 space 1, 8 blank, 9 AGENTS,
        // 10-11 agent 0, 12 blank, 13-14 agent 1.
        let two = entries(&[space("a", None, false), space("b", None, false)]);
        let at = |y| pick_at(&two, (2, 0), area, y);
        assert_eq!(at(3), None, "header");
        assert_eq!((at(4), at(5)), (Some(0), Some(0)));
        assert_eq!((at(6), at(7)), (Some(1), Some(1)));
        assert_eq!((at(8), at(9)), (None, None));
        assert_eq!((at(10), at(11)), (Some(2), Some(2)));
        assert_eq!(at(12), None);
        assert_eq!(at(14), Some(3));
        assert_eq!(at(15), None, "below the last line");
        assert_eq!(at(20), None, "below the sidebar");
    }

    #[test]
    fn lines_put_a_blank_between_agents_only() {
        use Line::*;
        assert_eq!(
            lines(&entries(&[space("a", None, false)]), 2, 0),
            [
                Header(SPACES),
                Space(0),
                SpacePlace(0),
                Blank,
                Header(AGENTS),
                Agent(0),
                AgentPlace(0),
                Blank,
                Agent(1),
                AgentPlace(1)
            ]
        );
    }

    #[test]
    fn external_agents_close_agents_under_their_own_header() {
        use Line::*;
        let ent = entries(&[space("a", None, false)]);
        assert_eq!(
            lines(&ent, 2, 1)[3..],
            [
                Blank,
                Header(AGENTS),
                Agent(0),
                AgentPlace(0),
                Blank,
                Header(EXTERNAL),
                Agent(1),
                AgentPlace(1)
            ]
        );
        assert_eq!(
            lines(&ent, 1, 1)[3..],
            [
                Blank,
                Header(AGENTS),
                Blank,
                Header(EXTERNAL),
                Agent(0),
                AgentPlace(0)
            ]
        );
        let area = Rect::new(0, 0, 24, 20);
        // The external header picks nothing; its agent is pick 2.
        assert_eq!(pick_at(&ent, (2, 1), area, 8), None);
        assert_eq!(pick_at(&ent, (2, 1), area, 9), Some(2));
        let agents = [
            agent("api", State::Running, "a · claude"),
            AgentLine {
                external: true,
                ..agent("web", State::Waiting, "feat/x")
            },
        ];
        let buf = draw(&[space("a", Some(State::Running), false)], &agents, None);
        assert_eq!(text(&buf, 8).trim_end_matches([' ', '│']), " EXTERNAL");
        assert!(text(&buf, 9).starts_with(" ● web"), "{}", text(&buf, 9));
        assert!(buf[(3, 9)].modifier.contains(Modifier::DIM), "dim label");
        assert!(!buf[(3, 5)].modifier.contains(Modifier::DIM));
        assert_eq!(text(&buf, 10).trim_end_matches([' ', '│']), "   feat/x");
        assert!(buf[(3, 10)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_space_whose_directory_is_not_read_yet_shows_no_second_line() {
        let spaces = [SpaceLine {
            path: None,
            branch: Some("main".into()),
            ..space("new", None, false)
        }];
        let buf = draw(&spaces, &[], None);
        assert_eq!(text(&buf, 1), "   new                 │");
        assert_eq!(text(&buf, 2), "                       │", "kept, empty");
        assert!(!buf[(3, 2)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_long_place_loses_the_paths_start_first_then_the_branchs_end() {
        assert_eq!(
            place("~/project/api", Some("main"), 19),
            "~/project/api  main"
        );
        assert_eq!(
            place("~/project/herdr", Some("feat/x"), 19),
            "…ject/herdr  feat/x"
        );
        assert_eq!(
            place("~/a", Some("feature/very-long"), 19),
            "…  feature/very-lo…"
        );
        assert_eq!(place("~/a/very/long/path", None, 10), "…long/path");
        assert_eq!(place("~/a", None, 19), "~/a");
        for width in 0..25 {
            let got = place("~/project/herdr", Some("feature/branch"), width);
            assert!(got.chars().count() <= width, "{width}: {got:?}");
        }
    }
}
