//! The sidebar, with a border on its right edge: a SPACES section with one
//! line per space (tmux session, ADR 0006), then an AGENTS section with two
//! lines per main Claude session, across every space:
//!
//! ```text
//!  SPACES
//!  ● api-server
//!    paddock
//!
//!  AGENTS
//!  ● api-server  waiting
//!    api-server · 2:claude
//! ```
//!
//! A space's dot shows the most urgent state of the agents in it (none
//! without agents); the current space has its own background. An agent's
//! second line is dim and names its space and tab. While the sidebar has the
//! keyboard, the selected space or agent is drawn reversed.

use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

const SPACES: &str = "SPACES";
const AGENTS: &str = "AGENTS";
/// Marks a state.
const DOT: &str = "●";
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
}

/// One agent (main Claude session) as the sidebar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLine {
    pub label: String,
    pub state: State,
    /// Where it runs: `space · tab`.
    pub place: String,
}

/// A line of the sidebar, below the top of its area. Drawing and clicks
/// both go through [`lines`], so they always agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Header(&'static str),
    Space(usize),
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
        Line::Space(i) => Some(i),
        Line::Agent(i) | Line::AgentPlace(i) => Some(spaces + i),
        Line::Header(_) | Line::Blank => None,
    }
}

/// The sidebar's lines for `spaces` spaces and `agents` agents, top first.
pub fn lines(spaces: usize, agents: usize) -> Vec<Line> {
    let mut out = vec![Line::Header(SPACES)];
    out.extend((0..spaces).map(Line::Space));
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
/// keyboard is on while the sidebar has it; `current_bg` the current
/// space's background.
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
    let inner = area.width - 1;
    for y in area.top()..area.bottom() {
        buf.set_string(area.right() - 1, y, "│", Style::default());
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    for (line, y) in lines(spaces.len(), agents.len())
        .into_iter()
        .zip(area.top()..area.bottom())
    {
        let row = Rect::new(area.x, y, inner, 1);
        let mut base = Style::default();
        if let Line::Space(i) = line {
            if spaces[i].current {
                base = base.bg(current_bg);
            }
        }
        if selected.is_some() && pick(line, spaces.len()) == selected {
            base = base.add_modifier(Modifier::REVERSED);
        }
        buf.set_style(row, base);
        let mut put = |x: u16, text: &str, style: Style| {
            let room = usize::from(row.right().saturating_sub(x));
            buf.set_stringn(x, y, text, room, base.patch(style)).0
        };
        let (name_x, end) = (area.x + INDENT, row.right().saturating_sub(1));
        match line {
            Line::Header(title) => {
                put(area.x + 1, title, dim.add_modifier(Modifier::BOLD));
            }
            Line::Space(i) => {
                let s = &spaces[i];
                if let Some(state) = s.state {
                    put(area.x + 1, DOT, dot(state));
                }
                put(name_x, &s.name, Style::default());
            }
            Line::Agent(i) => {
                let a = &agents[i];
                put(area.x + 1, DOT, dot(a.state));
                let state = a.state.label();
                let width = u16::try_from(state.chars().count()).unwrap_or(0);
                let state_x = end.saturating_sub(width).max(name_x);
                let room = usize::from(state_x.saturating_sub(name_x + 1));
                let label: String = a.label.chars().take(room).collect();
                put(name_x, &label, Style::default());
                put(state_x, state, highlight(a.state));
            }
            Line::AgentPlace(i) => {
                put(name_x, &agents[i].place, dim);
            }
            Line::Blank => {}
        }
    }
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
        }
    }

    fn agent(label: &str, state: State, place: &str) -> AgentLine {
        AgentLine {
            label: label.into(),
            state,
            place: place.into(),
        }
    }

    fn text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    const GREY: Color = Color::Indexed(236);

    fn draw(spaces: &[SpaceLine], agents: &[AgentLine], selected: Option<usize>) -> Buffer {
        let area = Rect::new(0, 0, 24, 10);
        let mut buf = Buffer::empty(area);
        render(spaces, agents, selected, GREY, area, &mut buf);
        buf
    }

    #[test]
    fn spaces_then_agents_each_under_a_header() {
        let spaces = [
            space("api-server", Some(State::Waiting), true),
            space("paddock", None, false),
        ];
        let agents = [
            agent("api", State::Waiting, "api-server · 2:claude"),
            agent("a-very-long-label", State::Running, "api-server · 1:x"),
        ];
        let buf = draw(&spaces, &agents, None);
        let want = [
            " SPACES                │",
            " ● api-server          │",
            "   paddock             │",
            "                       │",
            " AGENTS                │",
            " ● api         waiting │",
            "   api-server · 2:claud│",
            "                       │",
            " ● a-very-long running │",
            "   api-server · 1:x    │",
        ];
        for (y, line) in want.iter().enumerate() {
            assert_eq!(text(&buf, y as u16), *line, "row {y}");
        }
        assert!(buf[(3, 6)].modifier.contains(Modifier::DIM), "place is dim");
    }

    #[test]
    fn states_stand_out_and_the_current_space_has_its_own_background() {
        let spaces = [
            space("cur", Some(State::Idle), true),
            space("other", Some(State::Done), false),
        ];
        let agents = [agent("w", State::Waiting, "cur · 1:a")];
        let buf = draw(&spaces, &agents, None);
        assert_eq!(buf[(0, 1)].bg, GREY);
        assert_eq!(buf[(22, 1)].bg, GREY);
        assert_eq!(buf[(23, 1)].bg, Color::Reset, "not the border");
        assert_eq!(buf[(5, 2)].bg, Color::Reset);
        assert_eq!(buf[(1, 1)].fg, Color::Green, "idle dot");
        assert!(buf[(1, 2)].modifier.contains(Modifier::BOLD), "done dot");
        let waiting = &buf[(16, 5)];
        assert_eq!((waiting.fg, waiting.bg), (Color::Black, Color::Yellow));
        assert_eq!(buf[(1, 5)].fg, Color::Yellow);
    }

    #[test]
    fn the_selected_space_or_agent_is_reversed() {
        let spaces = [space("a", None, false), space("b", None, false)];
        let agents = [agent("x", State::Idle, "a · 1:x")];
        let buf = draw(&spaces, &agents, Some(1));
        assert!(buf[(5, 2)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(5, 1)].modifier.contains(Modifier::REVERSED));
        let buf = draw(&spaces, &agents, Some(2));
        assert!(buf[(5, 5)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(5, 6)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(5, 2)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn control_characters_in_names_never_reach_the_screen() {
        // Names come from tmux and Claude; an escape sequence in one must
        // not be written to the outer terminal.
        let evil = "a\x1b]52;c;eA==\x07b";
        let buf = draw(
            &[space(evil, None, false)],
            &[agent(evil, State::Idle, evil)],
            None,
        );
        for y in 0..10 {
            let drawn = text(&buf, y);
            assert!(!drawn.contains(char::is_control), "{drawn:?}");
        }
    }

    #[test]
    fn clicks_find_what_is_drawn_under_them() {
        let area = Rect::new(0, 3, 24, 12);
        // 3 + 0 SPACES, 4-5 spaces, 6 blank, 7 AGENTS, 8-9 agent 0, 10
        // blank, 11-12 agent 1.
        let at = |y| pick_at(2, 2, area, y);
        assert_eq!(at(3), None, "header");
        assert_eq!(at(4), Some(0));
        assert_eq!(at(5), Some(1));
        assert_eq!((at(6), at(7)), (None, None));
        assert_eq!((at(8), at(9)), (Some(2), Some(2)));
        assert_eq!(at(10), None);
        assert_eq!(at(12), Some(3));
        assert_eq!(at(13), None, "below the last line");
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
}
