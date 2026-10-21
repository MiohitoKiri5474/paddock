# paddock design

Agreed on 2026-10-19 in a design interview. Vocabulary is defined in [CONTEXT.md](../CONTEXT.md); decisions with lasting trade-offs are recorded in [docs/adr/](adr/).

## Purpose

paddock replaces herdr for one person on one Mac. It is a terminal workspace (spaces, tabs, splits) whose terminals keep running when paddock closes, and it tracks Claude Code sessions and the branches they and their subagents work on, so you can see at a glance which ones need you.

Not in scope for now: agents other than Claude Code, Linux, remote machines, a scripting API, and creating or deleting git worktrees.

## Architecture

- **One binary, no daemon** ([ADR 0003](adr/0003-no-daemon-single-instance.md)). `paddock` opens the app. While it is closed, only two things run: the hidden tmux server and Claude Code hooks.
- **Hidden tmux server** ([ADR 0001](adr/0001-tmux-as-hidden-backend.md)). `tmux -L paddock`, started with paddock's own minimal config (`escape-time 0`, `extended-keys on`, no status line, no prefix) and a clean environment (`SUPACODE_*`, `TERM_PROGRAM` and similar variables removed). The user's own tmux is never touched. Each space is a tmux session, its tabs the session's windows and its splits their panes ([ADR 0006](adr/0006-sidebar-items-are-tmux-sessions.md)), so `tmux -L paddock attach` shows the same structure. Floats are windows of the session too, marked as not being tabs, so `tmux attach` shows them as well.
- **Own renderer** ([ADR 0002](adr/0002-own-renderer-via-control-mode.md)). paddock is a tmux control-mode client, one per space. Each tmux pane's raw output feeds its own terminal-emulator instance; paddock draws everything with ratatui and keeps scrollback, selection and search itself. Spike 0a picks the emulator crate. Written in Rust.
- **One instance at a time.** A second launch offers to take over from the first.
- **State** lives in `~/.local/state/paddock/`: the current item (tmux holds tabs, splits, sizes and focus), dismissed rows, hook events and logs. **Config** lives in `~/.config/paddock/config.toml`: prefix, keys, colours, the sidebar width, floats and which notifications to send.
- **Hooks.** `paddock hooks install` and `paddock hooks uninstall` edit `~/.claude/settings.json`. Supacode's hooks stay; the old tmux-claude-session-manager hooks are removed. Every hook calls `paddock hook <event>`, which finds its terminal through `$TMUX_PANE` and appends the event to the state directory, so tasks created while paddock is closed still appear. `claude agents --json` is used to discover sessions and reconcile state.
- **Commands.** `paddock`, `paddock keys` (prints the default keys as TOML), `paddock hooks install|uninstall`, and the internal `paddock hook <event>`.
- **Any terminal app.** Only standard VT features; truecolor with a 256-colour fallback; the UI uses the terminal's 16 ANSI colours so it follows the user's theme; copying goes through `pbcopy` and notifications through `osascript`; no key depends on Option/Alt. paddock makes no network calls.
- **Refresh.** Git info refreshes on hook events and every few seconds for the dashboard's rows while it is open.

## Interface

### Layout

- Left: the closable, resizable **sidebar** (32 columns, or `[sidebar] width`; `C-a {` / `C-a }` or dragging its border change it by 2 columns between 16 and half the screen, and the last width is kept in the state directory), in two sections (after herdr):

  ```text
   SPACES
   ● api-server
     ~/project/api  main
     paddock
     ~/project/paddock  feat/x

   AGENTS
   ● api-server  waiting
     api-server · claude
  ```

  SPACES has two lines per space: a dot in the most urgent state of its agents (none without agents) and its name (the tmux session name), then, dim, its directory (that of its first tab's first pane, home as `~`) and the branch checked out there (the short commit if detached, nothing outside a repository). A path too long loses its start (`…`) first, then the branch its end. Directories and branches are read on the git thread, at start, on hook events and every few seconds; the second line stays empty until the first read. The current space (the one whose tabs are shown) has its own background over both lines (`[colors] current_space`, a 256-colour index or `#rrggbb`, default 236). AGENTS lists every main Claude session that has not ended (see Task tree) in an open pane, in any space, sorted by state: a dot, its label and state, then, dimmed, its space and tab, with a blank line between agents. The current agent (the one in the focused terminal of the page shown) has the current space's background over both its lines. Subagents and task rows stay in the dashboard. While the sidebar has the keyboard, the selected space or agent is reversed. `C-a s` gives it the keyboard on the current agent if the focused terminal holds one, else on the current space; `C-a w` always on the current space; either shows a hidden sidebar.
- Right: the **session view**, the current item's tab bar and the current tab's terminals (splits).
- The tab bar follows herdr's look: a darker row (`[colors] tab_bar`, default 235) with each tab a filled block, its name centred, at least 8 cells wide, one blank column apart; the current tab has the accent colour (`tab_current`, default the terminal's blue) with dark text, the others a dark grey (`tab`, default 237) with dim text, and a name the user gave is bold. A `+` after the last tab opens a new one. When the tabs do not fit, `<` and `>` scroll by one and `…` marks hidden tabs; switching tabs scrolls the current one into sight, about in the middle. The wheel over the bar switches tabs; `C-a <` and `C-a >` move the current tab (no wrap). A tab is named after what runs in its focused pane (`claude` while Claude runs there, else the command, or the directory for a shell) until the user renames it. A tab holding a waiting, done or idle Claude gets a `●` in that state's colour before its name.
- The **dashboard** opens as an overlay (`C-a o`) and holds the task tree: the list on top (as tall as its rows, at most about 40% of the height, with a blank line under its column header and another under its rows), the live preview below it in a rounded border with its title in the top border, and the keys on its last line, under that border. On a short screen the preview goes first.
- A new space (`C-a C`) and a new tab (`C-a c`) start a blank shell in the focused terminal's directory; paddock notices when Claude starts in it. A new space first asks for its name, prefilled with `space-N` (`Enter` keeps it); a new tab does not ask.
- **Floats** (`[floats]`, like tmux's `display-popup` but persistent): the prefix and a float's key (default `g`, running `lazygit`) show a program in a box centred over the whole screen, sidebar included, `width`% by `height`% of it (default 80 by 80, at least one cell inside and at most the screen), framed like the prompt popup: rounded, in the accent colour (`[colors] tab_current`), the command as its title in the top border. The same keys hide it and the program keeps running; showing it again brings back the same program and state. When the program exits the float closes, and the next toggle starts it again. Each space has its own float per key, started in the focused terminal's directory: a tmux window of the space's session, created in the background (never the session's current window) and marked with the window option `@paddock-float` (the key). `list-panes -s` reads the option, and marked windows are kept apart from the tabs, so they never show in the tab bar, its counts, `C-a Tab`'s history, cycling or the sidebar, and hooks and `claude agents` from their panes are ignored (paddock does not follow Claude in a float). The float's window gets its own size on the space's control client (`refresh-client -C @window:WxH`, which later client resizes leave alone; ADR 0006), the inside of the box, sent when it shows and on every relayout. If tmux makes a float current (the current tab closed), paddock selects a tab again; when the last tab closes, the space closes with its floats. One float shows at a time; showing another hides the first. While one shows, every key goes to it except the prefix: `C-a` commands work, and one that changes the page or gives the sidebar or dashboard the keyboard hides the float first (help, showing, hiding or resizing the sidebar, and detaching keep it). Paste goes to it, the mouse inside it goes to its pane, and a click outside does nothing. A float whose key was removed from the config is closed, with its program, the next time paddock opens (at attach, not on later structure reads). The float is made and marked in one tmux run, so it never shows as a tab; if marking fails, the new window is killed by its unique name, so it never stays as a tab. A program that exits within two seconds of starting, before any input (keys, paste or mouse) reaches it, gives a notice (`lazygit exited at once`). Hooks from a float's pane are ignored even after the float closed (paddock remembers the panes of floats it has seen). While a float shows, the tab under it does not count as looked at: a Claude there that finishes or waits still notifies. A space's directory (its sidebar row) is its first tab's, never a float's.
- **Prompts** (after LazyVim's input): renaming a tab (`C-a ,`) or space (`C-a $`), naming a new space, and the question before a busy close open a popup centred on the whole screen, over the sidebar, with nothing dimmed behind it: a rounded box in the accent colour (`[colors] tab_current`, the current tab's), its title in the top border (` Rename tab `, ` Rename space `, ` New space `, ` Close tab `, ` Close pane `, ` Close space `; closing a pane alone in its space's only tab says ` Close space `), one line inside. A name starts as the current one (or the default) with the cursor at its end; the box is 40 columns wide, narrower on small screens, and long text scrolls inside it to keep the cursor in sight (by character, wide characters by their width). A question reads `Claude is <state>: close? y/n`. While the popup is open every key goes to it; a click outside cancels it, a click inside does nothing. Asked from the sidebar or the dashboard, it gives the keyboard back to that panel when it closes. A tmux error (e.g. a taken name) shows as a notice in the tab bar once it closes, tmux's reason first (`duplicate session: space-1 (tmux …)`) so it shows even when the notice is cut to the bar's width.

### Task tree

- Shown in the dashboard. Levels: session → task.
- A session row is a terminal where Claude runs. Its label is the Claude session name if set, else the tab name if the user gave one, else the directory name.
- Task rows are the main agent's branch plus each subagent's branch, labelled with the subagent's task description.
- Plain shells and the default branch get no row.
- Everything under a session belongs to it: a session and its task rows form a block, with a blank line between two blocks. Only sessions are selected: moving skips task rows, the whole selected block gets the current space's background (`[colors] current_space`, as in the sidebar; each line keeps its own colours as text), scrolling keeps the whole block in sight where it fits, and every key acts on the session; dismissing it dismisses its task rows with it.
- Rows stay until dismissed (`D`) or until their session ends: its terminal (pane, tab or space) is closed, or Claude quits in it (`SessionEnd`: the session is stopped). A session that ended leaves at once with its task rows, everywhere rows are used (the dashboard, the sidebar's AGENTS, the dots and tab markers, `C-a a`), and replaying the event log on reopen does not bring it back. If it was selected, the session that took its place is selected, and interactive preview ends. A new Claude in the same terminal is a new session and gets a row, without the ended session's task rows. `/clear` (`SessionEnd` with reason `clear`) only starts a new conversation in the same Claude, so its row stays; subagents still at work belong to the conversation that ended, so their tasks stop all the same. A hook that lands after `SessionEnd` (a subagent's, or a tool's racing it) does not bring an ended session back, nor does a late `SessionStart` from a compaction (`source: compact`); only Claude starting that conversation again (a resume) does. There is no setting to keep ended sessions.
- External sessions appear dimmed in a group at the bottom (v1.2).

### Fields

| Where | Fields |
|---|---|
| Sidebar space | name, dot in the most urgent state of its agents, directory, branch |
| Sidebar agent | label, state and highlight, space and tab |
| Dashboard | state and highlight, branch, ahead/behind upstream, line changes (`+30 −144`, uncommitted), base diff, number of changed files, last commit age, merged badge; PR number and state after v1 |

The dashboard list is a table under a dim header: NAME (the session's label, its task rows indented under it), STATE, ↑↓, CHANGES, BASE, FILES, AGE, MERGED. Every line puts each value in its column, empty when zero or unknown, so values line up; a column is as wide as its widest value in sight (capped), numbers to the right. When the list is narrow, whole columns go so the name keeps room, MERGED first, then FILES, BASE, ↑↓, AGE and CHANGES; NAME and STATE always stay, and a name too long is cut with `…`.

### States

- Session: running, waiting, done, idle, or unknown. A session that ended (Claude quit) has no row. A main agent that finishes a turn (`Stop`) is done while you are not looking at its tab (the current space's current tab, outer window focused), and turns idle once you do. Done is not saved: it is per running paddock.
- Subagent task: running, waiting, done, or stopped.
- Compacting counts as running. Sort order: waiting, done, idle, running, unknown.
- Task rows within a session sort waiting, running, done, stopped, ties in tree order; the main agent's task row has its session's state (done and idle count as done).

### Attention

- Waiting rows and agents get a strong highlight; done (bold) and idle ones a soft one. Tabs holding such sessions are highlighted and marked `●` too.
- macOS notifications fire when a session or one of its subagents starts waiting and when a main agent goes idle (a subagent finishing never notifies), but not when that terminal is already focused. `[notify] waiting = false` or `idle = false` turns off that kind (both default to `true`); only the notification goes, while states, highlights, markers, `C-a a` and the dashboard stay as they are.

### Preview

- The dashboard shows a live preview of the selected row. A subagent's task row shows its session's screen.
- `I` enters interactive preview and types into that session; `Esc` leaves and is never sent to Claude; so does any Alt key, since `Esc` and a key read together arrive as one. To interrupt Claude, jump to the session.

### Actions

- **Close** (`x`): closes the focused pane, or in the dashboard the row's pane; its tab goes with its last pane and its space with its last tab. In the sidebar, `x` closes the selected space, or the selected agent's pane. Asks first if Claude is running or waiting there.
- **Fork** (`f`): runs `claude --resume <id> --fork-session` in a new tab of the same space, in the session's directory. Moving the fork into its own worktree is Claude's job (its `EnterWorktree` tool).
- A single subagent can only be stopped from inside its session.

### Mouse

- Click: focus a terminal, open a space or jump to an agent, switch tabs.
- Wheel: scroll a terminal's history.
- Drag: select text; releasing copies it.
- Programs that ask for the mouse (vim, htop) receive it; hold Shift to select anyway.
- Dragging split borders comes later.

### Keys

Prefix `C-a`, configurable; pressing it twice sends a literal `C-a`.

Every key paddock reads is a named action in one table per mode (`src/input/keymap/tables.rs`): after the prefix (`[keys.commands]`), with the sidebar focused (`[keys.sidebar]`) and in the dashboard (`[keys.dashboard]`). The same commands fire without the prefix on a direct chord (`[keys.direct]`, the commands' table with each command's default chords: `C-h/j/k/l` focus, `C-M-c` new tab, `C-\`/`C--` splits; cycling tabs has none, as `C-M-[` is two `Esc` outside the kitty protocol), whenever a terminal has the keyboard and no float shows (a float gets every key but the prefix); it never waits for a second key, and matches only as bound. The help overlay shows each command's chord after it in brackets, which keeps it within 80x24. Since `C-h` and `C-j` are Backspace and Enter to the terminal, a `[keys.direct]` not written at all gives a notice at start naming the control characters taken (`C-h`, `C-i`, `C-j`, `C-m`, `C-[`); written, even empty, the user chose them (`Keymap::configured` decides). Dispatch, the help overlay, the dashboard's key line and `paddock keys` (the defaults as TOML) are built from those tables, and a unit test checks that the README's key tables match them, so the README is where the default keys are listed. `config.toml` gives an action one key, a list, or `""` to unbind it; an action left out keeps its defaults.

One key grammar serves the prefix and the bindings (`src/input/keymap/key.rs`), normalised the way terminals report keys: Shift folded into characters, a Ctrl letter always lowercase, and the characters that share one control byte folded into one key: `C-_` and `C-7` are `C--`, `C-4` is `C-\`, `C-5` is `C-]`, `C-6` is `C-^` (the digits are what crossterm reports 0x1c-0x1f as without the kitty protocol). No other keys fold, so under kitty `C-/`, `C-3` and `C-2` stay keys of their own. Defaults avoid Alt (any terminal app), except the direct chord `C-M-c`, which gives no warning; `M-` keys the user binds are accepted with a warning, shown at start and printed again on leaving. As before keys were configurable, Shift does not matter after the prefix, and Ctrl and Shift do not matter in the sidebar and dashboard, unless a key is bound with them. Config errors stop paddock before it touches the screen, naming the file, section and action: an unknown action or section, a key that does not parse, a key listed twice for one action, one key bound to two actions in one section (defaults included), or the prefix key bound inside a section (it always starts a command). `[floats]` keys use the same grammar and fail the same way, and also when one is bound in `[keys.commands]` (the default `g` included); `width` and `height` are whole percents from 1 to 100.

Some keys are fixed on purpose: the prefix twice sends it; `Esc` always leaves typing into the dashboard preview and is never passed on, so no remap can trap you there; prompt editing (`Enter`, `Esc`, `Left`/`Right`, `Home`/`End`, `Backspace`/`Delete`, `C-u` (clear), `C-w` (delete word)) and the `y` that confirms a close.

Planned (v1.2): `[` keyboard copy mode, `/` search scrollback.

## Tasks and worktrees

A task is a branch ([ADR 0004](adr/0004-task-is-a-branch.md)). paddock reads `git worktree list` and branch status but never creates or deletes worktrees. Subagents working in parallel need their own worktrees; one line in the user's CLAUDE.md asks Claude to use worktree isolation for branch work in subagents.

## Scope

- **v1**: hidden tmux server, control-mode rendering with own scrollback, single instance, layout kept in tmux; spaces (tmux sessions) and agents in the sidebar, tabs, minimal splits (split, close, move focus, even out), prefix keys, mouse (click, wheel, drag to copy); hooks, states, highlights, notifications; dashboard with the task tree including subagents, git fields, dismiss, live and interactive preview; close and fork.
- **v1.2**: a root directory per space, scrollback search, keyboard copy mode, external sessions group.
- **Later**: restore after reboot (`claude --resume` per terminal), PR column, resizing splits, other agents, Linux.

## Build order

Each step leaves something usable.

0. Spikes.
   - (a) Claude through control mode and our emulator: Shift+Enter, Esc latency, bracketed paste, truecolor, wide characters and emoji, mouse wheel, history after reopening, CPU. Picks the emulator crate.
   - (b) Whether a worktree-isolated subagent's transcript records the worktree's directory and branch.
1. Terminal core: hidden server, control-mode client, emulator, scrollback, one tab.
2. Tabs, prefix keys, single instance, existing terminals listed as tabs on reopen.
3. Agents: hooks, states, sidebar session rows, highlights, notifications. Daily use starts here.
4. Tasks: branch tracking including subagents and worktrees, the tree, git fields, dismiss.
5. Splits, mouse, full layout restore.
6. Close and fork.
7. Dashboard: details, live preview, interactive preview. v1 is complete.

## Testing

Test-first with at least 80% coverage:

- **App-level harness**: the real `paddock` binary runs in a PTY against a tmux server on a temporary socket, fed recorded hook payloads, with `osascript` stubbed.
- **Pure-function tests** for turning hook events into the task tree.

## Facts checked during design (Claude Code 2.1.303, tmux 3.8)

- `claude agents --json` lists interactive and background sessions with `cwd`, `kind`, `name`, `pid`, `sessionId`, `startedAt` and `status` (`busy`, `waiting`, `idle`), without needing a TTY.
- Each subagent gets `~/.claude/projects/<mangled cwd>/<session id>/subagents/agent-<id>.jsonl` plus a `.meta.json` holding `agentType`, `description` and `finished`. Every transcript line carries `gitBranch` and `cwd`.
- `PermissionRequest` fires immediately, while `Notification` with `permission_prompt` fires about 6 seconds later; `Stop` does not fire when the user interrupts with Esc.
- `claude --resume <id> --fork-session` starts a new session id from an existing conversation.

## Risks

- Claude Code's transcript files, `.meta.json` and the `claude agents --json` fields are undocumented and may change. Hooks are the primary source; these only reconcile.
- The renderer is the largest piece of work, and encoding keys, mouse and paste for control mode is the hardest part of it. Spike 0a gates the build.
- One instance at a time is the price of having no daemon.
- A session ends for paddock on `SessionEnd` or when its terminal closes. A Claude killed without `SessionEnd` (e.g. `kill -9`) keeps its row, in its last state, until its terminal closes or it is dismissed.
