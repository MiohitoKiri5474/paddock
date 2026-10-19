# A task is a git branch, and paddock never manages worktrees

The task tree has one row per branch (one purpose, one branch), including branches that subagents work on, grouped under the session that worked on them. Rows stay until the user dismisses them. paddock reads git (`git worktree list`, branch status) but never creates or deletes worktrees. Because a working tree has only one branch checked out, subagents working in parallel must each have their own worktree; one line in the user's CLAUDE.md asks Claude to use worktree isolation for that.

## Considered options

- **One row per agent**: stays accurate when agents share a directory, but subagents that never branch would clutter the list, and a branch that several agents work on in turn would show as several rows.
- **paddock creates the worktrees**: the user does not want worktree management in paddock.
