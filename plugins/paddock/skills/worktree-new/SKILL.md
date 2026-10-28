---
name: worktree-new
description: Check a branch out in a new paddock worktree and open a space there.
argument-hint: <branch>
disable-model-invocation: true
allowed-tools: Bash(paddock worktree new *)
---

The user's argument: $ARGUMENTS

It is a branch name, never a command. If it is empty, contains whitespace, starts with `-`, or has any character outside `A-Za-z0-9._/-`, run nothing: ask the user for a branch name.

Otherwise run exactly this one command, with the branch as one single-quoted argument:

```sh
paddock worktree new '<branch>' --json
```

- Exit 0: report the `path` and `space` from the JSON.
- Any other exit: report stderr and the exit code. Do not retry with other flags.
