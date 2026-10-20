//! The actions, by section, with their names and default keys.

use crate::input::prefix::{Command, Direction, SIDEBAR_STEP};

/// An action as bound: its name in `config.toml`, what it does, its default
/// keys, and a line for the help.
#[derive(Debug, PartialEq, Eq)]
pub struct Binding<A: 'static> {
    pub name: &'static str,
    pub action: A,
    pub keys: &'static [&'static str],
    pub help: &'static str,
}

const fn bind<A>(
    name: &'static str,
    action: A,
    keys: &'static [&'static str],
    help: &'static str,
) -> Binding<A> {
    Binding {
        name,
        action,
        keys,
        help,
    }
}

/// What a bound key does.
pub trait Action: Copy + PartialEq + std::fmt::Debug + 'static {
    /// The action for the `i`th key of its binding: all the same, except
    /// `go_to_tab`, whose `i`th key goes to tab `i + 1`.
    fn nth(self, _i: usize) -> Self {
        self
    }
}

impl Action for Command {
    fn nth(self, i: usize) -> Self {
        match self {
            Command::GoToTab(_) => Command::GoToTab(i),
            other => other,
        }
    }
}

/// What a key does while the sidebar or the dashboard has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Down,
    Up,
    Open,
    Dismiss,
    Close,
    Fork,
    Interact,
    Back,
}

impl Action for Panel {}

/// Keys after the prefix (`[keys.commands]`).
pub const COMMANDS: &[Binding<Command>] = &[
    bind("new_tab", Command::NewTab, &["c"], "new tab in this space"),
    bind("new_space", Command::NewSpace, &["C"], "new space"),
    bind("next_tab", Command::NextTab, &["n"], "next tab"),
    bind("previous_tab", Command::PreviousTab, &["p"], "previous tab"),
    bind(
        "last_page",
        Command::LastPage,
        &["Tab"],
        "last space and tab shown",
    ),
    bind(
        "go_to_tab",
        Command::GoToTab(0),
        &["1", "2", "3", "4", "5", "6", "7", "8", "9"],
        "go to tab 1, 2, ... 9",
    ),
    bind("rename_tab", Command::RenameTab, &[","], "rename tab"),
    bind("rename_space", Command::RenameSpace, &["$"], "rename space"),
    bind(
        "move_tab_left",
        Command::MoveTab(-1),
        &["<"],
        "move tab left",
    ),
    bind(
        "move_tab_right",
        Command::MoveTab(1),
        &[">"],
        "move tab right",
    ),
    bind(
        "split_right",
        Command::SplitColumns,
        &["|"],
        "split side by side",
    ),
    bind(
        "split_down",
        Command::SplitRows,
        &["-"],
        "split top and bottom",
    ),
    bind(
        "focus_left",
        Command::Focus(Direction::Left),
        &["h", "Left"],
        "focus left",
    ),
    bind(
        "focus_down",
        Command::Focus(Direction::Down),
        &["j", "Down"],
        "focus down",
    ),
    bind(
        "focus_up",
        Command::Focus(Direction::Up),
        &["k", "Up"],
        "focus up",
    ),
    bind(
        "focus_right",
        Command::Focus(Direction::Right),
        &["l", "Right"],
        "focus right",
    ),
    bind(
        "equalize",
        Command::Equalise,
        &["="],
        "even out the focused split",
    ),
    bind("close", Command::Close, &["x"], "close pane (asks if busy)"),
    bind("fork", Command::Fork, &["f"], "fork Claude into a new tab"),
    bind(
        "focus_sidebar",
        Command::FocusSidebar,
        &["s"],
        "focus sidebar on agents",
    ),
    bind(
        "focus_spaces",
        Command::FocusSpaces,
        &["w"],
        "focus sidebar on the space",
    ),
    bind(
        "toggle_sidebar",
        Command::ToggleSidebar,
        &["b"],
        "show or hide sidebar",
    ),
    bind(
        "sidebar_narrower",
        Command::ResizeSidebar(-SIDEBAR_STEP),
        &["{"],
        "sidebar narrower",
    ),
    bind(
        "sidebar_wider",
        Command::ResizeSidebar(SIDEBAR_STEP),
        &["}"],
        "sidebar wider",
    ),
    bind(
        "next_attention",
        Command::NextAttention,
        &["a"],
        "next session that needs you",
    ),
    bind("dashboard", Command::Dashboard, &["o"], "dashboard"),
    bind(
        "detach",
        Command::Detach,
        &["d"],
        "detach: terminals keep running",
    ),
    bind("help", Command::Help, &["?"], "this help"),
];

/// Keys while the sidebar has the keyboard (`[keys.sidebar]`).
pub const SIDEBAR: &[Binding<Panel>] = &[
    bind("down", Panel::Down, &["j", "Down"], "next space or agent"),
    bind("up", Panel::Up, &["k", "Up"], "previous space or agent"),
    bind(
        "open",
        Panel::Open,
        &["Enter"],
        "show the space's tabs, or jump to the agent's pane",
    ),
    bind(
        "close",
        Panel::Close,
        &["x"],
        "close the space, or the agent's pane",
    ),
    bind(
        "fork",
        Panel::Fork,
        &["f"],
        "fork the agent (on a space, its agent that needs you most)",
    ),
    bind("back", Panel::Back, &["Esc"], "give the keyboard back"),
];

/// Keys while the dashboard is open (`[keys.dashboard]`).
pub const DASHBOARD: &[Binding<Panel>] = &[
    bind("down", Panel::Down, &["j", "Down"], "next row"),
    bind("up", Panel::Up, &["k", "Up"], "previous row"),
    bind(
        "open",
        Panel::Open,
        &["Enter"],
        "jump to the row's terminal",
    ),
    bind("dismiss", Panel::Dismiss, &["D"], "dismiss the row"),
    bind("close", Panel::Close, &["x"], "close the row's terminal"),
    bind("fork", Panel::Fork, &["f"], "fork the row's Claude session"),
    bind(
        "interact",
        Panel::Interact,
        &["I"],
        "type into the previewed terminal (Esc leaves)",
    ),
    bind("back", Panel::Back, &["Esc"], "close the dashboard"),
];
