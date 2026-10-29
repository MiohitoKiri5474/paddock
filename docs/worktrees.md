# Worktrees

Worktrees as spaces, the `paddock worktree` command, and the worktree skills for each agent. Back to the [README](../README.md).

## Worktree keys

Worktrees: a git worktree is a space. `C-a W` asks for a branch name (edited like the other prompts) and, in the focused terminal's repository, runs `git worktree add -b <branch> <path>` (`git worktree add <path> <branch>` if the branch exists), where `<path>` is `<[worktrees] directory>/<repo>/<branch>` with slashes in the branch written `-`; then it opens a new space named `<repo>/<branch>` with its first tab there. A name git refuses, a path that exists or any git error shows as a notice (git's reason first), and nothing is left half-made. `C-a O` lists the repository's worktrees that have no space yet: `j`/`k` move, `Enter` opens one, `Esc` cancels. In the sidebar, a space in a linked worktree is listed one cell in, under its repository's main-checkout space, or under a dim header with the repository's name when no space is in the main checkout; `W` and `o` there act on the selected space's repository, and `x` on a worktree space no longer just closes it: it deletes it. A worktree space is one paddock made, a linked worktree at exactly `<[worktrees] directory>/<repo>/<name>`; the main checkout and other worktrees (such as Claude's `.claude/worktrees/agent-*`) are never deleted, and `x` on their spaces only closes them, as before. Deleting asks `delete <branch> and its worktree (ignored files too)? y/n`; `n` (any key but `y`) cancels and nothing is deleted or closed. If the worktree has uncommitted changes, or commits not in its base (a branch's or a detached HEAD's), it asks again naming every risk; an unmerged branch is said to be kept (`… remove the worktree anyway? (the branch is kept) y/n`). Then it closes the space and any other space in that worktree (asking if an agent is busy in any, as close does), runs `git worktree remove` (`--force` only if you confirmed uncommitted changes; changes or commits made after the question leave everything in place, with a notice) and `git branch -d` (never `-D`: a branch git finds unmerged is kept, and a notice says so). `delete_worktree` (unbound by default) does the same for the current space. Every git read paddock makes gives up after 5 s, adding and removing after 60 s.

## Worktree command

`paddock worktree` does what `C-a W`, `C-a O` and sidebar `x` do, for scripts and agents. It drives paddock's server directly (starting it if needed), so it works whether paddock is open or not; an open paddock shows the spaces it makes and drops the ones it closes. Nothing prompts or needs a terminal.

```sh
paddock worktree new feat/x            # new worktree at <[worktrees] directory>/<repo>/feat-x and a space repo/feat/x; prints the path
paddock worktree new feat/x --no-space # the worktree alone
paddock worktree open feat/x           # a space for a worktree that has none (or by path)
paddock worktree list                  # one aligned line per worktree
paddock worktree rm feat/x             # close its spaces, remove it, delete the branch if merged
```

- Every command takes `--json` (a stable JSON object, or an array for `list`) and `--from <dir>` (the repository of `<dir>`; the current directory's without it). `paddock worktree --help` lists them.
- `<branch|path>` is a directory (relative to `--from`) or a branch. If it is one worktree's path and another's branch, the command refuses (exit 1) and names both: give the full path.
- `new` checks the branch out if it exists, else creates it at the current tree's HEAD. If anything fails, nothing is left half-made. With `--json`: `{"path","branch","repo","space"}`.
- `list --json` rows: `path`, `branch` (`null` if detached), `main`, `ours` (paddock made it), `space`, `dirty`, `merged` (into the base, as git sees it), `pr` (the branch's pull request: `null`, or `{"number", "state", "checks", "review", "url"}` with `state` one of `open`, `draft`, `merged`, `closed`; `checks` `passing`, `failing`, `pending` or `null`; `review` `approved`, `changes_requested`, `review_required` or `null`), `agents` (the states of the agent sessions in its spaces). The text form shows the PR as `#123 open`, else `-`, after `merged`/`unmerged`. `list` asks `gh` once for the repository (5 s limit, no cache); without `gh`, when it fails, or with `[github] refresh_secs = 0` (then it never runs), every `pr` is `null` and `list` still exits 0.
- `rm` refuses, naming the risk, when the worktree has uncommitted changes (unless `--discard-changes`), commits not in its base (unless `--discard-commits`; a detached HEAD's too), an agent running or waiting in it (floats included), or when it is the main checkout, a worktree paddock did not make, or holds the caller's current directory or terminal. It removes with `git worktree remove` (`--force` only with `--discard-changes`) and deletes the branch with `git branch -d`: an unmerged branch is kept, with a notice.
- Exit codes: `0` ok, `1` usage or config error, `2` git or backend error (git's reason on stderr), `3` refused for a risk, `4` timeout (nothing deleted). Errors are one line on stderr.

## Worktree skills for agents

Each coding agent paddock tracks (each agent kind: Claude Code, Gemini CLI, Antigravity CLI) gets a skill, `paddock-worktrees`, that teaches it when and how to use `paddock worktree` (check `list` first, work in the path `new` returns, delete only when asked or merged and ask you before each delete, never pass `--discard-*` without your say-so, and ask you on exit code 3), and, where the CLI loads commands, two commands to run `paddock worktree new <branch>` and `paddock worktree list`. The `new` command takes the branch you type as one quoted argument and asks you again for anything but `A-Za-z0-9._/-`. Each needs `paddock` on your `PATH`.

Claude Code's plugin pre-approves only `paddock worktree list`, `new` and `open`, so Claude Code asks you before each `rm` whatever the model does. Gemini CLI and agy have no such list: they rely on their own approval mode (in an auto-approve or yolo mode nothing asks), and only the skill's instruction to ask stands between them and a delete.

The Gemini CLI and agy installs below read the repository's default branch, `main`, which only releases reach: a change on `develop` reaches them with the next release.

### Claude Code

This repository is a Claude Code plugin marketplace. Its `paddock` plugin carries paddock's hooks (see [Connect Claude Code](connect-agents.md#connect-claude-code)), the skill, and two slash commands, `/paddock:worktree-new <branch>` and `/paddock:worktree-list`. In Claude Code:

```text
/plugin marketplace add MiohitoKiri5474/paddock
/plugin install paddock@paddock
```

### Gemini CLI

The repository is also a Gemini CLI extension (`gemini-extension.json` at its root; it needs Gemini CLI 0.26 or newer, where Agent Skills are on by default, and was checked with `gemini extensions validate` from Gemini CLI 0.65). It carries the skill and two commands, `/paddock:worktree-new <branch>` and `/paddock:worktree-list`, and no hooks: those come from `paddock hooks install --agent gemini` (see [Connect Gemini CLI](connect-agents.md#connect-gemini-cli)). From your shell:

```sh
gemini extensions install https://github.com/MiohitoKiri5474/paddock
gemini extensions update paddock      # after a paddock release
gemini extensions uninstall paddock
```

Gemini copies the whole repository into `~/.gemini/extensions/paddock`; restart running Gemini sessions afterwards.

### Antigravity CLI

agy installs the same extension as a plugin, converting it on install (`agy plugin validate`, agy 1.4.5, reports it as a Gemini CLI extension that `plugin install` converts). Expect the skill alone, so ask agy in words: agy's plugin validator warns that plugins do not load `commands/`, and an extension agy converted on install kept its `commands/` as they were (seen with agy 1.4.5; not checked with this one). From your shell:

```sh
agy plugin install https://github.com/MiohitoKiri5474/paddock
agy plugin uninstall paddock
```

The uninstall name is assumed, not checked: agy named another converted extension after its `name`. If it is not found, use the name `/plugin` lists. Restart running agy sessions afterwards. Do not give agy the Claude Code plugin (`agy plugin import claude`, or `plugins/paddock` as a local plugin): agy would run its Claude hooks, and paddock would record agy's events as a Claude session's.
