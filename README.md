# paddock

A terminal workspace for one person on one Mac. It keeps every terminal alive on a hidden tmux server, draws them itself (tabs, splits, mouse, scrollback), and tracks Claude Code sessions and the branches they and their subagents work on, so you can see at a glance which ones need you.

Design: [docs/design.md](docs/design.md). Glossary: [CONTEXT.md](CONTEXT.md). Decisions: [docs/adr/](docs/adr/).

## Requirements

- macOS
- tmux (tested with 3.8)
- Rust 1.85+
- Claude Code (for session tracking)

## Install

```sh
cargo install --path . --locked     # installs ~/.cargo/bin/paddock
cp ~/.claude/settings.json ~/.claude/settings.json.bak
paddock hooks install               # adds paddock's hooks to ~/.claude/settings.json
```

`hooks install` keeps other hooks (for example Supacode's), removes the old tmux-claude-session-manager hooks, and records the binary's full path, so install from a stable location. Re-run it after upgrading paddock.

## Use

```sh
paddock          # open (a second launch offers to take over)
```

The sidebar has two sections. SPACES lists your spaces, each with its directory and git branch on a dim second line: each space is a tmux session on paddock's hidden server, its tabs are the session's windows and its splits their panes, so `tmux -L paddock attach` shows the same thing. AGENTS lists every main Claude session in any space, with its state (waiting, done, idle, running, stopped) and the space and tab it runs in. Open a tab with `C-a c` and run `claude` in it; the dashboard (`C-a o`) shows the branches it and its subagents work on. A turn that ends while you look elsewhere is `done` until you look at its tab, then `idle`. You get a macOS notification when a session starts waiting or a main agent finishes a turn.

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
| `s` | focus sidebar on agents | `focus_sidebar` |
| `w` | focus sidebar on the space | `focus_spaces` |
| `b` | show or hide sidebar | `toggle_sidebar` |
| `{` | sidebar narrower | `sidebar_narrower` |
| `}` | sidebar wider | `sidebar_wider` |
| `a` | next session needing you | `next_attention` |
| `o` | dashboard | `dashboard` |
| `d` | detach (terminals go on) | `detach` |
| `?` | this help | `help` |

`C-a` pressed twice sends a literal `C-a`. `go_to_tab`'s first key goes to tab 1, its second to tab 2, and so on. `close` closes the pane; the last one closes its tab, the last tab its space, and it asks first if Claude is busy. Renaming, naming a new space (prefilled with `space-N`; `Enter` keeps it) and that question open a small box in the middle of the screen; `Esc` or a click outside it cancels. `{`/`}` can also be done by dragging the sidebar's border.

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
| `j` `Down` | next row | `down` |
| `k` `Up` | previous row | `up` |
| `Enter` | jump to the row's terminal | `open` |
| `D` | dismiss the row | `dismiss` |
| `x` | close the row's terminal | `close` |
| `f` | fork the row's Claude session | `fork` |
| `I` | type into the previewed terminal (Esc leaves) | `interact` |
| `Esc` | close the dashboard | `back` |

While typing into the preview, every key goes to the terminal except `Esc`, which only leaves and is never passed on; this is fixed, so a remap can never trap you there. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and `merged` once the branch is in its base. Prompt editing (`Enter`, `Esc`, `Left`/`Right`, `Home`/`End`, `Backspace`/`Delete`, `C-u` (clear), `C-w` (delete word)) and the `y` that confirms a close are fixed too.

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
current_space = 236       # the current space's background: 256-colour index or "#rrggbb" (also read as current_item)
tab_bar = 235             # the tab bar's row
tab = 237                 # a tab that is not current
tab_current = 4           # the current tab (default: the terminal's blue)

[sidebar]
width = 32                # columns until you resize it
```

Once you resize the sidebar (`C-a {`, `C-a }` or dragging its border), paddock saves that width in `~/.local/state/paddock/layout.toml` (`sidebar = N`), and it overrides `[sidebar] width` from then on. Delete that line (with paddock closed) to go back to the configured width.

`paddock keys` prints every action with its default keys, ready to copy. Actions you leave out keep their defaults. The prefix and the bindings share one key syntax:

- a character (`c`, `|`, `$`, `{`) or a name (`Tab`, `Enter`, `Esc`, `Space`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Left`, `Down`, `Up`, `Right`, `F1`-`F12`); names in any case
- optionally after `C-` (Ctrl), `S-` (Shift) or `M-` (Alt), in any case (`c-n` is `C-n`)
- `C-` with a letter is the same key in either case (`C-A` is `C-a`: terminals send them alike); `C-S-` with a character is refused for the same reason
- `S-` goes with a letter (`S-a` is `A`) or a named key (`S-Tab`); for other characters write the shifted one itself (`!`, not `S-1`)
- `M-` works, with a warning at start and again on leaving, but not every terminal sends it, so paddock's own keys avoid it
- the prefix must be `C-` with a letter, `Space`, or one of `\ ] ^ _`

After the prefix, Shift does not matter for a key bound without it (Shift+arrows move focus too); in the sidebar and the dashboard, neither do Ctrl and Shift. A key bound with the modifier wins.

paddock refuses to start, naming the file and the setting, on an unknown action or section, a key it cannot read, a key listed twice for one action, a key bound to two actions in the same section (unbind one with `""`, even a default), or the prefix key inside a `[keys.…]` section.

## Files

- State: `~/.local/state/paddock/` (current space and sidebar width, event logs, dismissed rows, `hook-errors.log`); tabs and splits live in tmux
- Hidden server: `tmux -L paddock ls`

## Uninstall

```sh
paddock hooks uninstall
tmux -L paddock kill-server         # ends every paddock terminal
rm -rf ~/.local/state/paddock ~/.config/paddock
cargo uninstall paddock
```

## Develop

```sh
cargo clippy --all-targets -- -D warnings
cargo test                          # unit + end-to-end tests (real tmux on a temp socket)
```

CI runs the unit tests on macOS and uploads a release binary when they pass.
