# Configure

Every `config.toml` section and option. Back to the [README](../README.md).

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

## Defaults

Every option, and its value when you leave it out:

| Section | Option | Default |
|---|---|---|
| `[keys]` | `prefix` | `"C-a"` |
| `[keys]` | `repeat_ms` | `500` |
| `[keys.commands]`, `[keys.direct]`, `[keys.sidebar]`, `[keys.dashboard]`, `[keys.copy]` | one per action | the keys in [keys.md](keys.md) (`paddock keys` prints them) |
| `[colors]` | `current_space` (also read as `current_item`) | `236` |
| `[colors]` | `tab_bar` | `235` |
| `[colors]` | `tab` | `237` |
| `[colors]` | `tab_current` | `4` (the terminal's blue) |
| `[sidebar]` | `width` | `32` |
| `[floats]` | `width` | `80` |
| `[floats]` | `height` | `80` |
| `[floats]` | one per key | `G = "lazygit"` |
| `[notify]` | `waiting` | `true` |
| `[notify]` | `idle` | `true` |
| `[worktrees]` | `directory` | `"~/.paddock/worktrees"` |
| `[spaces]` | `new_tab_dir` | `"current"` |
| `[dashboard]` | `external` | `true` |
| `[dashboard]` | `external_timeout` | `10` |
| `[github]` | `refresh_secs` | `60` |
| `[backend]` | `kind` | `"native"` |

## Notes

`[backend] kind` is `"native"` (the default: paddock's own server, `paddock server`, started in the background and listening on `~/.local/state/paddock/server.sock`, which saves its layout to `layout.json`) or `"tmux"`, the hidden `tmux -L paddock` server earlier releases used, kept until v0.2; anything else is refused at start. Switching kinds does not move running terminals: each backend keeps its own.

`[worktrees] directory` must be absolute once `~` is expanded, and not your home directory itself; paddock refuses to start otherwise.

`[spaces] new_tab_dir` is `"current"` (the default) or `"root"`, and applies to new splits too; anything else is refused at start. A space's root is described under [Use](use.md#space-roots).

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
