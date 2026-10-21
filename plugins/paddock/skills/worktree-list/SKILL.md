---
name: worktree-list
description: List the repository's worktrees with their paddock spaces, state and agents.
disable-model-invocation: true
allowed-tools: Bash(paddock worktree *)
---

Run `paddock worktree list` and show its output as it is.

If it fails, report stderr and the exit code.
