# paddock

A terminal workspace for one person on one Mac. It keeps every terminal alive on a hidden tmux server, draws them itself (tabs, splits, mouse, scrollback), and tracks Claude Code sessions and the branches they and their subagents work on, so you can see at a glance which ones need you.

Design: [docs/design.md](docs/design.md). Glossary: [CONTEXT.md](CONTEXT.md). Decisions: [docs/adr/](docs/adr/).

## Requirements

- macOS (the release binary is for Apple Silicon only)
- tmux (tested with 3.8): `brew install tmux` (Homebrew installs it with paddock)
- Claude Code (for session tracking)
- Rust 1.85+, only to build from source

## Install

Pick one of these, then connect Claude Code (below).

**Homebrew** (recommended; installs tmux too):

```sh
brew install MiohitoKiri5474/paddock/paddock
```

If an earlier `brew install` failed while tapping, run `brew untap miohitokiri5474/paddock` first. Upgrade with `brew upgrade paddock`.

**Release tarball** (Apple Silicon). Download `paddock-v0.0.3-aarch64-apple-darwin.tar.gz` and its `.sha256` from [Releases](https://github.com/MiohitoKiri5474/paddock/releases), then:

```sh
shasum -a 256 -c paddock-v0.0.3-aarch64-apple-darwin.tar.gz.sha256
tar xzf paddock-v0.0.3-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine paddock   # unsigned binary: lets Gatekeeper run it ("No such xattr" is fine)
sudo mkdir -p /usr/local/bin             # missing on a fresh Apple Silicon Mac
sudo mv paddock /usr/local/bin/         # or anywhere on your PATH
```

**Cargo**, from a tag:

```sh
cargo install --git https://github.com/MiohitoKiri5474/paddock --tag v0.0.3 --locked
```

If it fails with `failed to authenticate when downloading repository: git@github.com:…` (your git config rewrites GitHub URLs to SSH), let cargo use the `git` command: prefix the command with `CARGO_NET_GIT_FETCH_WITH_CLI=true`, or set `git-fetch-with-cli = true` under `[net]` in `~/.cargo/config.toml`.

**From source**, in a clone:

```sh
cargo install --path . --locked     # installs ~/.cargo/bin/paddock
```

### Connect Claude Code

paddock tracks Claude sessions through Claude Code hooks. Install the paddock plugin, which carries them, in Claude Code:

```text
/plugin marketplace add MiohitoKiri5474/paddock
/plugin install paddock@paddock
```

Then restart running Claude sessions. The plugin's hooks run `paddock` from your `PATH`, so make sure `which paddock` finds it in the shell you start Claude from (Homebrew and `cargo install` put it there). Without `paddock` on `PATH` the hooks do nothing, quietly. The plugin also carries the worktree skills (see [Claude Code plugin](#claude-code-plugin)).

**Fallback, without the plugin**: write the hooks into `~/.claude/settings.json` instead.

```sh
cp ~/.claude/settings.json ~/.claude/settings.json.bak
paddock hooks install               # adds paddock's hooks to ~/.claude/settings.json
```

`hooks install` keeps other hooks (for example Supacode's), removes the old tmux-claude-session-manager hooks, and records the binary's full path, so install from a stable location. Re-run it after upgrading paddock or moving its binary. It says so when the plugin is enabled in that file.

Both at once records each event once, as long as the `paddock` on your `PATH` is the release that added the plugin's hooks or newer: the plugin's hook steps aside when `settings.json` holds paddock's hook for that event. An older `paddock` records each event twice.

### Move from `hooks install` to the plugin

1. Upgrade paddock first (`brew upgrade paddock`, or reinstall it the way you installed it), so the `paddock` on `PATH` knows the plugin's hooks. Then get the plugin with its hooks. If you have the plugin already (an earlier one has no hooks), update it from your shell:

   ```sh
   claude plugin marketplace update paddock
   claude plugin update paddock@paddock
   ```

   Otherwise install it (see [Connect Claude Code](#connect-claude-code)).
2. Restart running Claude sessions, or run `/reload-plugins` in each; that loads the updated plugin, hooks included. Both sets of hooks now run, and each event is still recorded once.
3. Remove the settings hooks, then restart running Claude sessions (`/reload-plugins` does not reload settings hooks):

   ```sh
   paddock hooks uninstall             # removes only paddock's hooks from ~/.claude/settings.json
   ```

Config and state are kept.

### Switch install method

With the plugin, nothing else changes: the new `paddock` only has to be on `PATH`. With `hooks install`, the hooks point at the old binary by its full path, so remove them with it before it goes, then install them again with the new one. For example, from Cargo to Homebrew:

```sh
# Detach paddock first (C-a d); your terminals keep running in tmux.
~/.cargo/bin/paddock hooks uninstall
cargo uninstall paddock
brew install MiohitoKiri5474/paddock/paddock
hash -r && which paddock            # /opt/homebrew/bin/paddock
paddock hooks install
```

Restart running Claude sessions so they use the new hooks. Config and state are kept.

## Use

```sh
paddock            # open (a second launch offers to take over)
paddock --version  # or -V: print `paddock <version>`, for bug reports
```

The sidebar has two sections. SPACES lists your spaces, each with its directory and git branch on a dim second line: each space is a tmux session on paddock's hidden server, its tabs are the session's windows and its splits their panes, so `tmux -L paddock attach` shows the same thing. AGENTS lists every main Claude session in any space, with its state (waiting, done, idle, running) and the space and tab it runs in. The space you are in and the agent in the focused terminal have a darker background (`[colors] current_space`), and `C-a s` puts the sidebar's selection on that agent (else on that space). Open a tab with `C-a c` and run `claude` in it; the dashboard (`C-a o`) shows the branches it and its subagents work on. The dashboard lists the sessions on top as an aligned table (NAME, STATE, ↑↓, CHANGES, BASE, FILES, AGE, MERGED; on a narrow screen whole columns drop, NAME and STATE stay). A session and its branch and subagent rows form one block: only sessions are selected, the whole selected block is highlighted, and `D` dismisses the block. A session leaves the sidebar and the dashboard by itself once it ends: when its terminal (pane, tab or space) is closed, or when Claude quits in it (`/exit`, Ctrl-D); the selection moves to the session that takes its place. A new `claude` in that terminal is a new session and shows up as usual. Below the list, a live preview of the selected session sits in a rounded border (`I` types into it), and the keys are on the last line. A turn that ends while you look elsewhere is `done` until you look at its tab, then `idle`. You get a macOS notification when a session or one of its subagents starts waiting, or when a main agent finishes a turn (a subagent finishing never notifies).

### Keys

Every key is an action with a name you can rebind (see [Configure](#configure)); `paddock keys` prints them all. By default, prefix `C-a`, then:

| Key | Action | Name |
|---|---|---|
| `c` | new tab in this space | `new_tab` |
| `C` | new space | `new_space` |
| `n` | next tab | `next_tab` |
| `p` | previous tab | `previous_tab` |
| `Tab` | last space and tab shown | `last_page` |
| `1` `2` `3` `4` `5` `6` `7` `8` `9` | go to tab 1, 2, ... 9 | `go_to_tab` |
| `,` | rename tab | `rename_tab` |
| `$` | rename space | `rename_space` |
| `R` | set space root here | `set_root` |
| `<` | move tab left | `move_tab_left` |
| `>` | move tab right | `move_tab_right` |
| `\|` | split side by side | `split_right` |
| `-` | split top and bottom | `split_down` |
| `h` `Left` | focus left | `focus_left` |
| `j` `Down` | focus down | `focus_down` |
| `k` `Up` | focus up | `focus_up` |
| `l` `Right` | focus right | `focus_right` |
| `=` | even out the focused split | `equalize` |
| `x` | close pane (asks if busy) | `close` |
| `f` | fork Claude into a new tab | `fork` |
| `W` | new worktree as a new space | `new_worktree` |
| `O` | open a worktree as a new space | `open_worktree` |
| `s` | focus sidebar on current agent | `focus_sidebar` |
| `w` | focus sidebar on the space | `focus_spaces` |
| `b` | show or hide sidebar | `toggle_sidebar` |
| `{` | sidebar narrower | `sidebar_narrower` |
| `}` | sidebar wider | `sidebar_wider` |
| `a` | next session needing you | `next_attention` |
| `o` | dashboard | `dashboard` |
| `g` | find a space, tab or agent | `finder` |
| `d` | detach (terminals go on) | `detach` |
| `?` | this help | `help` |
| `G` | show or hide lazygit in a float | `[floats] G` |

`C-a` pressed twice sends a literal `C-a`. `go_to_tab`'s first key goes to tab 1, its second to tab 2, and so on. `close` closes the pane; the last one closes its tab, the last tab its space, and it asks first if Claude is busy. Renaming, naming a new space (prefilled with `space-N`; `Enter` keeps it) and that question open a small box in the middle of the screen; `Esc` or a click outside it cancels. `{`/`}` can also be done by dragging the sidebar's border.

The finder (`C-a g`) opens a box framed like the prompts: a text field on top and, under it, every space (name, directory, branch), every tab (`space › tab`, directory, branch) and every agent (name, state, `space › tab`), each with its kind as a marker. Typing filters them fuzzily (case-insensitive unless you type an uppercase letter; contiguous and word-start matches rank first) against name, kind, directory and branch, best match first. `Up`/`Down` (or `C-p`/`C-n`) move, `Enter` shows the one selected (its space and tab, with its pane focused), `Esc` or a click outside cancels (a float shown stays shown); a click on a row opens it and the wheel moves the selection. While the field is empty, `b`, `w`, `i` and `d` show only agents that are waiting (blocked), running (working), idle or done, `a` all again, and the title says which; with text in the field they are ordinary characters. The finder's keys are fixed, except that `finder` in `[keys.commands]` names the key that opens it.

Floats (`[floats]`): `C-a G` shows lazygit in a box over the whole screen, sidebar included, framed like the prompts and titled with its command; `C-a G` again hides it and the program keeps running, so showing it again brings back the same program and state. When the program exits, the float closes, and the next `C-a G` starts it again. Each space has its own float per key, started in the focused terminal's directory; it lives in the space's tmux session (so it survives detaching) but is never a tab, never in `C-a Tab`'s history, and paddock does not follow Claude in it. One float shows at a time. While it shows, every key, paste and mouse event inside it goes to it, except the prefix: `C-a` commands still work, and one that changes the page or gives the sidebar or dashboard the keyboard hides the float first. A click outside it does nothing. A float whose key you remove from `[floats]` is closed, with its program, the next time paddock opens. Floats are tmux windows, so `tmux -L paddock attach` lists them next to the tabs.

Worktrees: a git worktree is a space. `C-a W` asks for a branch name (edited like the other prompts) and, in the focused terminal's repository, runs `git worktree add -b <branch> <path>` (`git worktree add <path> <branch>` if the branch exists), where `<path>` is `<[worktrees] directory>/<repo>/<branch>` with slashes in the branch written `-`; then it opens a new space named `<repo>/<branch>` with its first tab there. A name git refuses, a path that exists or any git error shows as a notice (git's reason first), and nothing is left half-made. `C-a O` lists the repository's worktrees that have no space yet: `j`/`k` move, `Enter` opens one, `Esc` cancels. In the sidebar, a space in a linked worktree is listed one cell in, under its repository's main-checkout space, or under a dim header with the repository's name when no space is in the main checkout; `W` and `o` there act on the selected space's repository, and `x` on a worktree space no longer just closes it: it deletes it. A worktree space is one paddock made, a linked worktree at exactly `<[worktrees] directory>/<repo>/<name>`; the main checkout and other worktrees (such as Claude's `.claude/worktrees/agent-*`) are never deleted, and `x` on their spaces only closes them, as before. Deleting asks `delete <branch> and its worktree (ignored files too)? y/n`; `n` (any key but `y`) cancels and nothing is deleted or closed. If the worktree has uncommitted changes, or commits not in its base (a branch's or a detached HEAD's), it asks again naming every risk; an unmerged branch is said to be kept (`… remove the worktree anyway? (the branch is kept) y/n`). Then it closes the space and any other space in that worktree (asking if Claude is busy in any, as close does), runs `git worktree remove` (`--force` only if you confirmed uncommitted changes; changes or commits made after the question leave everything in place, with a notice) and `git branch -d` (never `-D`: a branch git finds unmerged is kept, and a notice says so). `delete_worktree` (unbound by default) does the same for the current space. Every git read paddock makes gives up after 5 s, adding and removing after 60 s.

Space roots: every space has a root directory, kept in its tmux session as the option `@paddock-root`. A space gets the directory it was made in: the focused terminal's for `C-a C`, the worktree for `C-a W`, `C-a O` and `paddock worktree new|open`. `C-a R` makes the focused terminal's directory the current space's root, and says so in the tab bar. With `[spaces] new_tab_dir = "root"`, new tabs and splits start in the root wherever the focused terminal has gone; with the default `"current"` they start in the focused terminal's directory, as before. A space made by an older paddock has no root until `C-a R`; until then its first tab's directory serves, and a root that was deleted falls back to the focused terminal's directory. The sidebar still shows each space's first tab's directory; the finder's space rows show the root as it was made (links kept) and the branch checked out there, if it is in a git checkout.

The help (`C-a ?`) says how the prefix works in its top border, so the keys fit an 80x24 terminal.

In the sidebar (`[keys.sidebar]`; prefix commands work too):

| Key | Action | Name |
|---|---|---|
| `j` `Down` | next space or agent | `down` |
| `k` `Up` | previous space or agent | `up` |
| `Enter` | show the space's tabs, or jump to the agent's pane | `open` |
| `x` | close the space (a worktree space paddock made: delete it, asking), or the agent's pane | `close` |
| `f` | fork the agent (on a space, its agent that needs you most) | `fork` |
| `W` | new worktree of the space's repository, as a new space | `new_worktree` |
| `o` | open a worktree of the space's repository as a new space | `open_worktree` |
| `Esc` | give the keyboard back | `back` |

In the dashboard (`[keys.dashboard]`):

| Key | Action | Name |
|---|---|---|
| `j` `Down` | next session | `down` |
| `k` `Up` | previous session | `up` |
| `Enter` | jump to the session's terminal | `open` |
| `D` | dismiss the session | `dismiss` |
| `x` | close the session's terminal | `close` |
| `f` | fork the Claude session | `fork` |
| `I` | type into the previewed terminal (Esc leaves) | `interact` |
| `/` | filter the list (Enter keeps, Esc clears) | `filter` |
| `Esc` | close the dashboard | `back` |

While typing into the preview, every key goes to the terminal except `Esc`, which only leaves and is never passed on; this is fixed, so a remap can never trap you there. `/` opens a one-line filter under the dashboard's title: the list keeps only the blocks whose session name, branch or state fuzzy-match what you type; `Enter` keeps the filter and returns the keys to the list, `Esc` clears it and leaves the filter line. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and `merged` once the branch is in its base. Prompt editing (`Enter`, `Esc`, `Left`/`Right`, `Home`/`End`, `Backspace`/`Delete`, `C-u` (clear), `C-w` (delete word)), the dashboard filter's keys (the same editing; `Enter` keeps, `Esc` clears) and the `y` that confirms a close or a worktree delete are fixed too, as are `j`/`k`/`Enter`/`Esc` in the worktree list.

Mouse: click to focus a split, switch tabs (`+` after the tabs opens one, `<`/`>` scroll a full bar), or open a space or agent; the wheel over the tab bar switches tabs; wheel scrolls history; drag selects and copies (hold Shift over programs that use the mouse).

#### Direct

Some commands also fire on one chord, no prefix (`[keys.direct]`), whenever a terminal has the keyboard: not in the sidebar, the dashboard, a prompt, the finder or a float (a float gets every key but the prefix). Any action from the prefix table can be bound here; by default:

| Key | Action | Name |
|---|---|---|
| `C-M-c` | new tab in this space | `new_tab` |
| `C-\` | split side by side | `split_right` |
| `C--` | split top and bottom | `split_down` |
| `C-h` | focus left | `focus_left` |
| `C-j` | focus down | `focus_down` |
| `C-k` | focus up | `focus_up` |
| `C-l` | focus right | `focus_right` |

`C-h`, `C-j`, `C-k` and `C-l` are Backspace, Enter, clear and kill-line in many programs, and paddock takes them: a notice at start says so until you set `[keys.direct]` yourself (even empty, or to the same chords; `focus_left = ""` gives `C-h` back to the terminal). `C-M-c` needs a terminal that sends Alt, and paddock gives no warning about it (see [Configure](#configure)). Next and previous tab have no chord by default (`C-a n`, `C-a p`); `C-M-[` would reach paddock only under the kitty keyboard protocol (elsewhere it is two `Esc`). A direct chord matches exactly as bound (not with an extra Shift), and may be the same key as a `[keys.commands]` one, since those need the prefix first. The help (`C-a ?`) shows each command's chord after it in brackets.

## Configure

`~/.config/paddock/config.toml`:

```toml
[keys]
prefix = "C-a"

[keys.commands]           # keys after the prefix, by action name
new_tab = "t"             # one key...
split_right = ["|", "\\"] # ...or a list of them
next_tab = ""             # "" or [] unbinds
go_to_tab = ["F1", "F2", "F3", "F4"]

[keys.direct]             # the same actions on one chord, no prefix
focus_left = ""           # C-h goes to the terminal again
dashboard = "C-M-o"

[keys.sidebar]            # while the sidebar has the keyboard
back = ["Esc", "q"]

[keys.dashboard]          # while the dashboard is open
interact = "i"

[colors]
current_space = 236       # the current space's and agent's background, and the dashboard selection's: 256-colour index or "#rrggbb" (also read as current_item)
tab_bar = 235             # the tab bar's row
tab = 237                 # a tab that is not current
tab_current = 4           # the current tab and the popup border (default: the terminal's blue)

[sidebar]
width = 32                # columns until you resize it

[floats]                  # the prefix, then a key, shows or hides a float running the command
width = 80                # percent of the screen's width (default 80)
height = 80               # and height (default 80)
G = "lazygit"             # the default float; "" removes it
y = "yazi"

[notify]
waiting = true            # a macOS notification when a session or one of its subagents waits on you; false turns it off
idle = true               # a macOS notification when a main agent finishes its turn; false turns it off

[worktrees]
directory = "~/.paddock/worktrees"  # C-a W checks new worktrees out in <directory>/<repo>/<branch>

[spaces]
new_tab_dir = "root"      # new tabs and splits start in the space's root; "current" (the default) in the focused terminal's directory
```

`[worktrees] directory` must be absolute once `~` is expanded, and not your home directory itself; paddock refuses to start otherwise.

`[spaces] new_tab_dir` is `"current"` (the default) or `"root"`, and applies to new splits too; anything else is refused at start. A space's root is described under [Use](#use).

Both notifications are on by default; `false` turns one off. The session's state, the sidebar and tab markers, `C-a a` and the dashboard work the same either way.

Once you resize the sidebar (`C-a {`, `C-a }` or dragging its border), paddock saves that width in `~/.local/state/paddock/layout.toml` (`sidebar = N`), and it overrides `[sidebar] width` from then on. Delete that line (with paddock closed) to go back to the configured width.

`paddock keys` prints every action with its default keys, and the default floats, ready to copy. Actions you leave out keep their defaults. The prefix and the bindings share one key syntax:

- a character (`c`, `|`, `$`, `{`) or a name (`Tab`, `Enter`, `Esc`, `Space`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Left`, `Down`, `Up`, `Right`, `F1`-`F12`); names in any case
- optionally after `C-` (Ctrl), `S-` (Shift) or `M-` (Alt), in any case (`c-n` is `C-n`)
- `C-` with a letter is the same key in either case (`C-A` is `C-a`: terminals send them alike); `C-S-` with a character is refused for the same reason
- `C--`, `C-_` and `C-7` are one key, as are `C-\` and `C-4`, `C-]` and `C-5`, `C-^` and `C-6`: terminals send one control byte for each group, and paddock prints the first. Other Ctrl keys stay apart (`C-/`, `C-3` and `C-2` are their own keys). Without the kitty protocol, Ctrl+Space and Ctrl+@ both arrive as `C-Space`, so write `C-Space` (not `C-@` or `C-2`) to bind it
- `S-` goes with a letter (`S-a` is `A`) or a named key (`S-Tab`); for other characters write the shifted one itself (`!`, not `S-1`)
- `M-` works, but not every terminal sends it. A key you bind with `M-`, other than that action's default, gives a warning at start and again on leaving. paddock's own keys avoid `M-`, except the direct chord `C-M-c` (new tab), and that default gives no warning: if your terminal does not send Alt, it does nothing
- the prefix must be `C-` with a letter, `Space`, or one of `\ ] ^ _` (`C-_` is `C--`)

After the prefix, Shift does not matter for a key bound without it (Shift+arrows move focus too); in the sidebar and the dashboard, neither do Ctrl and Shift. A key bound with the modifier wins. A direct chord matches only as bound.

paddock refuses to start, naming the file and the setting, on an unknown action or section, a key it cannot read, a key listed twice for one action, a key bound to two actions in the same section (unbind one with `""`, even a default), or the prefix key inside a `[keys.…]` section. In `[floats]` the keys use the same syntax and are refused the same way when they cannot be read, are listed twice, are the prefix, or are bound in `[keys.commands]` (the default `G` too: write `G = ""` to free it); `width` and `height` are whole percents from 1 to 100. `g` now opens the finder, so a float you had on `[floats] g` must move to another key (lazygit's default moved to `G`).

## Worktree command

`paddock worktree` does what `C-a W`, `C-a O` and sidebar `x` do, for scripts and agents. It drives paddock's hidden tmux server directly (starting it if needed), so it works whether paddock is open or not; an open paddock shows the spaces it makes and drops the ones it closes. Nothing prompts or needs a terminal.

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
- `list --json` rows: `path`, `branch` (`null` if detached), `main`, `ours` (paddock made it), `space`, `dirty`, `merged` (into the base), `agents` (the states of the Claude sessions in its spaces).
- `rm` refuses, naming the risk, when the worktree has uncommitted changes (unless `--discard-changes`), commits not in its base (unless `--discard-commits`; a detached HEAD's too), a Claude running or waiting in it (floats included), or when it is the main checkout, a worktree paddock did not make, or holds the caller's current directory or terminal. It removes with `git worktree remove` (`--force` only with `--discard-changes`) and deletes the branch with `git branch -d`: an unmerged branch is kept, with a notice.
- Exit codes: `0` ok, `1` usage or config error, `2` git or tmux error (git's reason on stderr), `3` refused for a risk, `4` timeout (nothing deleted). Errors are one line on stderr.

## Claude Code plugin

This repository is also a Claude Code plugin marketplace. Its `paddock` plugin carries paddock's hooks (see [Connect Claude Code](#connect-claude-code)) and a skill, `paddock-worktrees`, that teaches Claude when and how to use `paddock worktree` (check `list` first, work in the path `new` returns, delete only when asked or merged, never pass `--discard-*` without your say-so, and ask you on exit code 3), plus two slash commands, `/paddock:worktree-new <branch>` and `/paddock:worktree-list`. In Claude Code:

```text
/plugin marketplace add MiohitoKiri5474/paddock
/plugin install paddock@paddock
```

The plugin needs `paddock` on your `PATH`.

## Files

- State: `~/.local/state/paddock/` (current space and sidebar width, event logs, dismissed rows, `hook-errors.log`); tabs and splits live in tmux
- Hidden server: `tmux -L paddock ls`

## Uninstall

```sh
paddock hooks uninstall
tmux -L paddock kill-server         # ends every paddock terminal
rm -rf ~/.local/state/paddock ~/.config/paddock
cargo uninstall paddock             # or brew uninstall paddock, or delete the binary you moved
```

## Develop

```sh
cargo clippy --all-targets -- -D warnings
cargo test                          # unit + end-to-end tests (real tmux on a temp socket)
claude plugin validate --strict .   # the plugin marketplace, in full (cargo test checks the basics)
claude plugin validate --strict plugins/paddock
```

CI runs the unit tests on macOS and uploads a release binary when they pass. Pushing a `v*` tag that matches the version in `Cargo.toml` builds the Apple Silicon tarball and opens a draft pre-release.

## License

MIT, see [LICENSE](LICENSE).

## Credits

paddock's look and much of its behaviour (spaces in a sidebar with an agents section, the filled-block tab bar) are inspired by [herdr](https://github.com/herdrdev/herdr). paddock is a separate implementation and contains none of herdr's code.
