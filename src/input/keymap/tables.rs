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

/// What a key does while the sidebar has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarAction {
    Down,
    Up,
    Open,
    Close,
    Fork,
    Back,
}

/// What a key does while the dashboard is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardAction {
    Down,
    Up,
    Open,
    Dismiss,
    Close,
    Fork,
    Interact,
    Back,
}

/// Keys after the prefix (`[keys.commands]`). `go_to_tab`'s Nth key goes to tab
/// N: its action is completed with the key's index.
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
        "focus sidebar on current agent",
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
        "next session needing you",
    ),
    bind("dashboard", Command::Dashboard, &["o"], "dashboard"),
    bind(
        "detach",
        Command::Detach,
        &["d"],
        "detach (terminals go on)",
    ),
    bind("help", Command::Help, &["?"], "this help"),
];

/// Keys while the sidebar has the keyboard (`[keys.sidebar]`).
pub const SIDEBAR: &[Binding<SidebarAction>] = &[
    bind(
        "down",
        SidebarAction::Down,
        &["j", "Down"],
        "next space or agent",
    ),
    bind(
        "up",
        SidebarAction::Up,
        &["k", "Up"],
        "previous space or agent",
    ),
    bind(
        "open",
        SidebarAction::Open,
        &["Enter"],
        "show the space's tabs, or jump to the agent's pane",
    ),
    bind(
        "close",
        SidebarAction::Close,
        &["x"],
        "close the space, or the agent's pane",
    ),
    bind(
        "fork",
        SidebarAction::Fork,
        &["f"],
        "fork the agent (on a space, its agent that needs you most)",
    ),
    bind(
        "back",
        SidebarAction::Back,
        &["Esc"],
        "give the keyboard back",
    ),
];

/// Keys while the dashboard is open (`[keys.dashboard]`).
pub const DASHBOARD: &[Binding<DashboardAction>] = &[
    bind("down", DashboardAction::Down, &["j", "Down"], "next row"),
    bind("up", DashboardAction::Up, &["k", "Up"], "previous row"),
    bind(
        "open",
        DashboardAction::Open,
        &["Enter"],
        "jump to the row's terminal",
    ),
    bind(
        "dismiss",
        DashboardAction::Dismiss,
        &["D"],
        "dismiss the row",
    ),
    bind(
        "close",
        DashboardAction::Close,
        &["x"],
        "close the row's terminal",
    ),
    bind(
        "fork",
        DashboardAction::Fork,
        &["f"],
        "fork the row's Claude session",
    ),
    bind(
        "interact",
        DashboardAction::Interact,
        &["I"],
        "type into the previewed terminal (Esc leaves)",
    ),
    bind(
        "back",
        DashboardAction::Back,
        &["Esc"],
        "close the dashboard",
    ),
];
