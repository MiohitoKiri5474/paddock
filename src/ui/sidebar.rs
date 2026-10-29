//! The sidebar, with a border on its right edge: a SPACES section with one
//! line per space (tmux session, ADR 0006), then an AGENTS section with one
//! line per main agent session, across every space:
//!
//! ```text
//!  SPACES
//!  ● api-server
//!    ~/project/api  main
//!    paddock                 +200 −10 ↑2↓1
//!    ~/p/paddock  feat/x  #42 ✓
//!
//!  AGENTS
//!  ● PR-1434     waiting  core
//!  ● api-server  running  api
//!  EXTERNAL
//!  ● web         idle
//! ```
//!
//! A space's dot shows the most urgent state of the agents in it (none
//! without agents); at the end of its name's line comes the status of the
//! branch checked out in it (as the dashboard writes it: its line changes
//! `+200` in green and `−10` in red, then commits ahead of and behind
//! upstream `↑2↓1`, dim; parts at zero left out, nothing for a detached
//! HEAD or outside a repository; no colour on the selected, reversed,
//! line), two cells or more after the name. Short of room the name is cut
//! with `…` down to its first eight cells; past that the status is left
//! out whole, never cut. Its second line, dim, shows its directory and that
//! branch, then, two cells or more on, at the end, the branch's pull request:
//! its number in its state's colour and its checks mark (`✓`, `✗`, `•`).
//! Short of room the path gives way first (`…` from its start), then the PR
//! mark goes, then the branch loses its end. The status is read on the git
//! thread with the directory and branch, never per frame.
//! A space in a linked worktree is
//! listed one cell further in, under its repository's main-checkout space,
//! or under a dim header with the repository's name when no space is in
//! the main checkout:
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
//! line is its dot, label and state, then its space, dim; labels and states
//! line up in columns across the section. Short of room the space goes
//! first (left out whole), then the state column is cut, then the label
//! column; the dot always shows. The agent's kind is left to the dashboard.
//! The current agent (the one in the focused terminal) has the current
//! space's background too. While the sidebar has the keyboard, the selected
//! space or agent is drawn reversed. Agents stay in space, tab and split
//! order: a state change never moves one. External sessions (agents outside
//! paddock's terminals) close AGENTS under their own header, dim, in label
//! order, in the same form without a space.

use super::{pr_style, StatusText, STATE_DOT, STATUS_SEP};
use crate::git::Status;
use crate::github::Pr;
use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    /// That branch's git status, once read: its uncommitted changes and
    /// commits ahead of and behind upstream, after the branch where they
    /// fit.
    pub status: Option<Status>,
    /// That branch's pull request, if `gh` gave one: marked after the
    /// branch where it fits.
    pub pr: Option<Pr>,
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

/// One agent (main agent of a session) as the sidebar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLine {
    pub label: String,
    pub state: State,
    /// The space it runs in, dim at the end of its line; `None` for an
    /// external session.
    pub space: Option<String>,
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
    /// An agent's line: dot, label, state and space.
    Agent(usize),
    Blank,
}

/// What a sidebar line picks: space `i` or agent `i`, as an index into the
/// spaces followed by the agents.
pub fn pick(line: Line, spaces: usize) -> Option<usize> {
    match line {
        Line::Space(i) | Line::SpacePlace(i) => Some(i),
        Line::Agent(i) => Some(spaces + i),
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
            out.push(Line::Header(EXTERNAL));
        }
        out.push(Line::Agent(i));
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
        // The cells after the indent, less the border and the last cell.
        columns: Columns::of(agents, usize::from(area.width.saturating_sub(2 + INDENT))),
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
    /// Where the parts of the agents' lines go.
    columns: Columns,
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
            Line::Agent(i) => self.agents[i].current,
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
                let x = indent(name_x, s.linked);
                // The branch's status at the end, where it fits whole two
                // cells or more after the name, cut to make room down to
                // its first `NAME_LEAST` cells. On the selected (reversed)
                // line it keeps no colour, which would turn into a
                // background there.
                let coloured = !base.add_modifier.contains(Modifier::REVERSED);
                let parts = s
                    .status
                    .as_ref()
                    .map(|st| status_parts(st, coloured))
                    .unwrap_or_default();
                let at = end.saturating_sub(cells(parts_width(&parts)));
                // The name's room: the cells before the status, less the
                // gap, which stays whatever the name (even an empty one).
                let room = usize::from(at.saturating_sub(x))
                    .checked_sub(STATUS_GAP)
                    .filter(|&r| !parts.is_empty() && r >= s.name.width().min(NAME_LEAST + 1));
                let Some(room) = room else {
                    put(x, &s.name, Style::default());
                    return;
                };
                put(x, &head(&s.name, room), Style::default());
                parts
                    .iter()
                    .fold(at, |at, (text, style)| put(at, text, *style));
            }
            Line::Agent(i) => {
                let a = &self.agents[i];
                // An external session is dim all along: dot, label, state.
                let faded = |style: Style| if a.external { style.patch(dim) } else { style };
                put(dot_x, STATE_DOT, faded(dot(a.state)));
                let Columns {
                    label,
                    state,
                    space,
                } = self.columns;
                put(name_x, &cut(&a.label, label), faded(Style::default()));
                let state_x = name_x.saturating_add(cells(label + AGENT_GAP));
                put(
                    state_x,
                    &cut(a.state.label(), state),
                    faded(highlight(a.state)),
                );
                // The space only whole, never cut.
                if let Some(name) = a.space.as_deref().filter(|n| n.width() <= space) {
                    put(state_x.saturating_add(cells(state + AGENT_GAP)), name, dim);
                }
            }
            Line::SpacePlace(i) => {
                let s = &self.spaces[i];
                let x = indent(name_x, s.linked);
                let room = usize::from(end.saturating_sub(x));
                let Some(path) = &s.path else {
                    return;
                };
                let branch = s.branch.as_deref();
                // The PR's mark at the end, two cells or more after the
                // place: short of room the path gives way first, down to
                // `…`, then the mark goes, then the branch is cut.
                let parts = s.pr.as_ref().map(mark_parts).unwrap_or_default();
                let width = parts_width(&parts);
                let place_room = room
                    .checked_sub(width + MARK_GAP)
                    .filter(|&r| !parts.is_empty() && r >= place_least(path, branch));
                let Some(place_room) = place_room else {
                    put(x, &place(path, branch, room), dim);
                    return;
                };
                put(x, &place(path, branch, place_room), dim);
                let at = end.saturating_sub(cells(width));
                parts
                    .iter()
                    .fold(at, |at, (text, style)| put(at, text, *style));
            }
            Line::Blank => {}
        }
    }
}

/// The fewest cells between a space's name and its status.
const STATUS_GAP: usize = 2;
/// The fewest cells between a space's place and its PR mark.
const MARK_GAP: usize = 2;
/// The fewest cells of its own a space's name keeps, before `…`, when cut
/// to make room for its status.
const NAME_LEAST: usize = 8;

/// A space's branch status as drawn after its name, piece by piece: its
/// line changes added (green) and removed (red), then its commits ahead of
/// and behind upstream (dim), as the dashboard writes them (see
/// [`StatusText`]); without colour unless `coloured`. Empty when there is
/// nothing to show.
fn status_parts(status: &Status, coloured: bool) -> Vec<(String, Style)> {
    let text = StatusText::of(status);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let colour = |c: Color| {
        if coloured {
            Style::default().fg(c)
        } else {
            Style::default()
        }
    };
    let mut parts = Vec::new();
    if let Some((added, removed)) = text.added_removed {
        parts.push((added, colour(Color::Green)));
        parts.push((STATUS_SEP.to_string(), dim));
        parts.push((removed, colour(Color::Red)));
    }
    if !text.arrows.is_empty() {
        if !parts.is_empty() {
            parts.push((STATUS_SEP.to_string(), dim));
        }
        parts.push((text.arrows, dim));
    }
    parts
}

/// A pull request's mark as drawn at the end of its space's second line:
/// its number in its state's colour, then its checks mark (dim), if any.
fn mark_parts(pr: &Pr) -> Vec<(String, Style)> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let checks = pr.checks.map(|c| (format!(" {}", c.mark()), dim));
    std::iter::once((format!("#{}", pr.number), pr_style(pr.state)))
        .chain(checks)
        .collect()
}

/// The cells `parts` take, side by side.
fn parts_width(parts: &[(String, Style)]) -> usize {
    parts.iter().map(|(t, _)| t.width()).sum()
}

/// The fewest cells [`place`] takes with the path cut down to `…` and the
/// branch whole.
fn place_least(path: &str, branch: Option<&str>) -> usize {
    path.width().min(1) + branch.map_or(0, |b| b.width() + 2)
}

/// `w` cells as a screen offset, saturating.
fn cells(w: usize) -> u16 {
    u16::try_from(w).unwrap_or(u16::MAX)
}

/// The fewest cells between an agent's label, state and space columns.
const AGENT_GAP: usize = 2;

/// The columns of the agents' lines, in cells, the same for every agent so
/// that labels and states line up: each as wide as its widest entry, where
/// it fits. Short of room the space goes first, then the state column is
/// cut, then the label column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    label: usize,
    state: usize,
    /// The room left for a space after the state column whole; none once
    /// the state column is cut.
    space: usize,
}

impl Columns {
    /// The columns for `agents` in `room` cells after the dot.
    fn of(agents: &[AgentLine], room: usize) -> Self {
        let widest = |w: fn(&AgentLine) -> usize| agents.iter().map(w).max().unwrap_or(0);
        let label = widest(|a| a.label.width()).min(room);
        let full_state = widest(|a| a.state.label().width());
        let state = full_state.min(room.saturating_sub(label + AGENT_GAP));
        let space = if state == full_state {
            room.saturating_sub(label + state + 2 * AGENT_GAP)
        } else {
            0
        };
        Columns {
            label,
            state,
            space,
        }
    }
}

/// `t` in at most `n` cells, as [`head`] cuts it; nothing in none.
fn cut(t: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    head(t, n)
}

/// The longest start of `t` at most `n` cells wide.
fn first_cells(t: &str, n: usize) -> String {
    let mut used = 0;
    t.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= n
        })
        .collect()
}

/// `t` in at most `n` cells: whole, else its start then `…`.
fn head(t: &str, n: usize) -> String {
    if t.width() <= n {
        return t.to_string();
    }
    format!("{}…", first_cells(t, n.saturating_sub(1)))
}

/// The longest end of `t` at most `n` cells wide.
fn last_cells(t: &str, n: usize) -> String {
    let mut used = 0;
    let mut kept: Vec<char> = t
        .chars()
        .rev()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= n
        })
        .collect();
    kept.reverse();
    kept.into_iter().collect()
}

/// A space's directory and branch in `width` cells: `path  branch`. Too
/// long, the path loses its start first (`…ject/api`), then the branch its
/// end (`feat/lo…`).
pub fn place(path: &str, branch: Option<&str>, width: usize) -> String {
    let tail = |t: &str, n: usize| -> String {
        if t.width() <= n {
            return t.to_string();
        }
        format!("…{}", last_cells(t, n.saturating_sub(1)))
    };
    let Some(branch) = branch else {
        return if width == 0 {
            String::new()
        } else {
            tail(path, width)
        };
    };
    let path_room = width.saturating_sub(branch.width() + 2);
    if path_room >= 1 {
        return format!("{}  {branch}", tail(path, path_room));
    }
    // Even `…` and the branch do not fit: keep `…` and cut the branch.
    let branch_room = width.saturating_sub(3);
    first_cells(&format!("…  {}", head(branch, branch_room)), width)
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
    use crate::github::{Checks, PrState};

    fn space(name: &str, state: Option<State>, current: bool) -> SpaceLine {
        SpaceLine {
            name: name.into(),
            state,
            current,
            path: Some(format!("~/p/{name}")),
            branch: None,
            status: None,
            pr: None,
            linked: false,
        }
    }

    fn entries(spaces: &[SpaceLine]) -> Vec<Entry> {
        spaces.iter().cloned().map(Entry::Space).collect()
    }

    fn agent(label: &str, state: State, space: &str) -> AgentLine {
        AgentLine {
            external: false,
            label: label.into(),
            state,
            space: Some(space.into()),
            current: false,
        }
    }

    fn external(label: &str, state: State) -> AgentLine {
        AgentLine {
            external: true,
            space: None,
            ..agent(label, state, "")
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
        draw_sized(entries, agents, selected, 24)
    }

    fn draw_sized(
        entries: &[Entry],
        agents: &[AgentLine],
        selected: Option<usize>,
        width: u16,
    ) -> Buffer {
        let area = Rect::new(0, 0, width, 12);
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
            agent("api", State::Waiting, "core"),
            agent("web", State::Running, "api-server"),
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
            " ● api  waiting  core  │",
            " ● web  running        │",
            "                       │",
        ];
        for (y, line) in want.iter().enumerate() {
            assert_eq!(text(&buf, y as u16), *line, "row {y}");
        }
        assert!(buf[(3, 2)].modifier.contains(Modifier::DIM), "path is dim");
        assert!(
            buf[(17, 7)].modifier.contains(Modifier::DIM),
            "space is dim"
        );
        assert!(!buf[(3, 7)].modifier.contains(Modifier::DIM), "label");
    }

    #[test]
    fn states_stand_out_and_the_current_space_has_its_own_background() {
        let spaces = [
            space("cur", Some(State::Idle), true),
            space("other", Some(State::Done), false),
        ];
        let agents = [agent("w", State::Waiting, "cur")];
        let buf = draw(&spaces, &agents, None);
        for (x, y) in [(0, 1), (22, 1), (0, 2), (22, 2)] {
            assert_eq!(buf[(x, y)].bg, GREY, "({x}, {y}): both lines");
        }
        assert_eq!(buf[(23, 1)].bg, Color::Reset, "not the border");
        assert_eq!(buf[(5, 3)].bg, Color::Reset);
        assert_eq!(buf[(1, 1)].fg, Color::Green, "idle dot");
        assert!(buf[(1, 3)].modifier.contains(Modifier::BOLD), "done dot");
        let waiting = &buf[(6, 7)];
        assert_eq!((waiting.fg, waiting.bg), (Color::Black, Color::Yellow));
        assert_eq!(buf[(1, 7)].fg, Color::Yellow);
    }

    #[test]
    fn the_current_agent_has_the_current_background_over_its_line() {
        let spaces = [space("a", None, false)];
        let agents = [
            agent("x", State::Idle, "a"),
            AgentLine {
                current: true,
                ..agent("y", State::Running, "a")
            },
        ];
        let buf = draw(&spaces, &agents, None);
        // 0 SPACES, 1-2 a, 3 blank, 4 AGENTS, 5 x, 6 y.
        for (x, y) in [(0, 6), (22, 6)] {
            assert_eq!(buf[(x, y)].bg, GREY, "({x}, {y}): its line");
        }
        assert_eq!(buf[(23, 6)].bg, Color::Reset, "not the border");
        assert_eq!(buf[(5, 7)].bg, Color::Reset, "not the line below");
        assert_eq!(buf[(5, 5)].bg, Color::Reset, "not another agent");
        assert_eq!(buf[(5, 1)].bg, Color::Reset, "no current space");
    }

    #[test]
    fn the_selected_space_or_agent_is_reversed() {
        let spaces = [space("a", None, false), space("b", None, false)];
        let agents = [agent("x", State::Idle, "a"), agent("y", State::Idle, "b")];
        let reversed = |buf: &Buffer, y| buf[(5, y)].modifier.contains(Modifier::REVERSED);
        let buf = draw(&spaces, &agents, Some(1));
        assert!(reversed(&buf, 3) && reversed(&buf, 4));
        assert!(!reversed(&buf, 1) && !reversed(&buf, 2));
        // 5 blank, 6 AGENTS, 7 x, 8 y.
        let buf = draw(&spaces, &agents, Some(3));
        assert!(reversed(&buf, 8));
        assert!(!reversed(&buf, 7) && !reversed(&buf, 9) && !reversed(&buf, 3));
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
        let agents = [agent("x", State::Waiting, "api/feat/x")];
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
            " ● x  waiting          │",
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
        // 10 agent 0, 11 agent 1.
        let two = entries(&[space("a", None, false), space("b", None, false)]);
        let at = |y| pick_at(&two, (2, 0), area, y);
        assert_eq!(at(3), None, "header");
        assert_eq!((at(4), at(5)), (Some(0), Some(0)));
        assert_eq!((at(6), at(7)), (Some(1), Some(1)));
        assert_eq!((at(8), at(9)), (None, None));
        assert_eq!((at(10), at(11)), (Some(2), Some(3)));
        assert_eq!(at(12), None, "below the last line");
        assert_eq!(at(20), None, "below the sidebar");
    }

    #[test]
    fn lines_give_each_agent_one_line_with_no_blank_between() {
        use Line::*;
        assert_eq!(
            lines(&entries(&[space("a", None, false)]), 3, 0),
            [
                Header(SPACES),
                Space(0),
                SpacePlace(0),
                Blank,
                Header(AGENTS),
                Agent(0),
                Agent(1),
                Agent(2),
            ]
        );
    }

    #[test]
    fn external_agents_close_agents_under_their_own_header() {
        use Line::*;
        let ent = entries(&[space("a", None, false)]);
        assert_eq!(
            lines(&ent, 2, 1)[3..],
            [Blank, Header(AGENTS), Agent(0), Header(EXTERNAL), Agent(1)]
        );
        assert_eq!(
            lines(&ent, 1, 1)[3..],
            [Blank, Header(AGENTS), Header(EXTERNAL), Agent(0)]
        );
        let area = Rect::new(0, 0, 24, 20);
        // 4 AGENTS, 5 api, 6 EXTERNAL, 7 web: the external header picks
        // nothing; its agent is pick 2.
        assert_eq!(pick_at(&ent, (2, 1), area, 6), None);
        assert_eq!(pick_at(&ent, (2, 1), area, 7), Some(2));
        let agents = [
            agent("api", State::Running, "a"),
            external("web", State::Waiting),
        ];
        let buf = draw(&[space("a", Some(State::Running), false)], &agents, None);
        let trimmed = |y| text(&buf, y).trim_end_matches([' ', '│']).to_string();
        assert_eq!(trimmed(5), " ● api  running  a");
        assert_eq!(trimmed(6), " EXTERNAL");
        assert_eq!(trimmed(7), " ● web  waiting", "no space");
        // The whole line is dim: dot, label and state word.
        for x in [1, 3, 8] {
            assert!(buf[(x, 7)].modifier.contains(Modifier::DIM), "dim at {x}");
            assert!(!buf[(x, 5)].modifier.contains(Modifier::DIM), "{x}");
        }
    }

    /// The AGENTS and EXTERNAL lines of `agents` drawn `width` wide, without
    /// trailing blanks or the border.
    fn agent_rows(agents: &[AgentLine], width: u16) -> Vec<String> {
        let buf = draw_sized(&[], agents, None, width);
        (3..buf.area.height)
            .map(|y| text(&buf, y).trim_end_matches([' ', '│']).to_string())
            .take(agents.len() + 1)
            .collect()
    }

    /// The ticket's agents: three in spaces, one external.
    fn four_agents() -> [AgentLine; 4] {
        [
            agent("PR-1434", State::Waiting, "core"),
            agent("api-server", State::Running, "api"),
            agent("paddock", State::Idle, "paddock"),
            external("web", State::Idle),
        ]
    }

    #[test]
    fn labels_and_states_line_up_in_columns_with_the_space_after() {
        assert_eq!(
            agent_rows(&four_agents(), 33),
            [
                " ● PR-1434     waiting  core",
                " ● api-server  running  api",
                " ● paddock     idle     paddock",
                " EXTERNAL",
                " ● web         idle",
            ]
        );
    }

    #[test]
    fn short_of_room_the_space_goes_then_the_state_then_the_label() {
        let rows = |width| agent_rows(&four_agents(), width);
        // The space goes whole, line by line: `paddock` first.
        assert_eq!(
            rows(32)[..3],
            [
                " ● PR-1434     waiting  core",
                " ● api-server  running  api",
                " ● paddock     idle",
            ]
        );
        assert_eq!(
            rows(26)[..3],
            [
                " ● PR-1434     waiting",
                " ● api-server  running",
                " ● paddock     idle",
            ]
        );
        // Then the state column is cut, then gone.
        assert_eq!(
            rows(22)[..3],
            [
                " ● PR-1434     wait…",
                " ● api-server  runn…",
                " ● paddock     idle",
            ]
        );
        assert_eq!(rows(15)[..3], [" ● PR-1434", " ● api-server", " ● paddock"]);
        // Then the label; the dot always shows.
        assert_eq!(
            rows(10),
            [" ● PR-1…", " ● api-…", " ● padd…", " EXTERNAL", " ● web"]
        );
        assert_eq!(rows(5)[..3], [" ●", " ●", " ●"]);
    }

    /// A space in `path` on `branch`, with `pr`.
    fn on(path: &str, branch: &str, pr: Option<Pr>) -> SpaceLine {
        SpaceLine {
            path: Some(path.into()),
            branch: Some(branch.into()),
            pr,
            ..space("s", None, false)
        }
    }

    #[test]
    fn a_spaces_pr_is_marked_after_its_branch_only_where_it_fits() {
        let open = Some(Pr::sample(123, PrState::Open, Some(Checks::Passing), None));
        let spaces = [
            on("~/r", "feat/x", open.clone()),
            // The whole place fills the line: the path gives way.
            on("~/project/api", "main", open.clone()),
            on("~/a/very/long/path", "feat/y", open),
            on("~/r", "main", None),
        ];
        let buf = draw(&spaces, &[], None);
        assert_eq!(text(&buf, 2), "   ~/r  feat/x  #123 ✓ │");
        assert_eq!(text(&buf, 4), "   …/api  main  #123 ✓ │");
        assert_eq!(text(&buf, 6), "   …th  feat/y  #123 ✓ │");
        assert_eq!(text(&buf, 8), "   ~/r  main           │", "no PR: nothing");
        // The number in its state's colour, the checks mark dim.
        assert_eq!(buf[(16, 2)].fg, Color::Green);
        assert!(!buf[(16, 2)].modifier.contains(Modifier::DIM));
        assert!(buf[(21, 2)].modifier.contains(Modifier::DIM));
    }

    /// The lines of one space `s` at row `y` (1 its name, 2 its place),
    /// drawn `width` cells wide (border included), selected if `selected`,
    /// their end trimmed, and the buffer.
    fn space_lines(s: SpaceLine, width: u16, selected: bool) -> ([String; 2], Buffer) {
        let area = Rect::new(0, 0, width, 3);
        let mut buf = Buffer::empty(area);
        let selected = selected.then_some(0);
        render(&entries(&[s]), &[], selected, GREY, area, &mut buf);
        let line = |y| text(&buf, y).trim_end_matches([' ', '│']).to_string();
        ([line(1), line(2)], buf)
    }

    /// The second line of one space `s`, drawn `width` cells wide.
    fn second_line(s: SpaceLine, width: u16) -> String {
        let [_, place] = space_lines(s, width, false).0;
        place
    }

    /// The first line of one space `s`, drawn `width` cells wide, and the
    /// buffer.
    fn first_line(s: SpaceLine, width: u16) -> (String, Buffer) {
        let ([name, _], buf) = space_lines(s, width, false);
        (name, buf)
    }

    /// A space named `name` with `changes` and `ahead_behind`, in `~/r` on
    /// `main`.
    fn with_status(
        name: &str,
        changes: Option<(u32, u32)>,
        ahead_behind: Option<(u32, u32)>,
    ) -> SpaceLine {
        SpaceLine {
            name: name.into(),
            status: Some(Status {
                ahead_behind,
                changes,
            }),
            ..on("~/r", "main", None)
        }
    }

    #[test]
    fn a_spaces_status_ends_its_first_line_at_the_default_width() {
        let width = crate::config::DEFAULT_SIDEBAR_WIDTH;
        let arx = SpaceLine {
            name: "arx-platform-core".into(),
            status: Some(Status {
                ahead_behind: Some((0, 15)),
                changes: None,
            }),
            ..on("~/project/arx-platform-core", "develop", None)
        };
        let ([name, place], _) = space_lines(arx.clone(), width, false);
        assert_eq!(name, format!("   arx-platform-core{}↓15", " ".repeat(7)));
        // The second line keeps the place alone.
        assert_eq!(place, "   …arx-platform-core  develop");
        let paddock = with_status("paddock", Some((200, 10)), Some((2, 1)));
        let (name, buf) = first_line(paddock.clone(), width);
        assert_eq!(name, format!("   paddock{}+200 −10 ↑2↓1", " ".repeat(7)));
        // `+200` green, `−10` red, the arrows dim, on the line's style.
        let at = |x: u16| &buf[(x, 1)];
        assert_eq!((at(17).symbol(), at(17).fg), ("+", Color::Green));
        assert_eq!((at(22).symbol(), at(22).fg), ("−", Color::Red));
        assert_eq!(at(26).symbol(), "↑");
        assert!(at(26).modifier.contains(Modifier::DIM));
        assert!(!at(17).modifier.contains(Modifier::DIM));
        let current = SpaceLine {
            current: true,
            ..paddock
        };
        let (_, buf) = first_line(current, width);
        assert_eq!(buf[(17, 1)].bg, GREY, "the current background stays");
        // A worktree space, a cell further in, ends at the same edge.
        let linked = SpaceLine {
            linked: true,
            ..arx
        };
        let (name, _) = first_line(linked, width);
        assert_eq!(name, format!("    arx-platform-core{}↓15", " ".repeat(6)));
    }

    #[test]
    fn a_long_name_is_cut_for_the_status_down_to_its_first_eight_cells() {
        let s = with_status("a-very-long-space-name", Some((200, 10)), Some((2, 1)));
        let line = |width| first_line(s.clone(), width).0;
        let status = "  +200 −10 ↑2↓1";
        assert_eq!(line(42), format!("   a-very-long-space-name{status}"));
        assert_eq!(line(41), format!("   a-very-long-space-na…{status}"));
        assert_eq!(line(32), format!("   a-very-long…{status}"));
        assert_eq!(line(29), format!("   a-very-l…{status}"));
        // Shorter still: the status goes whole, never cut; the name stays.
        assert_eq!(line(28), "   a-very-long-space-name");
        // A short name is never cut: the status needs its whole width.
        let short = with_status("api", Some((200, 10)), Some((2, 1)));
        let line = |width| first_line(short.clone(), width).0;
        assert_eq!(line(23), format!("   api{status}"));
        assert_eq!(line(22), "   api");
        // An empty name keeps the gap all the same.
        let empty = with_status("", Some((200, 10)), Some((2, 1)));
        let line = |width| first_line(empty.clone(), width).0;
        assert_eq!(line(20), format!("     {}", status.trim_start()));
        assert_eq!(line(19), "");
        for width in 0..45 {
            let got = line(width);
            assert!(!got.contains('+') || got.contains(status), "{width}: {got}");
        }
    }

    #[test]
    fn a_selected_spaces_status_has_no_colour_to_turn_into_a_background() {
        let s = with_status("r", Some((3, 1)), None);
        let ([name, place], buf) = space_lines(s, 30, true);
        assert_eq!(name, format!("   r{}+3 −1", " ".repeat(19)));
        assert_eq!(place, "   ~/r  main", "the status is on the first line");
        for x in [23, 26] {
            let cell = &buf[(x, 1)];
            assert!(cell.modifier.contains(Modifier::REVERSED), "selected");
            assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{x}");
        }
    }

    #[test]
    fn a_spaces_status_shows_only_what_is_not_zero() {
        let line = |s| {
            let (name, _) = first_line(s, 30);
            name.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        assert_eq!(line(with_status("s", None, Some((0, 3)))), "s ↓3");
        assert_eq!(line(with_status("s", Some((5, 0)), None)), "s +5 −0");
        // Clean and even with upstream, or a detached HEAD (nothing read):
        // nothing after the name.
        assert_eq!(line(with_status("s", Some((0, 0)), Some((0, 0)))), "s");
        assert_eq!(line(with_status("s", None, None)), "s");
    }

    #[test]
    fn the_path_gives_way_to_the_pr_mark_then_the_mark_to_the_branch() {
        let pr = Some(Pr::sample(42, PrState::Open, Some(Checks::Passing), None));
        let s = on("~/p/paddock", "feat/x", pr);
        let line = |width| second_line(s.clone(), width);
        assert_eq!(line(31), "   ~/p/paddock  feat/x  #42 ✓");
        assert_eq!(line(30), "   …p/paddock  feat/x  #42 ✓");
        assert_eq!(line(21), "   …  feat/x  #42 ✓");
        // Short of room for the mark: it goes, the path back as long as it
        // fits, then the branch loses its end.
        assert_eq!(line(20), "   …addock  feat/x");
        assert_eq!(line(14), "   …  feat/x");
        assert_eq!(line(13), "   …  feat…");
    }

    #[test]
    fn a_prs_number_is_coloured_by_its_state() {
        let mark = |state, checks| {
            let buf = draw(
                &[on("~/r", "x", Some(Pr::sample(7, state, checks, None)))],
                &[],
                None,
            );
            let at = (0..23)
                .find(|&x| buf[(x, 2)].symbol() == "#")
                .expect("a mark");
            let line = text(&buf, 2);
            (
                line.trim_end_matches([' ', '│']).to_string(),
                buf[(at, 2)].clone(),
            )
        };
        let (line, cell) = mark(PrState::Open, Some(Checks::Failing));
        assert_eq!(line, format!("   ~/r  x{}#7 ✗", " ".repeat(9)));
        assert_eq!(cell.fg, Color::Green);
        let (line, cell) = mark(PrState::Merged, Some(Checks::Pending));
        assert_eq!(line, format!("   ~/r  x{}#7 •", " ".repeat(9)));
        assert_eq!(cell.fg, Color::Magenta);
        let (line, cell) = mark(PrState::Closed, None);
        assert_eq!(
            line,
            format!("   ~/r  x{}#7", " ".repeat(11)),
            "no checks: alone"
        );
        assert_eq!(cell.fg, Color::Red);
        let (_, cell) = mark(PrState::Draft, None);
        assert_eq!(cell.fg, Color::Reset);
        assert!(cell.modifier.contains(Modifier::DIM), "draft is dim");
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
            assert!(got.width() <= width, "{width}: {got:?}");
            // Wide characters count by the cells they take.
            let wide = place("~/專案/前端", Some("功能/登入"), width);
            assert!(wide.width() <= width, "{width}: {wide:?}");
        }
        assert_eq!(place("~/專案/前端", Some("x"), 10), "…/前端  x");
    }

    #[test]
    fn a_place_keeps_at_least_a_cell_of_path_and_its_branch_whole() {
        assert_eq!(place_least("~/專案", Some("x")), 4);
        assert_eq!(place_least("", None), 0);
    }
}
