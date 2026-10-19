# paddock design

Agreed on 2026-10-19 in a design interview. Vocabulary is defined in [CONTEXT.md](../CONTEXT.md); decisions with lasting trade-offs are recorded in [docs/adr/](adr/).

## Purpose

paddock replaces herdr for one person on one Mac. It is a terminal workspace (workspaces, tabs, splits) whose terminals keep running when paddock closes, and it tracks Claude Code sessions and the branches they and their subagents work on, so you can see at a glance which ones need you.

Not in scope for now: agents other than Claude Code, Linux, remote machines, a scripting API, and creating or deleting git worktrees.

## Architecture

- **One binary, no daemon** ([ADR 0003](adr/0003-no-daemon-single-instance.md)). `paddock` opens the app. While it is closed, only two things run: the hidden tmux server and Claude Code hooks.
- **Hidden tmux server** ([ADR 0001](adr/0001-tmux-as-hidden-backend.md)). `tmux -L paddock`, one tmux session per terminal, started with paddock's own minimal config (`escape-time 0`, `extended-keys on`, no status line, no prefix) and a clean environment (`SUPACODE_*`, `TERM_PROGRAM` and similar variables removed). The user's own tmux is never touched.
- **Own renderer** ([ADR 0002](adr/0002-own-renderer-via-control-mode.md)). paddock is a tmux control-mode client. Each pane's raw output feeds its own terminal-emulator instance; paddock draws everything with ratatui and keeps scrollback, selection and search itself. Spike 0a picks the emulator crate. Written in Rust.
- **One instance at a time.** A second launch offers to take over from the first.
- **State** lives in `~/.local/state/paddock/`: layout, task rows, hook events and logs. **Config** lives in `~/.config/paddock/config.toml`: prefix, keys, colours and notification switches.
- **Hooks.** `paddock hooks install` and `paddock hooks uninstall` edit `~/.claude/settings.json`. Supacode's hooks stay; the old tmux-claude-session-manager hooks are removed. Every hook calls `paddock hook <event>`, which finds its terminal through `$TMUX_PANE` and appends the event to the state directory, so tasks created while paddock is closed still appear. `claude agents --json` is used to discover sessions and reconcile state.
- **Commands.** `paddock`, `paddock hooks install|uninstall`, and the internal `paddock hook <event>`.
- **Any terminal app.** Only standard VT features; truecolor with a 256-colour fallback; the UI uses the terminal's 16 ANSI colours so it follows the user's theme; copying goes through `pbcopy` and notifications through `osascript`; no key depends on Option/Alt. paddock makes no network calls.
- **Refresh.** Git info refreshes on hook events and every few seconds for rows on screen.

## Interface

### Layout

- Left: the **sidebar**, a closable workspace panel above the task tree.
- Right: the **session view**, the current workspace's tab bar and the current tab's terminals (tabs and splits).
- The **dashboard** opens as an overlay (`C-a o`).
- Workspaces are created and named by the user, each with an optional root directory. With the panel closed, everything lives in one default workspace.
- A new terminal (`C-a c`) is a blank shell; paddock notices when Claude starts in it.

### Task tree

- Levels: workspace → session → task. The workspace level disappears when the panel is closed.
- A session row is a terminal where Claude runs or ran. Its label is the Claude session name if set, else the tab name, else the directory name.
- Task rows are the main agent's branch plus each subagent's branch, labelled with the subagent's task description.
- Plain shells and the default branch get no row.
- Rows stay until dismissed (`D`).
- External sessions appear dimmed in a group at the bottom (v1.2).

### Fields

| Where | Fields |
|---|---|
| Sidebar row | state and highlight, branch, ahead/behind upstream, line changes (`+30 −144`, uncommitted) |
| Dashboard | everything in the sidebar, plus base diff, number of changed files, last commit age, merged badge; PR number and state after v1 |

### States

- Session: running, waiting, idle, stopped, or unknown.
- Subagent task: running, waiting, done, or stopped.
- Compacting counts as running. Sort order: waiting, idle, running, stopped.

### Attention

- Waiting rows get a strong highlight; idle and done rows a soft one. Tabs holding such sessions are highlighted too.
- macOS notifications fire when a session starts waiting and when a main agent goes idle, but not when that terminal is already focused.

### Preview

- The dashboard shows a live preview of the selected row. A subagent's task row shows its session's screen.
- `I` enters interactive preview and types into that session; `Esc` leaves and is never sent to Claude. To interrupt Claude, jump to the session.

### Actions

- **Close** (`x`, session rows): kills the terminal's tmux session; asks first if Claude is running or waiting.
- **Fork** (`f`, session rows): runs `claude --resume <id> --fork-session` in a new tab in the same directory. Moving the fork into its own worktree is Claude's job (its `EnterWorktree` tool).
- A single subagent can only be stopped from inside its session.

### Mouse

- Click: focus a terminal, jump to a row, switch tabs.
- Wheel: scroll a terminal's history.
- Drag: select text; releasing copies it.
- Programs that ask for the mouse (vim, htop) receive it; hold Shift to select anyway.
- Dragging split borders comes later.

### Keys

Prefix `C-a`, configurable; pressing it twice sends a literal `C-a`.

| Key | Action |
|---|---|
| `c` | new shell tab |
| `\|` / `-` | split side by side / top and bottom |
| `x` | close terminal (asks if Claude is busy) |
| `f` | fork Claude into a new tab |
| `h` `j` `k` `l` / arrows | move focus |
| `1`–`9` | go to tab |
| `n` / `p` | next / previous tab |
| `a` | next session that needs you |
| `=` | equalise split sizes |
| `o` | dashboard (v1.1) |
| `s` | focus sidebar |
| `b` | show or hide sidebar |
| `w` | workspace panel (v1.2) |
| `?` | key help |
| `d` | detach: paddock quits, terminals keep running |
| `,` | rename tab |
| `[` | keyboard copy mode (v1.2) |
| `/` | search scrollback (v1.2) |
| `C-a` | literal `C-a` |

In the sidebar and dashboard: `j`/`k` move, `Enter` jumps, `D` dismisses, `x` closes, `f` forks, `Esc` goes back. Dashboard only: `I` for interactive preview.

## Tasks and worktrees

A task is a branch ([ADR 0004](adr/0004-task-is-a-branch.md)). paddock reads `git worktree list` and branch status but never creates or deletes worktrees. Subagents working in parallel need their own worktrees; one line in the user's CLAUDE.md asks Claude to use worktree isolation for branch work in subagents.

## Scope

- **v1**: hidden tmux server, control-mode rendering with own scrollback, single instance, layout restored on reopen; one default workspace, tabs, minimal splits (split, close, move focus, equalise), prefix keys, mouse (click, wheel, drag to copy); hooks, states, sidebar task tree including subagents, sidebar git fields, highlights, notifications, dismiss; close and fork.
- **v1.1**: dashboard (details, live preview, interactive preview).
- **v1.2**: workspaces and the workspace panel, scrollback search, keyboard copy mode, external sessions group.
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
6. Close and fork. v1 is complete.

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
