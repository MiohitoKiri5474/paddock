---
name: paddock-worktrees
description: Make, open, list and delete git worktrees as paddock spaces with the `paddock worktree` command. Use when work should happen on another branch in parallel, or when a task must not touch the current working tree, and paddock is installed (`command -v paddock`).
allowed-tools: Bash(paddock worktree list), Bash(paddock worktree list *), Bash(paddock worktree new *), Bash(paddock worktree open *)
---

# Worktrees with paddock

`paddock worktree` makes a git worktree and opens a paddock space (a tmux session) in it, so the user sees the work in paddock's sidebar. It never prompts and never needs a TTY.

## When to make one

- Parallel work on another branch while the current tree stays as it is.
- A task that must not touch the current working tree.

Otherwise, work where you are.

## Steps

1. Check first. A worktree for the branch may exist already:

   ```sh
   paddock worktree list --json
   ```

   Each row: `path`, `branch` (`null` if detached), `main`, `ours` (paddock made it), `space` (`null` if none open), `dirty`, `merged` (git's check: its commits are in the base), `pr` (its branch's GitHub pull request: `null` if none or `gh` could not tell, else `{"number", "state", "checks", "review", "url"}` with `state` `open`, `draft`, `merged` or `closed`), `agents` (states of the Claude sessions in its space).

   Find the row whose `branch` is your branch:

   - A row with a `space`: reuse it. Work in its `path`; skip step 2.
   - A row with `space: null`: run `open` (step 2).
   - No row: run `new` (step 2).

2. Make it, or open the one that exists:

   ```sh
   paddock worktree new <branch> --json      # {"path","branch","repo","space"}
   paddock worktree open <branch> --json     # same shape; exit 0 if a space is open already
   ```

   An existing branch is checked out; a new branch starts at the current tree's HEAD. Add `--no-space` for the worktree alone. Add `--from <dir>` to use another repository than the current directory's.

3. Work in the returned `path`. A `cd` alone may not last to the next command: start each shell command with `cd <path> &&`, and give file tools absolute paths under `path`. Do not edit the original tree for this task.

4. Delete it only when the user asks, or when its row in `paddock worktree list --json` has `"merged": true` (git's check; `pr` only reports what GitHub says of its pull request). Run `rm` from the main checkout (the row with `"main": true`), never from inside the worktree:

   ```sh
   cd <main checkout path> && paddock worktree rm <row's path> --json
   ```

   Pass the row's absolute `path` from `list --json`, not the branch name: a branch named like a directory in the repository is refused as ambiguous (exit 1).

   This closes its spaces, removes the worktree and deletes the branch with `git branch -d`. An unmerged branch is kept, with a notice on stderr. `rm` is not pre-approved: the user confirms each one.

## Never

- Never pass `--discard-changes` or `--discard-commits` unless the user said to lose that work.
- Never run `rm` from inside the worktree being removed. paddock refuses anyway.
- Never fall back to `git worktree remove` or `git branch -D` when `rm` refuses.

## Exit codes

| Code | Meaning | Do |
|---|---|---|
| 0 | ok | go on |
| 1 | usage or config error | fix the command; stderr says what is wrong |
| 2 | git or tmux error | read git's reason on stderr; report it |
| 3 | refused for a risk (uncommitted changes, unmerged commits, a Claude running or waiting there, not made by paddock, the main checkout, you are inside it) | stop and ask the user, quoting stderr |
| 4 | timeout; nothing deleted by the check (a `rm` past its checks may have closed spaces) | run `paddock worktree list --json`, then retry once if the worktree is still there; else tell the user |

Errors are one line on stderr.
