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
//! ```
//!
//! A space's dot shows the most urgent state of the agents in it (none
//! without agents); its second line, dim, shows its directory and the git
//! branch checked out there. The current space has its own background
//! over both lines. An agent's
//! second line is dim and names its space and tab; the current agent (the
//! one in the focused terminal) has the current space's background too. While the sidebar has the
//! keyboard, the selected space or agent is drawn reversed.

use super::STATE_DOT;
use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

const SPACES: &str = "SPACES";
const AGENTS: &str = "AGENTS";
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
}

/// One agent (main Claude session) as the sidebar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLine {
    pub label: String,
    pub state: State,
    /// Where it runs: `space · tab`.
    pub place: String,
    /// Its pane is the focused terminal of the page shown now.
    pub current: bool,
}

/// A line of the sidebar, below the top of its area. Drawing and clicks
/// both go through [`lines`], so they always agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Header(&'static str),
    Space(usize),
    /// A space's second line: its directory and branch.
    SpacePlace(usize),
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
        Line::Header(_) | Line::Blank => None,
    }
}

/// The sidebar's lines for `spaces` spaces and `agents` agents, top first.
pub fn lines(spaces: usize, agents: usize) -> Vec<Line> {
    let mut out = vec![Line::Header(SPACES)];
    out.extend((0..spaces).flat_map(|i| [Line::Space(i), Line::SpacePlace(i)]));
    out.extend([Line::Blank, Line::Header(AGENTS)]);
    for i in 0..agents {
        if i > 0 {
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
    spaces: &[SpaceLine],
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
        spaces,
        agents,
        selected,
        current_bg,
    };
    for (line, y) in lines(spaces.len(), agents.len())
        .into_iter()
        .zip(area.top()..area.bottom())
    {
        sidebar.draw_line(line, Rect::new(area.x, y, area.width - 1, 1), buf);
    }
}

/// What [`render`] draws, line by line.
struct Sidebar<'a> {
    spaces: &'a [SpaceLine],
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
            Line::Header(_) | Line::Blank => false,
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
        match line {
            Line::Header(title) => {
                put(dot_x, title, dim.add_modifier(Modifier::BOLD));
            }
            Line::Space(i) => {
                let s = &self.spaces[i];
                if let Some(state) = s.state {
                    put(dot_x, STATE_DOT, dot(state));
                }
                put(name_x, &s.name, Style::default());
            }
            Line::Agent(i) => {
                let a = &self.agents[i];
                put(dot_x, STATE_DOT, dot(a.state));
                let state = a.state.label();
                let width = u16::try_from(state.chars().count()).unwrap_or(0);
                let state_x = end.saturating_sub(width).max(name_x);
                let room = usize::from(state_x.saturating_sub(name_x + 1));
                let label: String = a.label.chars().take(room).collect();
                put(name_x, &label, Style::default());
                put(state_x, state, highlight(a.state));
            }
            Line::SpacePlace(i) => {
                let room = usize::from(end.saturating_sub(name_x));
                let s = &self.spaces[i];
                if let Some(path) = &s.path {
                    put(name_x, &place(path, s.branch.as_deref(), room), dim);
                }
            }
            Line::AgentPlace(i) => {
                put(name_x, &self.agents[i].place, dim);
            }
            Line::Blank => {}
        }
    }
}

/// `path` with the home directory `home` written `~`.
pub fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_string(),
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
pub fn pick_at(spaces: usize, agents: usize, area: Rect, y: u16) -> Option<usize> {
    if !(area.top()..area.bottom()).contains(&y) {
        return None;
    }
    let line = lines(spaces, agents)
        .into_iter()
        .nth(usize::from(y - area.y))?;
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
        }
    }

    fn agent(label: &str, state: State, place: &str) -> AgentLine {
        AgentLine {
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
        let area = Rect::new(0, 0, 24, 12);
        let mut buf = Buffer::empty(area);
        render(spaces, agents, selected, GREY, area, &mut buf);
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
    fn clicks_find_what_is_drawn_under_them() {
        let area = Rect::new(0, 3, 24, 14);
        // 3 SPACES, 4-5 space 0, 6-7 space 1, 8 blank, 9 AGENTS,
        // 10-11 agent 0, 12 blank, 13-14 agent 1.
        let at = |y| pick_at(2, 2, area, y);
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
            lines(1, 2),
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
    fn home_is_written_as_a_tilde() {
        assert_eq!(tilde("/Users/me/project/api", "/Users/me"), "~/project/api");
        assert_eq!(tilde("/Users/me", "/Users/me"), "~");
        assert_eq!(tilde("/Users/meg/x", "/Users/me"), "/Users/meg/x");
        assert_eq!(tilde("/tmp", ""), "/tmp");
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
