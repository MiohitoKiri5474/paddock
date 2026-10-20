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

Open a terminal with `C-a c` and run `claude` in it; the session appears in the sidebar with its state (running, waiting, idle, stopped) and the branches it works on. You get a macOS notification when a session starts waiting or a main agent finishes a turn.

### Keys

Prefix `C-a`, then:

| Key | Action |
|---|---|
| `c` | new shell tab |
| `n` / `p` | next / previous tab |
| `1`–`9` | go to tab |
| `,` | rename tab |
| `\|` / `-` | split side by side / top and bottom |
| `h` `j` `k` `l` / arrows | move focus |
| `=` | equalise split sizes |
| `x` | close terminal (asks if Claude is busy) |
| `f` | fork Claude into a new tab |
| `s` | focus sidebar |
| `b` | show or hide sidebar |
| `a` | next session that needs you |
| `o` | dashboard: every session and task with details and a live preview |
| `d` | detach: paddock quits, terminals keep running |
| `?` | key help |
| `C-a` | literal `C-a` |

In the sidebar: `j`/`k` move, `Enter` jumps, `D` dismisses a row, `x` closes, `f` forks, `Esc` goes back.

In the dashboard: `j`/`k` move, `Enter` jumps, `D` dismisses, `x` closes, `f` forks, `I` types into the previewed terminal (`Esc` leaves and is never passed on), `Esc` closes it. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and `merged` once the branch is in its base.

Mouse: click to focus a split, switch tabs or jump to a row; wheel scrolls history; drag selects and copies (hold Shift over programs that use the mouse).

## Configure

`~/.config/paddock/config.toml`:

```toml
[keys]
prefix = "C-a"
```

## Files

- State: `~/.local/state/paddock/` (layout, event logs, dismissed rows, `hook-errors.log`)
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
