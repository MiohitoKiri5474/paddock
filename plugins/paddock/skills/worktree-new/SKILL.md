---
name: worktree-new
description: Check a branch out in a new paddock worktree and open a space there.
argument-hint: <branch>
disable-model-invocation: true
allowed-tools: Bash(paddock worktree *)
---

Run `paddock worktree new $ARGUMENTS --json`.

- Exit 0: report the `path` and `space` from the JSON.
- Any other exit: report stderr and the exit code. Do not retry with other flags.
