# paddock

A terminal workspace for one person on one Mac. It keeps every terminal alive on a hidden tmux server, draws them itself (tabs, splits, mouse, scrollback), and tracks Claude Code sessions and the branches they and their subagents work on, so you can see at a glance which ones need you.

Design: [docs/design.md](docs/design.md). Glossary: [CONTEXT.md](CONTEXT.md). Decisions: [docs/adr/](docs/adr/).

## Requirements

- macOS (the release binary is for Apple Silicon only)
- tmux (tested with 3.8): `brew install tmux` (Homebrew installs it with paddock)
- Claude Code (for session tracking)
- Rust 1.85+, only to build from source

## Install

Pick one of these, then install the hooks (below).

**Homebrew** (recommended; installs tmux too):

```sh
brew install MiohitoKiri5474/paddock/paddock
```

If an earlier `brew install` failed while tapping, run `brew untap miohitokiri5474/paddock` first. Upgrade with `brew upgrade paddock`.

**Release tarball** (Apple Silicon). Download `paddock-v0.0.2-aarch64-apple-darwin.tar.gz` and its `.sha256` from [Releases](https://github.com/MiohitoKiri5474/paddock/releases), then:

```sh
shasum -a 256 -c paddock-v0.0.2-aarch64-apple-darwin.tar.gz.sha256
tar xzf paddock-v0.0.2-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine paddock   # unsigned binary: lets Gatekeeper run it ("No such xattr" is fine)
sudo mkdir -p /usr/local/bin             # missing on a fresh Apple Silicon Mac
sudo mv paddock /usr/local/bin/         # or anywhere on your PATH
```

**Cargo**, from a tag:

```sh
cargo install --git https://github.com/MiohitoKiri5474/paddock --tag v0.0.2 --locked
```

If it fails with `failed to authenticate when downloading repository: git@github.com:…` (your git config rewrites GitHub URLs to SSH), let cargo use the `git` command: prefix the command with `CARGO_NET_GIT_FETCH_WITH_CLI=true`, or set `git-fetch-with-cli = true` under `[net]` in `~/.cargo/config.toml`.

**From source**, in a clone:

```sh
cargo install --path . --locked     # installs ~/.cargo/bin/paddock
```

Then install the Claude Code hooks:

```sh
cp ~/.claude/settings.json ~/.claude/settings.json.bak
paddock hooks install               # adds paddock's hooks to ~/.claude/settings.json
```

`hooks install` keeps other hooks (for example Supacode's), removes the old tmux-claude-session-manager hooks, and records the binary's full path, so install from a stable location. Re-run it after upgrading paddock or moving its binary.

### Switch install method

The hooks point at the old binary by its full path, so remove them with it before it goes, then install them again with the new one. For example, from Cargo to Homebrew:

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
paddock          # open (a second launch offers to take over)
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

In the sidebar (`[keys.sidebar]`; prefix commands work too):

| Key | Action | Name |
|---|---|---|
| `j` `Down` | next space or agent | `down` |
| `k` `Up` | previous space or agent | `up` |
| `Enter` | show the space's tabs, or jump to the agent's pane | `open` |
| `x` | close the space, or the agent's pane | `close` |
| `f` | fork the agent (on a space, its agent that needs you most) | `fork` |
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

While typing into the preview, every key goes to the terminal except `Esc`, which only leaves and is never passed on; this is fixed, so a remap can never trap you there. `/` opens a one-line filter under the dashboard's title: the list keeps only the blocks whose session name, branch or state fuzzy-match what you type; `Enter` keeps the filter and returns the keys to the list, `Esc` clears it and leaves the filter line. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and `merged` once the branch is in its base. Prompt editing (`Enter`, `Esc`, `Left`/`Right`, `Home`/`End`, `Backspace`/`Delete`, `C-u` (clear), `C-w` (delete word)), the dashboard filter's keys (the same editing; `Enter` keeps, `Esc` clears) and the `y` that confirms a close are fixed too.

Mouse: click to focus a split, switch tabs (`+` after the tabs opens one, `<`/`>` scroll a full bar), or open a space or agent; the wheel over the tab bar switches tabs; wheel scrolls history; drag selects and copies (hold Shift over programs that use the mouse).

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
```

Both notifications are on by default; `false` turns one off. The session's state, the sidebar and tab markers, `C-a a` and the dashboard work the same either way.

Once you resize the sidebar (`C-a {`, `C-a }` or dragging its border), paddock saves that width in `~/.local/state/paddock/layout.toml` (`sidebar = N`), and it overrides `[sidebar] width` from then on. Delete that line (with paddock closed) to go back to the configured width.

`paddock keys` prints every action with its default keys, and the default floats, ready to copy. Actions you leave out keep their defaults. The prefix and the bindings share one key syntax:

- a character (`c`, `|`, `$`, `{`) or a name (`Tab`, `Enter`, `Esc`, `Space`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Left`, `Down`, `Up`, `Right`, `F1`-`F12`); names in any case
- optionally after `C-` (Ctrl), `S-` (Shift) or `M-` (Alt), in any case (`c-n` is `C-n`)
- `C-` with a letter is the same key in either case (`C-A` is `C-a`: terminals send them alike); `C-S-` with a character is refused for the same reason
- `S-` goes with a letter (`S-a` is `A`) or a named key (`S-Tab`); for other characters write the shifted one itself (`!`, not `S-1`)
- `M-` works, with a warning at start and again on leaving, but not every terminal sends it, so paddock's own keys avoid it
- the prefix must be `C-` with a letter, `Space`, or one of `\ ] ^ _`

After the prefix, Shift does not matter for a key bound without it (Shift+arrows move focus too); in the sidebar and the dashboard, neither do Ctrl and Shift. A key bound with the modifier wins.

paddock refuses to start, naming the file and the setting, on an unknown action or section, a key it cannot read, a key listed twice for one action, a key bound to two actions in the same section (unbind one with `""`, even a default), or the prefix key inside a `[keys.…]` section. In `[floats]` the keys use the same syntax and are refused the same way when they cannot be read, are listed twice, are the prefix, or are bound in `[keys.commands]` (the default `G` too: write `G = ""` to free it); `width` and `height` are whole percents from 1 to 100. `g` now opens the finder, so a float you had on `[floats] g` must move to another key (lazygit's default moved to `G`).

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
```

CI runs the unit tests on macOS and uploads a release binary when they pass. Pushing a `v*` tag that matches the version in `Cargo.toml` builds the Apple Silicon tarball and opens a draft pre-release.

## License

MIT, see [LICENSE](LICENSE).

## Credits

paddock's look and much of its behaviour (spaces in a sidebar with an agents section, the filled-block tab bar) are inspired by [herdr](https://github.com/herdrdev/herdr). paddock is a separate implementation and contains none of herdr's code.
