# paddock

paddock is a terminal workspace for running coding agents (Claude Code, Gemini CLI and Antigravity CLI). It keeps your terminals alive in the background and shows which agent sessions, and which branches they work on, need you.

## Language

### Layout

**Space**:
A named group of tabs you create to organise work, listed in the sidebar's SPACES section. It is a tmux session on the hidden server.
_Avoid_: workspace, project, group, category

**Space root**:
A space's own directory: the one it was made in (a worktree space's worktree), until `C-a R` makes it the focused terminal's. New tabs and splits can start there (`[spaces] new_tab_dir = "root"`). A space made before roots uses its first tab's directory. Not the **Main checkout**, which belongs to a repository, not a space.
_Avoid_: root (alone), home, base (that is a branch), project directory

**Current space**:
The space whose tabs the tab bar shows. It has its own background in the sidebar.

**Tab**:
One page of a space, holding one or more terminals arranged as splits. It is a tmux window of the space's session.

**Split**:
A terminal sharing its tab with others, beside or above them. It is a tmux pane; tmux keeps the layout and the sizes, which `C-a H/J/K/L` and dragging the border between two splits change.
_Avoid_: window (that is a tab)

**Zoom**:
A split filling its tab for a while (`C-a z`), the others hidden but running; the tab bar marks the tab with a `Z`. It is tmux's `resize-pane -Z`, so it lives in tmux.
_Avoid_: maximise, fullscreen

**Terminal**:
One shell inside paddock, shown in a tab, alone or with others. It is a tmux pane and keeps running when paddock is closed.
_Avoid_: tmux session

**Server**:
paddock's own background process that holds every terminal and the layout while the app is closed, with the native backend. The app is its client.
_Avoid_: daemon (in user-facing text), hidden server (that is tmux's)

**Backend**:
What hosts the terminals: paddock's own server (the native backend), or the hidden tmux server (the tmux backend).
_Avoid_: engine, runtime

**Sidebar**:
The panel on the left, which can be shown or hidden: SPACES, two lines per space with a dot for the most urgent state of its agents, then its directory and branch, that branch's git status (`+200 −10 ↑2↓1`: uncommitted lines, commits ahead and behind) and its PR mark (`#123 ✓`) where each fits (the PR mark dropped first), then AGENTS, two lines per main agent in any space with its state, space and tab, and its agent kind's tag where it fits, in space and tab order (a state change never moves one), external sessions last.

**Agent**:
A main agent in the sidebar's AGENTS section. Subagents show only in the dashboard.

**Float**:
A program shown in a box over the whole screen and hidden again by the prefix and its key, kept running while hidden. Each space has its own float per key. It is a tmux window of the space's session marked as a float, never a tab.
_Avoid_: popup (that is the prompt), overlay

**Session view**:
The main area, showing the current tab's terminals.

**Finder**:
A box opened by the prefix and `g` with a text field and, under it, every space, tab and agent that fuzzy-matches the text, best first; `Enter` shows the one selected. It is a mode of the app, like naming, not a float.
_Avoid_: picker, palette, goto, switcher

**Copy mode**:
A mode of one terminal, opened by the prefix and `[`, where its view stops following output and keys move a cursor over its history and screen to select, copy and search; leaving shows what came meanwhile. It is a mode of the app, like the finder.
_Avoid_: scroll mode, vi mode

**Dashboard**:
An overlay with the task tree, full details and a live preview of the selected row.
_Avoid_: overview screen

**Preview**:
A live picture of a session's screen inside the dashboard. In interactive preview you type into that session; `Esc` leaves and is never passed on.

### Agents and work

**Session**:
A coding agent's conversation (Claude Code, Gemini CLI or Antigravity CLI) running, or that ran, in a terminal. One running outside paddock's terminals is an external session.
_Avoid_: agent (too broad), terminal, tmux session

**Agent kind**:
Which coding agent a session belongs to: `claude` (Claude Code), `gemini` (Gemini CLI) or `agy` (Antigravity CLI). Its rows show it as a short tag, and a tab named after its session takes it. Fork, subagents and the `claude agents --json` snapshot are Claude's alone.
_Avoid_: agent type, provider, model

**Status report**:
An agent's report of its state (working, waiting on you, done, failed, idle) that it writes to its terminal's output (OSC 7501), read by paddock from that output. Only Antigravity CLI's count, and only in paddock's terminals; they are how its waits show.
_Avoid_: status line, notification (that is macOS's)

**Ended**:
A session whose terminal was closed or whose agent quit (agy: its terminal is back at the shell). It leaves the dashboard and the sidebar; a new agent in the same terminal is a new session. `/clear` (Claude's or Gemini's) does not end it.
_Avoid_: stopped (a subagent task's state)

**Main agent**:
The agent you talk to directly in a session.
_Avoid_: parent agent

**Subagent**:
A Claude agent the main agent starts inside a session. It has no terminal of its own; you reach it through its session.

**Task**:
One purpose of work, identified by its git branch: one purpose, one branch.
_Avoid_: job, ticket, worktree

**External session**:
A session running outside paddock's terminals (another terminal app, an IDE). paddock shows it dimmed in an External group at the bottom of the dashboard and of AGENTS, with its state and branch, but not its screen. It can only be dismissed: never shown, closed, forked or typed into; and `C-a a` skips it. It leaves when it ends, or after a quiet timeout.

### States

**Running**:
The agent is working.

**Waiting**:
The agent is blocked on you: a permission prompt, a question, or a plan to approve.

**Done**:
The main agent finished its turn while you looked at another tab; it turns idle once you look at its tab. (A subagent that handed back its result is also done.)

**Idle**:
The main agent finished its turn and is ready for your next prompt.

**Stopped**:
A subagent task whose session ended while it was still running.

**Unknown**:
No state has been reported yet.

### Actions

**Dismiss**:
Remove a row from the dashboard's task tree. Git and running processes are untouched.
_Avoid_: delete, clear

**Close**:
End a terminal and everything running in it. A tab closes with its last terminal, a space with its last tab.
_Avoid_: terminate, kill

**Fork**:
Start a new session that continues a copy of another session's conversation, in a new tab of the same space, in the same directory.
_Avoid_: clone, duplicate

**Detach**:
Leave paddock while every terminal keeps running.
_Avoid_: quit, exit

### Keys

**Prefix**:
The key (`C-a` unless configured) that makes the next key a paddock command instead of input for the terminal. Pressed twice, it goes to the terminal.
_Avoid_: shortcut, hotkey

**Direct chord**:
One key combination that runs a paddock command without the prefix, such as `C-l` to focus the terminal on the right. It works while a terminal has the keyboard, and goes to the program in a float.
_Avoid_: shortcut, hotkey

### Git

**Worktree**:
A git working tree linked to a repository, checked out on its own branch in its own directory. paddock makes one with `C-a W` (under `[worktrees] directory`), opens one with `C-a O`, and deletes one it made (under that directory) with `x` on its space; a space in a worktree is listed under its repository's main checkout. `paddock worktree new|open|list|rm` does the same without the UI, for agents and scripts.
_Avoid_: checkout (for a linked one), clone

**Main checkout**:
A repository's first working tree, the one `git clone` or `git init` made; the others are its worktrees. paddock never deletes it.
_Avoid_: main worktree, root repo

**Line changes**:
Uncommitted lines added and removed in a task's working tree, shown as `+30 −144`.
_Avoid_: diff

**Base diff**:
Lines added and removed on a task's branch compared with its base branch.

**Pull request (PR)**:
The GitHub pull request of a branch paddock shows (a task's, a space's or a worktree's), as the GitHub CLI (`gh`) reports it: its number, state (open, draft, merged, closed), CI checks rolled up into one mark (`✓`, `✗`, `•`) and review decision. The newest one when a branch has several. paddock only reads it.
_Avoid_: merge request, MR
