# paddock

A terminal workspace for one person on one Mac. It keeps every terminal alive on its own background server (`paddock server`), draws them itself (tabs, splits, mouse, scrollback), and tracks coding agents' sessions (Claude Code, Gemini CLI and Antigravity CLI) and the branches they and their subagents work on, so you can see at a glance which ones need you.

Design: [docs/design.md](docs/design.md). Glossary: [CONTEXT.md](CONTEXT.md). Decisions: [docs/adr/](docs/adr/).

## Requirements

- macOS (the release binary is for Apple Silicon only)
- tmux (tested with 3.8), only for the legacy tmux backend (`[backend] kind = "tmux"`, kept until v0.2): `brew install tmux` (Homebrew installs it with paddock)
- Claude Code, Gemini CLI and/or Antigravity CLI (for session tracking; Gemini CLI tested with 0.64, Antigravity CLI with 1.4.3)
- Rust 1.85+, only to build from source
- Optional: the GitHub CLI (`gh`, logged in), for pull request status on the dashboard, in the sidebar and in `paddock worktree list`

## Install

Pick one of these, then connect Claude Code, Gemini CLI or Antigravity CLI (below).

**Homebrew** (recommended):

```sh
brew install MiohitoKiri5474/paddock/paddock
```

If an earlier `brew install` failed while tapping, run `brew untap miohitokiri5474/paddock` first. Upgrade with `brew upgrade paddock` (see [Upgrade](#upgrade)).

**Release tarball** (Apple Silicon). Download `paddock-v0.1.0-rc.1-aarch64-apple-darwin.tar.gz` and its `.sha256` from [Releases](https://github.com/MiohitoKiri5474/paddock/releases), then:

```sh
shasum -a 256 -c paddock-v0.1.0-rc.1-aarch64-apple-darwin.tar.gz.sha256
tar xzf paddock-v0.1.0-rc.1-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine paddock   # unsigned binary: lets Gatekeeper run it ("No such xattr" is fine)
sudo mkdir -p /usr/local/bin             # missing on a fresh Apple Silicon Mac
sudo mv paddock /usr/local/bin/         # or anywhere on your PATH
```

**Cargo**, from a tag:

```sh
cargo install --git https://github.com/MiohitoKiri5474/paddock --tag v0.1.0-rc.1 --locked
```

If it fails with `failed to authenticate when downloading repository: git@github.com:…` (your git config rewrites GitHub URLs to SSH), let cargo use the `git` command: prefix the command with `CARGO_NET_GIT_FETCH_WITH_CLI=true`, or set `git-fetch-with-cli = true` under `[net]` in `~/.cargo/config.toml`.

**From source**, in a clone:

```sh
cargo install --path . --locked     # installs ~/.cargo/bin/paddock
```

### Upgrade

Your terminals run on paddock's server, which keeps running after you leave paddock. A new paddock whose server speaks another protocol version says so and offers to restart it: the programs in its terminals end, and its layout (spaces, tabs, splits and their directories, saved in `~/.local/state/paddock/layout.json`) comes back with a fresh shell in each terminal, under the scrollback it had (saved in `~/.local/state/paddock/scrollback/` as the server ends, up to 1 MiB of text per terminal, below a dim line saying when; searchable in copy mode like any scrollback). After a crash or a SIGKILL the server saves nothing, so each terminal shows the history saved at the last clean end, under that older date (or none). The first upgrade to a paddock that saves it restores none, as the older server wrote none. Say no and paddock stops; leave paddock and finish what runs first, then start it again. Floats and programs other than shells (an agent, an editor) are not restarted.

**From paddock 0.0.x (tmux).** paddock now runs on its own server by default, not on a hidden tmux server. The old `tmux -L paddock` server is not migrated: its spaces and programs stay where they are, and the new paddock starts empty. While that server runs, paddock says so in the tab bar, and prints it again however it ends (on leaving, or on an error at start), once per server: once printed, it remembers that server's pid and start time in `~/.local/state/paddock/old-tmux-server`, and a different server is said again. Finish what runs there first: `tmux -L paddock attach` attaches to that old server, with the old paddock's spaces as tmux sessions. Then close it:

```sh
tmux -L paddock kill-server         # ends every terminal of the old paddock
```

The new paddock's terminals live on its own server, so `tmux -L paddock attach` never shows them. Hooks in paddock's terminals get `PADDOCK_PANE` and `PADDOCK_SOCKET` instead of `TMUX` and `TMUX_PANE`, by which `paddock hook` tells which terminal an event comes from; a script of yours that read `TMUX_PANE` should read `PADDOCK_PANE` now. To keep the tmux backend until v0.2, set `[backend] kind = "tmux"` (see [Configure](#configure)).

### Connect Claude Code

paddock tracks Claude sessions through Claude Code hooks. Install the paddock plugin, which carries them, in Claude Code:

```text
/plugin marketplace add MiohitoKiri5474/paddock
/plugin install paddock@paddock
```

Then restart running Claude sessions. The plugin's hooks run `paddock` from your `PATH`, so make sure `which paddock` finds it in the shell you start Claude from (Homebrew and `cargo install` put it there). Without `paddock` on `PATH` the hooks do nothing, quietly. The plugin also carries the worktree skills (see [Worktree skills for agents](#worktree-skills-for-agents)).

**Fallback, without the plugin**: write the hooks into `~/.claude/settings.json` instead.

```sh
cp ~/.claude/settings.json ~/.claude/settings.json.bak
paddock hooks install               # adds paddock's hooks to ~/.claude/settings.json
```

`hooks install` keeps other hooks (for example Supacode's), removes the old tmux-claude-session-manager hooks, and records the binary's full path, so install from a stable location. Re-run it after upgrading paddock or moving its binary. It says so when the plugin is enabled in that file.

`hooks install` and `hooks uninstall` take `--agent <kind>`, repeatable, to act only on that agent's hooks: `claude`, `gemini` or `agy`. Without it they act on Claude Code, as before, on Gemini CLI only when `gemini` is on your `PATH` (see [Connect Gemini CLI](#connect-gemini-cli)), and on Antigravity CLI only when `agy` is (see [Connect Antigravity CLI](#connect-antigravity-cli)). Each run reads every settings file it acts on before writing any: if one can't be read or parsed, it names that file, exits 1 and changes none. Each file is replaced atomically, but a write that fails partway (a full disk, say) can leave the files written before it. `hooks uninstall` never creates a settings file that does not exist, and keeps lists you left empty in it.

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
   paddock hooks uninstall --agent claude   # removes only paddock's hooks from ~/.claude/settings.json
   ```

Config and state are kept.

### Switch install method

With the plugin, nothing else changes: the new `paddock` only has to be on `PATH`. With `hooks install`, the hooks point at the old binary by its full path, so remove them with it before it goes, then install them again with the new one. For example, from Cargo to Homebrew:

```sh
# Detach paddock first (C-a d); your terminals keep running on its server.
~/.cargo/bin/paddock hooks uninstall --agent claude --agent gemini --agent agy
cargo uninstall paddock
brew install MiohitoKiri5474/paddock/paddock
hash -r && which paddock            # /opt/homebrew/bin/paddock
paddock hooks install --agent claude --agent gemini --agent agy
```

Name each agent whose hooks you installed (drop `--agent gemini` or `--agent agy` for one you never connected): a plain `hooks uninstall` or `hooks install` covers Gemini only while `gemini` is on your `PATH`, and agy only while `agy` is. Restart running sessions so they use the new hooks. Config and state are kept.

### Connect Gemini CLI

paddock tracks Gemini CLI sessions (hooks need Gemini CLI 0.26 or newer; tested with 0.64) through Gemini's hooks in `~/.gemini/settings.json` (`$GEMINI_CLI_HOME/.gemini/settings.json` if set). paddock's Gemini extension carries the worktree skill and commands but no hooks (see [Worktree skills for agents](#worktree-skills-for-agents)): install the hooks with paddock.

```sh
cp ~/.gemini/settings.json ~/.gemini/settings.json.bak
paddock hooks install --agent gemini   # adds paddock's hooks to ~/.gemini/settings.json
paddock hooks uninstall --agent gemini # removes only paddock's hooks
```

A plain `paddock hooks install` includes Gemini when `gemini` is on your `PATH`. The hooks (`SessionStart`, `BeforeAgent`, `AfterTool`, `Notification`, `AfterAgent`, `SessionEnd`) call `paddock hook --agent gemini <event>` by paddock's full path, with a 10 s timeout, so re-run the install after moving paddock's binary; your other hooks and settings stay. paddock cannot edit a `settings.json` with comments (Gemini allows them): it says `invalid JSON` and leaves the file as it was, so remove the comments or add the hooks by hand. A plain `paddock hooks install` with `gemini` on your `PATH` then changes no file at all, Claude's included: use `--agent claude` to skip Gemini's. Restart running Gemini sessions afterwards.

What Gemini sessions get: their row, dot, tab marker, state (running after a prompt, waiting on a tool permission, done or idle when a turn ends, gone when Gemini quits; `/clear` keeps the terminal's session), branch and task rows, `C-a a`, notifications, the EXTERNAL group and the AGENT tag `gemini`. What stays Claude Code's alone: fork, subagent rows (Gemini's subagents are not shown) and the `claude agents --json` snapshot. Without a snapshot, nothing catches what Gemini's hooks miss: after you press Esc or deny a permission, the row stays running or waiting until the next prompt. Gemini runs no hooks in a folder it does not trust (folder trust is on by default), so such a session does not show. An external Gemini session is kept alive by its hooks alone: waiting on you, it stays in the EXTERNAL group until it ends, you dismiss it, or nothing has been heard of it for 24 hours (as when Gemini was killed while waiting); in any other state, quiet for `[dashboard] external_timeout` minutes (idle at its prompt, say), it leaves until its next hook.

### Connect Antigravity CLI

paddock tracks Antigravity CLI (`agy`; tested with 1.4.3) sessions through agy's hooks in `~/.gemini/config/hooks.json`, a file keyed by hook name, and, in paddock's terminals, through the status agy reports to its terminal (OSC 7501, agy 1.4.0 or newer). paddock's agy plugin carries the worktree skill but no hooks (see [Worktree skills for agents](#worktree-skills-for-agents)): install the hooks with paddock.

```sh
cp ~/.gemini/config/hooks.json ~/.gemini/config/hooks.json.bak   # if you have one
paddock hooks install --agent agy     # adds paddock's entry, "paddock", to ~/.gemini/config/hooks.json
paddock hooks uninstall --agent agy   # removes only that entry
```

A plain `paddock hooks install` includes agy when `agy` is on your `PATH`. paddock's entry holds three hooks (`SessionStart`, `PreInvocation`, `Stop`) that call `paddock hook --agent agy <event>` by paddock's full path, with a 10 s timeout, and answer `{}` (agy reads a hook's reply as JSON); re-run the install after moving paddock's binary. Your other named hooks in the file stay, and `uninstall` never creates the file. Hooks in `~/.gemini/antigravity-cli/settings.json` or a workspace's `.agents/hooks.json` are not touched. Restart running agy sessions afterwards.

States: idle when agy starts, running when it starts on a prompt (`PreInvocation`), done or idle once it is fully idle again (`Stop`). Waiting comes only from agy's status reports, read from the terminal's output while paddock runs: paddock sets `AGY_CLI_OSC7501=on` in its terminals, since agy reports only to a terminal that answers its check, and paddock's server (tmux, on the tmux backend) answers in paddock's place (that it does not support them). There agy also reports working, done and failed (failed counts as done); its idle reports never notify, so starting agy and quitting without a prompt says nothing. paddock records a report only while an agy session runs in that terminal, and not again while it says the same. Any program run inside an agy session's terminal can write the same report (`app=antigravity`) and so set that session's state. A terminal opened before you upgraded paddock lacks the variable: open a new one, or `export AGY_CLI_OSC7501=on` in it before running `agy`. Setting it `off` in your shell turns waiting off.

agy sends no hook when it quits: its session ends when its terminal closes, or when the terminal is back at a shell (paddock looks every 3 s, while an agy session runs in one of its terminals). Run `agy` from a shell: started through another program that stays in the foreground (`npx`, `node`, a wrapper script), it ends only when that program quits back to the shell or the terminal closes.

What agy sessions get: their row, dot, tab marker, state, branch and task rows (its first workspace stands in for the directory), `C-a a`, notifications, the EXTERNAL group and the AGENT tag `agy`. What stays Claude Code's alone: fork, subagent rows and the `claude agents --json` snapshot. An external agy session lives on its hooks alone: no status reports reach paddock from another terminal, so it is never waiting, and with no end event and no snapshot it leaves once nothing has been heard of it for `[dashboard] external_timeout` minutes (after it quit, or idle at its prompt), until its next hook.

Not checked yet, as agy needs a sign-in: that agy's hooks run with the terminal's `PADDOCK_PANE` and `PADDOCK_SOCKET` (or, on the tmux backend, `TMUX` and `TMUX_PANE`), by which paddock tells which terminal an event comes from. If they don't, every agy session shows under EXTERNAL, even one running in a paddock terminal, and is never waiting (its status reports find no session in that terminal). Please report it if you see that.

## Use

```sh
paddock            # open (a second launch offers to take over)
paddock version    # or -V, --version: print `paddock <version>`, for bug reports
paddock help       # or -h, --help: list the commands
```

The sidebar has two sections. SPACES lists your spaces, each with its directory and git branch on a dim second line, then, only where each fits after both uncut, the branch's status as on the dashboard (`+200 −10 ↑2↓1`: uncommitted lines added in green and removed in red, then commits ahead of and behind upstream; nothing on a detached HEAD or outside a repository) and, at the end, its pull request as `#123 ✓` (the number coloured by its state, then its checks; see Pull requests); short of room, the PR mark goes before the status, and on the selected space the status is not coloured: each space lives on paddock's server with its tabs and splits, so it outlives paddock until closed (on the tmux backend, a space is a session of the hidden tmux server, its tabs the session's windows and its splits their panes). AGENTS lists every main agent session in any space, in space and tab order (an agent never moves when its state changes), with its state (waiting, done, idle, running), the space and tab it runs in, and, at the end of that second line when it fits after the space and tab, its agent's kind (`claude`, `gemini` or `agy`). The space you are in and the agent in the focused terminal have a darker background (`[colors] current_space`), and `C-a s` puts the sidebar's selection on that agent (else on that space). Open a tab with `C-a c` and run `claude` (or `gemini`, or `agy`) in it; the dashboard (`C-a o`) shows the branches it and its subagents work on. The dashboard lists the sessions on top as an aligned table (NAME, STATE, ↑↓, CHANGES, BASE, FILES, AGE, PR, AGENT; AGENT is the session's agent kind, `claude`, `gemini` or `agy`; on a narrow screen whole columns drop, AGENT first, then PR after shortening to `#123 ✓`, NAME and STATE stay). Fork, subagent rows and `claude agents --json` are Claude Code's alone. A session and its branch and subagent rows form one block: only sessions are selected, the whole selected block is highlighted, and `D` dismisses the block. A session leaves the sidebar and the dashboard by itself once it ends: when its terminal (pane, tab or space) is closed, or when its agent quits in it (`/exit`, Ctrl-D; for agy, once the terminal is back at its shell); the selection moves to the session that takes its place. A new agent in that terminal is a new session and shows up as usual. Below the list, a live preview of the selected session sits in a rounded border (`I` types into it), and the keys are on the last line. A turn that ends while you look elsewhere is `done` until you look at its tab, then `idle`. You get a macOS notification when a session or one of its subagents starts waiting, or when a main agent finishes a turn (a subagent finishing never notifies).

External sessions: an agent running outside paddock's terminals (another terminal app, an IDE) shows too, dim, in an EXTERNAL group at the bottom of AGENTS and of the dashboard, with its state, its branch and task rows, but no screen: the preview says `external session: no screen`. paddock learns of it from the same hooks (the plugin's, or `hooks install`, both global to their agent) and, for Claude, from `claude agents --json`; its label is the session's name, else its directory's. It can't be jumped to, closed, forked or typed into: `Enter`, `x`, `f` and `I` on it only show a notice, `C-a a` skips it, and the finder leaves it out; `D` dismisses it. Waiting or done, it notifies and its dot shows like any other; with no tab to look at, a finished turn stays `done` until its row is selected in the dashboard, or in AGENTS while the sidebar has the keyboard. It leaves when it ends (`SessionEnd`, `/clear` included), or once nothing has been heard of it for `[dashboard] external_timeout` minutes (no hook event, not in `claude agents --json`), as when its agent was killed; a Gemini session waiting on you outlasts that timeout, since it has no snapshot and a wait sends no hooks, but leaves after 24 hours with nothing heard of it; an agy session sends no end event, so it leaves only by that timeout. `[dashboard] external = false` hides the group and its notifications. Its hook events go to `~/.local/state/paddock/events/external.jsonl`; once it passes 4 MiB, paddock trims it (when it opens, and every few seconds while it runs) to the last day's records, at most the newest 2 MiB of them.

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
| `H` | move split border left | `resize_left` |
| `J` | move split border down | `resize_down` |
| `K` | move split border up | `resize_up` |
| `L` | move split border right | `resize_right` |
| `z` | zoom the focused split | `zoom` |
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
| `[` | copy mode: move, select, copy | `copy_mode` |
| `/` | search the history | `search` |
| `?` | this help | `help` |
| `G` | show or hide lazygit in a float | `[floats] G` |

`C-a` pressed twice sends a literal `C-a`. `go_to_tab`'s first key goes to tab 1, its second to tab 2, and so on. `close` closes the pane; the last one closes its tab, the last tab its space, and it asks first if an agent is busy (`an agent is running: close? y/n`). Renaming, naming a new space (prefilled with `space-N`; `Enter` keeps it) and that question open a small box in the middle of the screen; `Esc` or a click outside it cancels. `{`/`}` can also be done by dragging the sidebar's border.

Zoom: `C-a z` makes the focused split fill its tab, and the tab bar shows a `Z` after the tab's name; `C-a z` again gives every split its place back, with the focus where it was. The other splits are hidden but keep running. Moving the focus (`C-a h/j/k/l`, a direct chord, or `C-a a` and the finder jumping to a hidden split), splitting, evening out (`C-a =`), or closing the zoomed split undoes the zoom first, as in tmux. With a float shown, `C-a z` hides the float, then zooms. The zoom lives on paddock's server with the tab's layout, so it survives detaching and comes back with a restored layout. (On the tmux backend it is tmux's `resize-pane -Z`, and a zoom made in `tmux -L paddock attach` shows in paddock too.) An agent in a hidden split that finishes a turn stays `done` until you see it. `zoom` has no direct chord by default.

Resizing: `C-a H`, `J`, `K` and `L` move the focused split's border left, down, up or right by 2 columns (or rows), the way tmux's `resize-pane` does: the border after the split, or the one before it for the last split of a row or column. After `C-a H`, the four keys work again without the prefix for 500 ms, each press starting the wait over, as tmux's `bind -r` does: `C-a H H H` moves the border 6 columns. Any other key ends that and is read as usual (so `H` typed later goes to the terminal); `[keys] repeat_ms` sets the wait, and `0` turns it off. You can also press on the line between two splits and drag it: the splits follow the pointer, and the size is kept on release. A press on that line never selects text. (On the tmux backend, dragging needs tmux 3.8 or later, whose layout paddock reads as JSON (`#{window_layout}`); with an older tmux a drag on that line does nothing, and the keys still work.) A split stops at 1 column or row, without a message. The sizes live on paddock's server (in tmux, on the tmux backend), so they survive detaching; `C-a =` evens them out again, and resizing a zoomed tab unzooms it first, as in tmux. The resize keys have no direct chord by default.

The finder (`C-a g`) opens a box framed like the prompts: a text field on top and, under it, every space (name, directory, branch), every tab (`space › tab`, directory, branch) and every agent (name, state, `space › tab`), each with its kind as a marker. Typing filters them fuzzily (case-insensitive unless you type an uppercase letter; contiguous and word-start matches rank first) against name, kind, directory and branch, best match first. `Up`/`Down` (or `C-p`/`C-n`) move, `Enter` shows the one selected (its space and tab, with its pane focused), `Esc` or a click outside cancels (a float shown stays shown); a click on a row opens it and the wheel moves the selection. While the field is empty, `b`, `w`, `i` and `d` show only agents that are waiting (blocked), running (working), idle or done, `a` all again, and the title says which; with text in the field they are ordinary characters. The finder's keys are fixed, except that `finder` in `[keys.commands]` names the key that opens it.

Floats (`[floats]`): `C-a G` shows lazygit in a box over the whole screen, sidebar included, framed like the prompts and titled with its command; `C-a G` again hides it and the program keeps running, so showing it again brings back the same program and state. When the program exits, the float closes, and the next `C-a G` starts it again. Each space has its own float per key, started in the focused terminal's directory; it lives on paddock's server with its space (so it survives detaching) but is never a tab, never in `C-a Tab`'s history, and paddock does not follow an agent in it. One float shows at a time. While it shows, every key, paste and mouse event inside it goes to it, except the prefix: `C-a` commands still work, and one that changes the page or gives the sidebar or dashboard the keyboard hides the float first. A click outside it does nothing. A float whose key you remove from `[floats]` is closed, with its program, the next time paddock opens. Floats are not saved in `layout.json`: after a server restart, their keys start them again. (On the tmux backend, floats are tmux windows, so `tmux -L paddock attach` lists them next to the tabs.)

Worktrees: a git worktree is a space. `C-a W` asks for a branch name (edited like the other prompts) and, in the focused terminal's repository, runs `git worktree add -b <branch> <path>` (`git worktree add <path> <branch>` if the branch exists), where `<path>` is `<[worktrees] directory>/<repo>/<branch>` with slashes in the branch written `-`; then it opens a new space named `<repo>/<branch>` with its first tab there. A name git refuses, a path that exists or any git error shows as a notice (git's reason first), and nothing is left half-made. `C-a O` lists the repository's worktrees that have no space yet: `j`/`k` move, `Enter` opens one, `Esc` cancels. In the sidebar, a space in a linked worktree is listed one cell in, under its repository's main-checkout space, or under a dim header with the repository's name when no space is in the main checkout; `W` and `o` there act on the selected space's repository, and `x` on a worktree space no longer just closes it: it deletes it. A worktree space is one paddock made, a linked worktree at exactly `<[worktrees] directory>/<repo>/<name>`; the main checkout and other worktrees (such as Claude's `.claude/worktrees/agent-*`) are never deleted, and `x` on their spaces only closes them, as before. Deleting asks `delete <branch> and its worktree (ignored files too)? y/n`; `n` (any key but `y`) cancels and nothing is deleted or closed. If the worktree has uncommitted changes, or commits not in its base (a branch's or a detached HEAD's), it asks again naming every risk; an unmerged branch is said to be kept (`… remove the worktree anyway? (the branch is kept) y/n`). Then it closes the space and any other space in that worktree (asking if an agent is busy in any, as close does), runs `git worktree remove` (`--force` only if you confirmed uncommitted changes; changes or commits made after the question leave everything in place, with a notice) and `git branch -d` (never `-D`: a branch git finds unmerged is kept, and a notice says so). `delete_worktree` (unbound by default) does the same for the current space. Every git read paddock makes gives up after 5 s, adding and removing after 60 s.

Space roots: every space has a root directory, kept with the space on paddock's server and in `layout.json` (on the tmux backend, in its tmux session as the option `@paddock-root`). A space gets the directory it was made in: the focused terminal's for `C-a C`, the worktree for `C-a W`, `C-a O` and `paddock worktree new|open`. `C-a R` makes the focused terminal's directory the current space's root, and says so in the tab bar. With `[spaces] new_tab_dir = "root"`, new tabs and splits start in the root wherever the focused terminal has gone; with the default `"current"` they start in the focused terminal's directory, as before. A space made by an older paddock has no root until `C-a R`; until then its first tab's directory serves, and a root that was deleted falls back to the focused terminal's directory. The sidebar still shows each space's first tab's directory; the finder's space rows show the root as it was made (links kept) and the branch checked out there, if it is in a git checkout.

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
| `p` | open the session's pull request in the browser | `pr` |
| `I` | type into the previewed terminal (Esc leaves) | `interact` |
| `/` | filter the list (Enter keeps, Esc clears) | `filter` |
| `Esc` | close the dashboard | `back` |

While typing into the preview, every key goes to the terminal except `Esc`, which only leaves and is never passed on; this is fixed, so a remap can never trap you there. `/` opens a one-line filter under the dashboard's title: the list keeps only the blocks whose session name, branch or state fuzzy-match what you type; `Enter` keeps the filter and returns the keys to the list, `Esc` clears it and leaves the filter line. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and its pull request in the PR column (below), else `merged` once the branch is in its base. Prompt editing (`Enter`, `Esc`, `Left`/`Right`, `Home`/`End`, `Backspace`/`Delete`, `C-u` (clear), `C-w` (delete word)), the dashboard filter's keys (the same editing; `Enter` keeps, `Esc` clears) and the `y` that confirms a close or a worktree delete are fixed too, as are `j`/`k`/`Enter`/`Esc` in the worktree list.

Pull requests: a task row whose branch has a GitHub pull request shows it in the PR column, as `#123 open ✓ approved`: the number (green open, dim draft, purple merged, red closed), its state (open, draft, merged, closed), its CI checks (`✓` passing, `✗` failing, `•` pending, nothing without checks) and its review (approved, changes requested, review required, nothing without a decision). paddock asks the GitHub CLI once per repository (`gh pr list --state all --limit 20`, in the repository, with a 5 s limit), matching pull requests to branches by name, the newest one when a branch has several; it asks when the dashboard opens (unless it asked less than 10 s ago) and every `[github] refresh_secs` seconds (60) while the dashboard is open or the sidebar shows, one repository after another, never while the sidebar is hidden and the dashboard closed, and `refresh_secs = 0` turns it off. The sidebar marks each space's branch's pull request after its branch, only where it fits after the directory and branch uncut: `#123 ✓`, the number in its state's colour and the checks mark (no review there). A branch with no pull request among the repository's newest 20 shows `merged` if git finds it in its base, else nothing. Without `gh`, logged out, with no GitHub remote, or when `gh` fails or takes longer than 5 s, the column shows what git knows and nothing else; the reason goes to one line of `~/.local/state/paddock/gh.log`, not again until `gh` answers. `p` opens the selected session's pull request (its main agent's branch's, else a subagent branch's) with `gh pr view --web` (a press while one is opening does nothing); without one, a notice says so. paddock runs gh without `GH_REPO`, so each repository is asked about itself. paddock never changes anything on GitHub, and sends no notification when a pull request or its checks change.

**Breaking (PR status):** the dashboard's MERGED column is now the PR column. A merged branch with no pull request reads `merged` there instead of `✓`; a `[keys.dashboard]` that binds `p` to another action now clashes with `pr`, and paddock refuses to start until one is unbound (`pr = ""`). `paddock worktree list`'s text gains a PR column (`#123 open` or `-`) between `merged`/`unmerged` and the agents, so a script that splits its columns by position sees the agents one column later (`--json` only gains `pr`).

Mouse: click to focus a split, switch tabs (`+` after the tabs opens one, `<`/`>` scroll a full bar), or open a space or agent; the wheel over the tab bar switches tabs; wheel scrolls history; drag selects and copies (hold Shift over programs that use the mouse); dragging the line between two splits resizes them, and dragging the sidebar's border resizes the sidebar.

#### Copy mode

`C-a [` puts the terminal you type into (the float shown, else the focused split) in copy mode: its view stops following the program, a cursor appears, and the keys of `[keys.copy]` move it over the whole history and the screen, select, copy and search. The program keeps running; what it writes meanwhile shows when you leave. The bottom line reads ` COPY  line N/M ` (with ` match i/n ` after a search) and as many of the keys as fit; it moves to the top while the cursor is on the bottom row. `C-a /` opens copy mode with the search prompt already open.

| Key | Action | Name |
|---|---|---|
| `h` `Left` | cursor left | `left` |
| `j` `Down` | cursor down | `down` |
| `k` `Up` | cursor up | `up` |
| `l` `Right` | cursor right | `right` |
| `w` | start of the next word | `next_word` |
| `b` | start of the word | `previous_word` |
| `e` | end of the word | `word_end` |
| `0` | start of the line | `line_start` |
| `$` | end of the line | `line_end` |
| `g` | top of the history | `top` |
| `G` | bottom of history | `bottom` |
| `C-u` | half a page up | `half_page_up` |
| `C-d` | half a page down | `half_page_down` |
| `C-b` | a page up | `page_up` |
| `C-f` | a page down | `page_down` |
| `v` | start or stop a selection | `select` |
| `V` | start or stop a line selection | `select_lines` |
| `y` `Enter` | copy the selection and leave | `copy` |
| `/` | search back towards the top | `search_up` |
| `?` | search on towards the bottom | `search_down` |
| `n` | next match, the search's way | `next_match` |
| `N` | previous match | `previous_match` |
| `Esc` `q` | leave without copying | `leave` |

`y` copies the selection to the clipboard the way dragging does (a line selection without its last newline) and leaves; with nothing selected it just leaves. `v` or `V` again stops selecting; the other switches kind. A search is plain text, case-insensitive unless it has an uppercase letter (as in the finder), over the whole history and screen, rows wrapped into one line counting as one; its prompt is edited like the others (`Enter` searches, `Esc` closes it), and paste goes into it. `Enter` jumps to the nearest match back up from the cursor for `/`, down for `?`; `n` goes on the same way, `N` the other, both wrapping around. Every match on screen is highlighted; with none, a notice says so and the cursor stays. The prefix works in copy mode, except in the search prompt, which takes every key as the other prompts do; a command leaves copy mode first. Direct chords do not fire. The wheel over the terminal scrolls the copy view; a click or drag leaves copy mode and acts as usual. Copy mode also ends when its terminal closes or leaves the screen, when the terminal is resized, and if more than 8 MiB of output piles up meanwhile (with a notice). A default copy key that is your prefix (`C-b` under `prefix = "C-b"`) is left out.

#### Direct

Some commands also fire on one chord, no prefix (`[keys.direct]`), whenever a terminal has the keyboard: not in the sidebar, the dashboard, a prompt, the finder, copy mode or a float (a float gets every key but the prefix). Any action from the prefix table can be bound here; by default:

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
repeat_ms = 500           # resize keys work again without the prefix this long; 0 turns it off

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

[keys.copy]               # in copy mode (C-a [)
leave = ["Esc", "q", "C-c"]

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

[dashboard]
external = true           # list agent sessions running outside paddock (the External group); false hides them
external_timeout = 10     # minutes an external session stays with no hook event and no longer in `claude agents --json`

[github]
refresh_secs = 60         # how often the dashboard and the sidebar ask `gh` for each repository's pull requests; 0 turns PR status off (in `worktree list` too)

[backend]
kind = "native"           # what hosts the terminals: "native" (the default) or "tmux" (legacy, until v0.2)
```

`[backend] kind` is `"native"` (the default: paddock's own server, `paddock server`, started in the background and listening on `~/.local/state/paddock/server.sock`, which saves its layout to `layout.json`) or `"tmux"`, the hidden `tmux -L paddock` server earlier releases used, kept until v0.2; anything else is refused at start. Switching kinds does not move running terminals: each backend keeps its own.

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

paddock refuses to start, naming the file and the setting, on an unknown action or section, a key it cannot read, a key listed twice for one action, a key bound to two actions in the same section (unbind one with `""`, even a default), or the prefix key inside a `[keys.…]` section. In `[floats]` the keys use the same syntax and are refused the same way when they cannot be read, are listed twice, are the prefix, or are bound in `[keys.commands]` (the default `G` too: write `G = ""` to free it); `width` and `height` are whole percents from 1 to 100. `g` now opens the finder, so a float you had on `[floats] g` must move to another key (lazygit's default moved to `G`). Likewise `H`, `J`, `K` and `L` now resize splits: a float or another command on one of them must move, or free the key with `resize_left = ""` (and so on) in `[keys.commands]`.

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

This repository is a Claude Code plugin marketplace. Its `paddock` plugin carries paddock's hooks (see [Connect Claude Code](#connect-claude-code)), the skill, and two slash commands, `/paddock:worktree-new <branch>` and `/paddock:worktree-list`. In Claude Code:

```text
/plugin marketplace add MiohitoKiri5474/paddock
/plugin install paddock@paddock
```

### Gemini CLI

The repository is also a Gemini CLI extension (`gemini-extension.json` at its root; it needs Gemini CLI 0.26 or newer, where Agent Skills are on by default, and was checked with `gemini extensions validate` from Gemini CLI 0.65). It carries the skill and two commands, `/paddock:worktree-new <branch>` and `/paddock:worktree-list`, and no hooks: those come from `paddock hooks install --agent gemini` (see [Connect Gemini CLI](#connect-gemini-cli)). From your shell:

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

## Files

- State: `~/.local/state/paddock/` (the server's `server.sock`, `server.lock`, `server.log` and `layout.json`, sidebar width, event logs, dismissed rows, `hook-errors.log`, `gh.log`)
- The tmux backend's hidden server: `tmux -L paddock ls`

## Uninstall

```sh
paddock hooks uninstall --agent claude --agent gemini --agent agy   # each agent you connected
# First close every space in paddock (sidebar `x`): the server ends with the last one.
tmux -L paddock kill-server         # the tmux backend's server, if one still runs
rm -rf ~/.local/state/paddock ~/.config/paddock
cargo uninstall paddock             # or brew uninstall paddock, or delete the binary you moved
```

## Develop

```sh
cargo clippy --all-targets -- -D warnings
cargo test                          # unit + end-to-end tests on the tmux backend (real tmux on a temp socket)
scripts/test-native.sh              # the end-to-end tests on the native backend (the default)
claude plugin validate --strict .   # the plugin marketplace, in full (cargo test checks the basics)
claude plugin validate --strict plugins/paddock
gemini extensions validate .        # the Gemini extension (agy installs it too)
```

A release bumps the version in `Cargo.toml` (and `Cargo.lock`), `plugins/paddock/.claude-plugin/plugin.json` and `gemini-extension.json` together; `cargo test` fails while either manifest differs from `Cargo.toml`.

CI runs the unit tests, then the end-to-end tests on each backend (native, and tmux with its tmux-only tests), on macOS, and uploads a release binary when the unit tests pass. Pushing a `v*` tag that matches the version in `Cargo.toml` builds the Apple Silicon tarball and opens a draft pre-release.

## License

MIT, see [LICENSE](LICENSE).

## Credits

paddock's look and much of its behaviour (spaces in a sidebar with an agents section, the filled-block tab bar) are inspired by [herdr](https://github.com/herdrdev/herdr). paddock is a separate implementation and contains none of herdr's code.
