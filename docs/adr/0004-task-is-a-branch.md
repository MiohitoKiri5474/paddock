# A task is a git branch, and a worktree is a space

The task tree has one row per branch (one purpose, one branch), including branches that subagents work on, grouped under the session that worked on them. Rows stay until the user dismisses them. paddock reads git (`git worktree list`, branch status). Because a working tree has only one branch checked out, subagents working in parallel must each have their own worktree; one line in the user's CLAUDE.md asks Claude to use worktree isolation for that.

## Amendment (ticket 24): paddock creates, opens and deletes worktrees

This ADR first said paddock "never creates or deletes worktrees". That restriction is dropped; "a task is a branch" stays.

paddock now follows herdr's model: a git worktree is a space. `C-a W` checks a branch out in a new worktree under `[worktrees] directory` and opens a space in it, `C-a O` opens a space for an existing worktree, and `x` on a worktree space deletes the space, its worktree and its branch (`git branch -d`, never `-D`), asking twice when work would be lost. The sidebar lists worktree spaces under their repository's main checkout. The main checkout is never deleted.

Why: in daily use, every task got its own worktree anyway, made by hand in another terminal and then opened as a space by hand; cleaning up meant the same steps backwards, and forgotten worktrees piled up. Doing it where the spaces live makes a task's whole life (branch, worktree, space, Claude) one place, and herdr showed the model works. The task tree is unchanged: rows are still per branch, whether or not a worktree or a space holds it. paddock still changes nothing else in git, and subagents' worktrees stay Claude's own (paddock only reads them).

## Considered options

- **One row per agent**: stays accurate when agents share a directory, but subagents that never branch would clutter the list, and a branch that several agents work on in turn would show as several rows.
- **paddock creates the worktrees** (first rejected: the user did not want worktree management in paddock): accepted by the amendment above, for the user's own task worktrees only.
