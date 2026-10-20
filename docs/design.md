# paddock design

Agreed on 2026-10-19 in a design interview. Vocabulary is defined in [CONTEXT.md](../CONTEXT.md); decisions with lasting trade-offs are recorded in [docs/adr/](adr/).

## Purpose

paddock replaces herdr for one person on one Mac. It is a terminal workspace (spaces, tabs, splits) whose terminals keep running when paddock closes, and it tracks Claude Code sessions and the branches they and their subagents work on, so you can see at a glance which ones need you.

Not in scope for now: agents other than Claude Code, Linux, remote machines, a scripting API, and creating or deleting git worktrees.

## Architecture

- **One binary, no daemon** ([ADR 0003](adr/0003-no-daemon-single-instance.md)). `paddock` opens the app. While it is closed, only two things run: the hidden tmux server and Claude Code hooks.
- **Hidden tmux server** ([ADR 0001](adr/0001-tmux-as-hidden-backend.md)). `tmux -L paddock`, started with paddock's own minimal config (`escape-time 0`, `extended-keys on`, no status line, no prefix) and a clean environment (`SUPACODE_*`, `TERM_PROGRAM` and similar variables removed). The user's own tmux is never touched. Each space is a tmux session, its tabs the session's windows and its splits their panes ([ADR 0006](adr/0006-sidebar-items-are-tmux-sessions.md)), so `tmux -L paddock attach` shows the same structure.
- **Own renderer** ([ADR 0002](adr/0002-own-renderer-via-control-mode.md)). paddock is a tmux control-mode client, one per space. Each pane's raw output feeds its own terminal-emulator instance; paddock draws everything with ratatui and keeps scrollback, selection and search itself. Spike 0a picks the emulator crate. Written in Rust.
- **One instance at a time.** A second launch offers to take over from the first.
- **State** lives in `~/.local/state/paddock/`: the current item (tmux holds tabs, splits, sizes and focus), dismissed rows, hook events and logs. **Config** lives in `~/.config/paddock/config.toml`: prefix, keys, colours and notification switches.
- **Hooks.** `paddock hooks install` and `paddock hooks uninstall` edit `~/.claude/settings.json`. Supacode's hooks stay; the old tmux-claude-session-manager hooks are removed. Every hook calls `paddock hook <event>`, which finds its terminal through `$TMUX_PANE` and appends the event to the state directory, so tasks created while paddock is closed still appear. `claude agents --json` is used to discover sessions and reconcile state.
- **Commands.** `paddock`, `paddock hooks install|uninstall`, and the internal `paddock hook <event>`.
- **Any terminal app.** Only standard VT features; truecolor with a 256-colour fallback; the UI uses the terminal's 16 ANSI colours so it follows the user's theme; copying goes through `pbcopy` and notifications through `osascript`; no key depends on Option/Alt. paddock makes no network calls.
- **Refresh.** Git info refreshes on hook events and every few seconds for the dashboard's rows while it is open.

## Interface

### Layout

- Left: the closable **sidebar**, in two sections (after herdr):

  ```text
   SPACES
   ● api-server
     ~/project/api  main
     paddock
     ~/project/paddock  feat/x

   AGENTS
   ● api-server  waiting
     api-server · 2:claude
  ```

  SPACES has two lines per space: a dot in the most urgent state of its agents (none without agents) and its name (the tmux session name), then, dim, its directory (that of its first tab's first pane, home as `~`) and the branch checked out there (the short commit if detached, nothing outside a repository). A path too long loses its start (`…`) first, then the branch its end. Directories and branches are read on the git thread, on hook events and every few seconds. The current space (the one whose tabs are shown) has its own background over both lines (`[colors] current_item`, a 256-colour index or `#rrggbb`, default 236). AGENTS lists every main Claude session in an open pane, in any space, sorted by state: a dot, its label and state, then, dimmed, its space and tab, with a blank line between agents. Subagents and task rows stay in the dashboard. While the sidebar has the keyboard, the selected space or agent is reversed.
- Right: the **session view**, the current item's tab bar and the current tab's terminals (splits).
- Tabs are numbered and set apart by thin lines; the current one is reversed and bold. A tab is named after what runs in its focused pane (`claude` while Claude runs there, else the command, or the directory for a shell) until the user renames it. A tab holding a waiting, done or idle Claude gets its colour and a `●`.
- The **dashboard** opens as an overlay (`C-a o`) and holds the task tree.
- A new space (`C-a C`) and a new tab (`C-a c`) start a blank shell in the focused terminal's directory; paddock notices when Claude starts in it.

### Task tree

- Shown in the dashboard. Levels: session → task.
- A session row is a terminal where Claude runs or ran. Its label is the Claude session name if set, else the tab name if the user gave one, else the directory name.
- Task rows are the main agent's branch plus each subagent's branch, labelled with the subagent's task description.
- Plain shells and the default branch get no row.
- Rows stay until dismissed (`D`).
- External sessions appear dimmed in a group at the bottom (v1.2).

### Fields

| Where | Fields |
|---|---|
| Sidebar space | name, dot in the most urgent state of its agents, directory, branch |
| Sidebar agent | label, state and highlight, space and tab |
| Dashboard | state and highlight, branch, ahead/behind upstream, line changes (`+30 −144`, uncommitted), base diff, number of changed files, last commit age, merged badge; PR number and state after v1 |

### States

- Session: running, waiting, done, idle, stopped, or unknown. A main agent that finishes a turn (`Stop`) is done while you are not looking at its tab (the current space's current tab, outer window focused), and turns idle once you do. Done is not saved: it is per running paddock.
- Subagent task: running, waiting, done, or stopped.
- Compacting counts as running. Sort order: waiting, done, idle, running, stopped.
- Task rows within a session sort waiting, running, done, stopped, ties in tree order; the main agent's task row has its session's state (done and idle count as done).

### Attention

- Waiting rows and agents get a strong highlight; done (bold) and idle ones a soft one. Tabs holding such sessions are highlighted and marked `●` too.
- macOS notifications fire when a session starts waiting and when a main agent goes idle, but not when that terminal is already focused.

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

| Key | Action |
|---|---|
| `c` | new tab in the current space |
| `C` | new space |
| `\|` / `-` | split side by side / top and bottom |
| `x` | close pane (asks if Claude is busy) |
| `f` | fork Claude into a new tab |
| `h` `j` `k` `l` / arrows | move focus |
| `1`–`9` | go to tab |
| `n` / `p` | next / previous tab |
| `a` | next session that needs you |
| `=` | even out the focused split and its neighbours |
| `o` | dashboard |
| `s` | focus sidebar (on the first agent) |
| `w` | focus sidebar on the current space |
| `b` | show or hide sidebar |
| `?` | key help |
| `d` | detach: paddock quits, terminals keep running |
| `,` | rename tab |
| `$` | rename space |
| `[` | keyboard copy mode (v1.2) |
| `/` | search scrollback (v1.2) |
| `C-a` | literal `C-a` |

In the sidebar: `j`/`k` move over spaces then agents, `Enter` shows a space's tabs or jumps to an agent's pane, `x` closes the space or the agent's pane, `f` forks the agent (on a space, its most urgent one), `Esc` goes back. In the dashboard: `j`/`k` move, `Enter` jumps, `D` dismisses, `x` closes, `f` forks, `I` for interactive preview, `Esc` goes back.

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

Test-first with at least 80% coverage: unit tests for the model and git parsing, integration tests against a real tmux on a temporary socket, and ratatui `TestBackend` snapshots for the UI.

## Facts checked during design (Claude Code 2.1.303, tmux 3.8)

- `claude agents --json` lists interactive and background sessions with `cwd`, `kind`, `name`, `pid`, `sessionId`, `startedAt` and `status` (`busy`, `waiting`, `idle`), without needing a TTY.
- Each subagent gets `~/.claude/projects/<mangled cwd>/<session id>/subagents/agent-<id>.jsonl` plus a `.meta.json` holding `agentType`, `description` and `finished`. Every transcript line carries `gitBranch` and `cwd`.
- `PermissionRequest` fires immediately, while `Notification` with `permission_prompt` fires about 6 seconds later; `Stop` does not fire when the user interrupts with Esc.
- `claude --resume <id> --fork-session` starts a new session id from an existing conversation.

## Risks

- Claude Code's transcript files, `.meta.json` and the `claude agents --json` fields are undocumented and may change. Hooks are the primary source; these only reconcile.
- The renderer is the largest piece of work, and encoding keys, mouse and paste for control mode is the hardest part of it. Spike 0a gates the build.
- One instance at a time is the price of having no daemon.
- Rows stay until dismissed, so the task list grows unless the user prunes it.
