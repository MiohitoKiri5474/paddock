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

Prefix `C-a`, then:

| Key | Action |
|---|---|
| `c` | new tab in the current space |
| `C` | new space |
| `n` / `p` | next / previous tab |
| `Tab` | last page: back to the space and tab shown before (again: return) |
| `1`–`9` | go to tab |
| `,` | rename tab |
| `$` | rename space |
| `<` / `>` | move the current tab left / right |
| `\|` / `-` | split side by side / top and bottom |
| `h` `j` `k` `l` / arrows | move focus |
| `=` | even out the focused split and its neighbours |
| `x` | close pane; the last one closes its tab, the last tab its space (asks if Claude is busy) |
| `f` | fork Claude into a new tab of the same space |
| `s` | focus sidebar (on the first agent) |
| `w` | focus sidebar on the current space |
| `b` | show or hide sidebar |
| `{` / `}` | sidebar narrower / wider (also: drag its border) |
| `a` | next session that needs you |
| `o` | dashboard: every session and task with details and a live preview |
| `d` | detach: paddock quits, terminals keep running |
| `?` | key help |
| `C-a` | literal `C-a` |

In the sidebar: `j`/`k` move over spaces and agents; `Enter` on a space shows its tabs, on an agent jumps to its pane (space, tab and split); `x` closes the space, or the agent's pane; `f` forks the agent (on a space, its agent that needs you most); `Esc` goes back.

In the dashboard: `j`/`k` move, `Enter` jumps, `D` dismisses, `x` closes, `f` forks, `I` types into the previewed terminal (`Esc` leaves and is never passed on), `Esc` closes it. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and `merged` once the branch is in its base.

Mouse: click to focus a split, switch tabs (`+` after the tabs opens one, `<`/`>` scroll a full bar), or open a space or agent; the wheel over the tab bar switches tabs; wheel scrolls history; drag selects and copies (hold Shift over programs that use the mouse).

## Configure

`~/.config/paddock/config.toml`:

```toml
[keys]
prefix = "C-a"

[colors]
current_space = 236       # the current space's background: 256-colour index or "#rrggbb" (also read as current_item)
tab_bar = 235             # the tab bar's row
tab = 237                 # a tab that is not current
tab_current = 4           # the current tab (default: the terminal's blue)

[sidebar]
width = 32                # columns until you resize it; the last width is remembered
```

## Files

- State: `~/.local/state/paddock/` (current space, event logs, dismissed rows, `hook-errors.log`); tabs and splits live in tmux
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
