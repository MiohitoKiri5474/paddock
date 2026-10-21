# A task is a git branch, and a worktree is a space

The task tree has one row per branch (one purpose, one branch), including branches that subagents work on, grouped under the session that worked on them. Rows stay until the user dismisses them. paddock reads git (`git worktree list`, branch status). Because a working tree has only one branch checked out, subagents working in parallel must each have their own worktree; one line in the user's CLAUDE.md asks Claude to use worktree isolation for that.

## Amendment (ticket 24): paddock creates, opens and deletes worktrees

This ADR first said paddock "never creates or deletes worktrees". That restriction is dropped; "a task is a branch" stays.

paddock now follows herdr's model: a git worktree is a space. `C-a W` checks a branch out in a new worktree under `[worktrees] directory` and opens a space in it, `C-a O` opens a space for an existing worktree, and `x` on a worktree space deletes the space, its worktree and its branch (`git branch -d`, never `-D`), asking twice when work would be lost. The sidebar lists worktree spaces under their repository's main checkout. Only worktrees under `[worktrees] directory` are deleted: the main checkout never is, nor are worktrees elsewhere, such as Claude's own.

Why: in daily use, every task got its own worktree anyway, made by hand in another terminal and then opened as a space by hand; cleaning up meant the same steps backwards, and forgotten worktrees piled up. Doing it where the spaces live makes a task's whole life (branch, worktree, space, Claude) one place, and herdr showed the model works. The task tree is unchanged: rows are still per branch, whether or not a worktree or a space holds it. paddock still changes nothing else in git, and subagents' worktrees stay Claude's own (paddock only reads them).

## Considered options

- **One row per agent**: stays accurate when agents share a directory, but subagents that never branch would clutter the list, and a branch that several agents work on in turn would show as several rows.
- **paddock creates the worktrees** (first rejected: the user did not want worktree management in paddock): accepted by the amendment above, for the user's own task worktrees only.

## Amendment (ticket 25): an agent can delete a worktree, without the questions

`paddock worktree rm` deletes what `x` deletes, for agents and scripts, and asks nothing: it has no terminal to ask in. What `x` asks about (uncommitted changes, commits not in the base) it refuses with exit 3, naming the risk, unless an explicit flag agrees to that loss: `--discard-changes` (and only then `git worktree remove --force`) or `--discard-commits` (`branch -d` still keeps an unmerged branch). What `x` never deletes, it never deletes either, whatever the flags: the main checkout, a worktree paddock did not make, one where a Claude is running or waiting, and one the caller's own directory or terminal is in. It checks again right before removing, as `x` does, and a check that runs out of time deletes nothing.

Why: the owner wants worktree management to be agent friendly, and an agent cannot answer `y/n`. The flags stand in for the second question; the answer has to be given up front and by name. The plugin's skill never pre-approves `rm`, so Claude Code still asks the user before each delete, and it tells Claude never to pass `--discard-*` without the user's say-so.
