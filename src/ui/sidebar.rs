//! The sidebar, with a border on its right edge: a SPACES section with one
//! line per space (tmux session, ADR 0006), then an AGENTS section with two
//! lines per main agent session, across every space:
//!
//! ```text
//!  SPACES
//!  ● api-server
//!    ~/project/api  main
//!    paddock
//!    ~/p/paddock  feat/x  +200 −10 ↑2↓1  #42 ✓
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
//! branch checked out there. After both, uncut, come the branch's status
//! (as the dashboard writes it: uncommitted lines `+200` in green and `−10`
//! in red, then commits ahead of and behind upstream `↑2↓1`, dim; parts at
//! zero left out, nothing for a detached HEAD or outside a repository),
//! then, at the end, the branch's pull request: its number in its state's
//! colour and its checks mark (`✓`, `✗`, `•`). Each shows only where it
//! fits whole; short of room for both, the PR mark goes first, so the
//! diff stays (the mark still shows alone when only it fits). The status
//! is read on the git thread with the directory and branch, never per
//! frame. A space in a linked worktree is
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
//! second line is dim and names its space and tab; the current agent (the
//! one in the focused terminal) has the current space's background too.
//! While the sidebar has the keyboard, the selected space or agent is drawn
//! reversed. Agents stay in space, tab and split order: a state change never
//! moves one. External sessions (agents outside paddock's terminals) close
//! AGENTS under their own header, dim, in label order, their second line
//! naming their branch (else their directory).

use super::{pr_style, StatusText, STATE_DOT};
use crate::git::Status;
use crate::github::Pr;
use crate::sessions::{AgentKind, State};
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
    /// Its kind, tagged at the end of its second line.
    pub kind: AgentKind,
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
    /// An agent's second line: its space and tab, and its kind.
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
                // An external session is dim all along: dot, label, state.
                let faded = |style: Style| if a.external { style.patch(dim) } else { style };
                put(dot_x, STATE_DOT, faded(dot(a.state)));
                let state = a.state.label();
                let width = u16::try_from(state.chars().count()).unwrap_or(0);
                let state_x = end.saturating_sub(width).max(name_x);
                let room = usize::from(state_x.saturating_sub(name_x + 1));
                let label: String = a.label.chars().take(room).collect();
                put(name_x, &label, faded(Style::default()));
                put(state_x, state, faded(highlight(a.state)));
            }
            Line::SpacePlace(i) => {
                let s = &self.spaces[i];
                let x = indent(name_x, s.linked);
                let room = usize::from(end.saturating_sub(x));
                let Some(path) = &s.path else {
                    return;
                };
                let branch = s.branch.as_deref();
                put(x, &place(path, branch, room), dim);
                // After the whole place (the directory and branch are never
                // cut for them): the branch's status, where it fits whole,
                // then the PR's mark at the end, where it fits after that.
                // Short of room for both, the mark goes first.
                let mut used = place_width(path, branch);
                let parts = s.status.as_ref().map(status_parts).unwrap_or_default();
                let width: usize = parts.iter().map(|(t, _)| t.width()).sum();
                if !parts.is_empty() && used + PART_GAP.width() + width <= room {
                    let mut at = x.saturating_add(cells(used));
                    at = put(at, PART_GAP, dim);
                    for (text, style) in &parts {
                        at = put(at, text, *style);
                    }
                    used += PART_GAP.width() + width;
                }
                let Some(pr) = &s.pr else {
                    return;
                };
                let number = format!("#{}", pr.number);
                let checks = pr.checks.map(|c| format!(" {}", c.mark()));
                let mark = number.width() + checks.as_deref().map_or(0, str::width);
                if let Some(at) = end_x(x, used, mark, end) {
                    let at = put(at, &number, pr_style(pr.state));
                    if let Some(checks) = &checks {
                        put(at, checks, dim);
                    }
                }
            }
            Line::AgentPlace(i) => {
                // The kind's tag under the state, only where it fits after
                // the whole place: the place is never cut for it.
                let a = &self.agents[i];
                put(name_x, &a.place, dim);
                let tag = a.kind.tag();
                if let Some(at) = end_x(name_x, a.place.width(), tag.width(), end) {
                    put(at, tag, dim);
                }
            }
            Line::Blank => {}
        }
    }
}

/// What goes between a space's branch and its status.
const PART_GAP: &str = "  ";

/// A space's branch status as drawn after its branch, piece by piece: its
/// uncommitted lines added (green) and removed (red), then its commits
/// ahead of and behind upstream (dim), as the dashboard writes them (see
/// [`StatusText`]). Empty when there is nothing to show.
fn status_parts(status: &Status) -> Vec<(String, Style)> {
    let text = StatusText::of(status);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut parts = Vec::new();
    if let Some((added, removed)) = text.changes {
        parts.push((added, Style::default().fg(Color::Green)));
        parts.push((" ".to_string(), dim));
        parts.push((removed, Style::default().fg(Color::Red)));
    }
    if !text.arrows.is_empty() {
        if !parts.is_empty() {
            parts.push((" ".to_string(), dim));
        }
        parts.push((text.arrows, dim));
    }
    parts
}

/// `w` cells as a screen offset, saturating.
fn cells(w: usize) -> u16 {
    u16::try_from(w).unwrap_or(u16::MAX)
}

/// Where something `width` cells wide (a kind tag, a PR mark) starts so
/// that it ends at `end`, if it fits after `text` cells of text drawn from
/// `x`, a cell or more apart.
/// The text is never cut for it.
fn end_x(x: u16, text: usize, width: usize, end: u16) -> Option<u16> {
    let at = end.saturating_sub(cells(width));
    (at > x.saturating_add(cells(text))).then_some(at)
}

/// The cells `path  branch` takes uncut (see [`place`]).
fn place_width(path: &str, branch: Option<&str>) -> usize {
    path.width() + branch.map_or(0, |b| b.width() + 2)
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
    let head = |t: &str, n: usize| -> String {
        if t.width() <= n {
            return t.to_string();
        }
        format!("{}…", first_cells(t, n.saturating_sub(1)))
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

    fn agent(label: &str, state: State, place: &str) -> AgentLine {
        AgentLine {
            external: false,
            label: label.into(),
            state,
            kind: AgentKind::Claude,
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
            agent("api", State::Waiting, "api-server · server"),
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
            "   api-server · server │",
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
        // The whole line is dim: dot, label and state word.
        for x in [1, 3, 16] {
            assert!(buf[(x, 9)].modifier.contains(Modifier::DIM), "dim at {x}");
            assert!(!buf[(x, 5)].modifier.contains(Modifier::DIM), "{x}");
        }
        assert_eq!(
            text(&buf, 10).trim_end_matches([' ', '│']),
            "   feat/x       claude"
        );
        assert!(buf[(3, 10)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn an_agents_kind_is_tagged_under_its_state_only_where_its_place_fits() {
        let agents = [
            agent("api", State::Waiting, "api · x"),
            agent("web", State::Idle, "a-long-space-name · tab"),
        ];
        let buf = draw(&[], &agents, None);
        let top = (0..buf.area.height)
            .find(|&y| text(&buf, y).starts_with(" AGENTS"))
            .expect("an AGENTS header");
        let want = [
            " AGENTS                │",
            " ● api         waiting │",
            "   api · x      claude │",
            "                       │",
            " ● web            idle │",
            "   a-long-space-name · │",
        ];
        for (y, line) in want.iter().enumerate() {
            assert_eq!(text(&buf, top + y as u16), *line, "row {y}");
        }
        assert!(
            buf[(17, top + 2)].modifier.contains(Modifier::DIM),
            "dim tag"
        );
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
            // The whole place fills the line: no mark, nothing cut.
            on("~/project/api", "main", open.clone()),
            // Too long already: cut as before, never for the mark.
            on("~/a/very/long/path", "feat/y", open),
            on("~/r", "main", None),
        ];
        let buf = draw(&spaces, &[], None);
        assert_eq!(text(&buf, 2), "   ~/r  feat/x  #123 ✓ │");
        assert_eq!(text(&buf, 4), "   ~/project/api  main │");
        assert_eq!(text(&buf, 6), "   …/long/path  feat/y │");
        assert_eq!(text(&buf, 8), "   ~/r  main           │", "no PR: nothing");
        // The number in its state's colour, the checks mark dim.
        assert_eq!(buf[(16, 2)].fg, Color::Green);
        assert!(!buf[(16, 2)].modifier.contains(Modifier::DIM));
        assert!(buf[(21, 2)].modifier.contains(Modifier::DIM));
    }

    /// The second line of one space `s`, drawn `width` cells wide (border
    /// included), its end trimmed, and the buffer.
    fn second_line(s: SpaceLine, width: u16) -> (String, Buffer) {
        let area = Rect::new(0, 0, width, 3);
        let mut buf = Buffer::empty(area);
        render(&entries(&[s]), &[], None, GREY, area, &mut buf);
        let line = text(&buf, 2).trim_end_matches([' ', '│']).to_string();
        (line, buf)
    }

    #[test]
    fn a_spaces_status_follows_its_branch_and_the_pr_mark_drops_first() {
        let status = Status {
            ahead_behind: Some((2, 1)),
            changes: Some((200, 10)),
        };
        let pr = Some(Pr::sample(42, PrState::Open, Some(Checks::Passing), None));
        let s = SpaceLine {
            status: Some(status),
            ..on("~/p/paddock", "feat/x", pr)
        };
        let line = |width| second_line(s.clone(), width).0;
        let place = "   ~/p/paddock  feat/x";
        let diff = "  +200 −10 ↑2↓1";
        // Room for all: the diff after the branch, the mark at the end.
        assert_eq!(line(45), format!("{place}{diff} #42 ✓"));
        assert_eq!(line(50), format!("{place}{diff}{}#42 ✓", " ".repeat(6)));
        // A cell short for the mark: the mark goes, the diff stays.
        assert_eq!(line(44), format!("{place}{diff}"));
        assert_eq!(line(39), format!("{place}{diff}"));
        // Short of room for the diff whole: it goes, the mark fits alone.
        assert_eq!(line(38), format!("{place}{}#42 ✓", " ".repeat(9)));
        assert_eq!(line(30), format!("{place} #42 ✓"));
        // Neither: the place alone, uncut.
        assert_eq!(line(29), place);
        let (_, buf) = second_line(s.clone(), 45);
        // `+200` green, `−10` red, the arrows dim, on the line's style.
        let at = |x: u16| &buf[(x, 2)];
        assert_eq!((at(24).symbol(), at(24).fg), ("+", Color::Green));
        assert_eq!((at(29).symbol(), at(29).fg), ("−", Color::Red));
        assert_eq!(at(33).symbol(), "↑");
        assert!(at(33).modifier.contains(Modifier::DIM));
        assert!(!at(24).modifier.contains(Modifier::DIM));
        let current = SpaceLine {
            current: true,
            ..s.clone()
        };
        let (_, buf) = second_line(current, 45);
        assert_eq!(buf[(24, 2)].bg, GREY, "the current background stays");
    }

    #[test]
    fn a_spaces_status_shows_only_what_is_not_zero() {
        let with = |ahead_behind, changes| SpaceLine {
            status: Some(Status {
                ahead_behind,
                changes,
            }),
            ..on("~/r", "main", None)
        };
        let line = |s| second_line(s, 30).0;
        assert_eq!(line(with(Some((0, 3)), None)), "   ~/r  main  ↓3");
        assert_eq!(line(with(None, Some((5, 0)))), "   ~/r  main  +5 −0");
        // Clean and even with upstream, or a detached HEAD (nothing read):
        // nothing after the branch.
        assert_eq!(line(with(Some((0, 0)), Some((0, 0)))), "   ~/r  main");
        assert_eq!(line(with(None, None)), "   ~/r  main");
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
    fn a_tag_or_mark_goes_at_the_end_only_a_cell_or_more_after_the_text() {
        assert_eq!(end_x(3, 5, 6, 22), Some(16));
        assert_eq!(end_x(3, 12, 6, 22), Some(16), "one cell apart");
        assert_eq!(end_x(3, 13, 6, 22), None, "touching");
        assert_eq!(end_x(3, 0, 30, 22), None, "wider than the line");
        assert_eq!(place_width("~/專案", Some("x")), 9);
    }
}
