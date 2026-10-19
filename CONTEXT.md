# paddock

paddock is a terminal workspace for running Claude Code. It keeps your terminals alive in the background and shows which Claude sessions, and which branches they work on, need you.

## Language

### Layout

**Workspace**:
A named group of tabs you create to organise work, optionally tied to a root directory where new terminals start. With the workspace panel hidden, everything lives in one default workspace.
_Avoid_: project, group, category

**Tab**:
One page of a workspace, holding one or more terminals arranged as splits.
_Avoid_: window

**Terminal**:
One shell inside paddock, shown in a tab or a split. It keeps running when paddock is closed.
_Avoid_: pane, tmux session

**Sidebar**:
The always-visible list on the left: the workspace panel above the task tree.

**Session view**:
The main area, showing the current tab's terminals.

**Dashboard**:
An overlay with full details and a live preview of the selected row.
_Avoid_: overview screen

**Preview**:
A live picture of a session's screen inside the dashboard. In interactive preview you type into that session; `Esc` leaves and is never passed on.

### Agents and work

**Session**:
A Claude Code conversation running, or that ran, in a terminal.
_Avoid_: agent (too broad), terminal, tmux session

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

**Idle**:
The main agent finished its turn and is ready for your next prompt.

**Done**:
A subagent handed back its result.

**Stopped**:
Claude no longer runs in that terminal, or the session ended while a subagent was still running.

**Unknown**:
No state has been reported yet.

### Actions

**Dismiss**:
Remove a row from the task tree. Git and running processes are untouched.
_Avoid_: delete, clear

**Close**:
End a terminal and everything running in it.
_Avoid_: terminate, kill

**Fork**:
Start a new session that continues a copy of another session's conversation, in a new tab in the same directory.
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
