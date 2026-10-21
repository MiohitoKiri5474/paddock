# paddock

paddock is a terminal workspace for running Claude Code. It keeps your terminals alive in the background and shows which Claude sessions, and which branches they work on, need you.

## Language

### Layout

**Space**:
A named group of tabs you create to organise work, listed in the sidebar's SPACES section. It is a tmux session on the hidden server.
_Avoid_: workspace, project, group, category

**Current space**:
The space whose tabs the tab bar shows. It has its own background in the sidebar.

**Tab**:
One page of a space, holding one or more terminals arranged as splits. It is a tmux window of the space's session.

**Terminal**:
One shell inside paddock, shown in a tab or a split. It is a tmux pane and keeps running when paddock is closed.
_Avoid_: tmux session

**Sidebar**:
The panel on the left, which can be shown or hidden: SPACES, one line per space with a dot for the most urgent state of its agents, then AGENTS, two lines per main agent in any space with its state, space and tab.

**Agent**:
A main agent in the sidebar's AGENTS section. Subagents show only in the dashboard.

**Float**:
A program shown in a box over the whole screen and hidden again by the prefix and its key, kept running while hidden. Each space has its own float per key. It is a tmux window of the space's session marked as a float, never a tab.
_Avoid_: popup (that is the prompt), overlay

**Session view**:
The main area, showing the current tab's terminals.

**Dashboard**:
An overlay with the task tree, full details and a live preview of the selected row.
_Avoid_: overview screen

**Preview**:
A live picture of a session's screen inside the dashboard. In interactive preview you type into that session; `Esc` leaves and is never passed on.

### Agents and work

**Session**:
A Claude Code conversation running, or that ran, in a terminal. One running outside paddock's terminals is an external session.
_Avoid_: agent (too broad), terminal, tmux session

**Ended**:
A session whose terminal was closed or whose Claude quit. It leaves the dashboard and the sidebar; a new Claude in the same terminal is a new session. `/clear` does not end it.
_Avoid_: stopped (a subagent task's state)

**Main agent**:
The Claude you talk to directly in a session.
_Avoid_: parent agent

**Subagent**:
A Claude agent the main agent starts inside a session. It has no terminal of its own; you reach it through its session.

**Task**:
One purpose of work, identified by its git branch: one purpose, one branch.
_Avoid_: job, ticket, worktree

**External session**:
A Claude session running outside paddock's terminals. paddock shows its state and branch, but not its screen.

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

### Git

**Line changes**:
Uncommitted lines added and removed in a task's working tree, shown as `+30 −144`.
_Avoid_: diff

**Base diff**:
Lines added and removed on a task's branch compared with its base branch.
