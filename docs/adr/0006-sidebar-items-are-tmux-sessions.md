# Sidebar items are tmux sessions, tabs their windows, splits their panes

> Superseded by [ADR 0007](0007-native-backend-with-a-server.md): paddock hosts its own terminals in a server. This stays true for the `tmux` backend until it is removed (planned for v0.2).

Supersedes the "one tmux session per terminal" part of [ADR 0001](0001-tmux-as-hidden-backend.md). The hidden server, its own config and the clean environment stay.

After a day with v1, the sidebar lists spaces (herdr-like) instead of Claude session and task rows; a second section lists every main agent across spaces with its space and tab. Each **sidebar item is a tmux session** on the hidden server. Its **tabs are that session's windows**, and its **splits are panes** in a window. `tmux -L paddock attach` therefore shows the same structure as paddock. Layout, split sizes, the current tab of each item and the focused pane of each tab all live in tmux. paddock keeps only the current item in `layout.toml`.

Amended 2026-10-21 for floats (see [the design](../design.md)): a session may also hold **float windows**, marked with the window option `@paddock-float` (the float's key). They are not tabs: paddock keeps them out of the tab bar, the sidebar and Claude tracking. They are still windows of the session, so `tmux -L paddock attach` shows them too, as ordinary windows next to the tabs.

Hooks still name terminals by `$TMUX_PANE`. Pane ids are unique on the server, so Claude sessions stay keyed by pane. Task rows leave the sidebar for the dashboard; the task model is unchanged.

## Control mode facts (tmux 3.8, checked 2026-10-20)

- **One control client per item is enough.** A client attached to a session gets `%output` for every pane of every window in it, current window or not.
- **`refresh-client -C WxH` sizes every window of the session**, the non-current ones included (`window-size latest`). Other sessions keep their own size, so each item's client is resized on its own.
- **`refresh-client -C @window:WxH` sizes one window apart from the session** (checked 2026-10-21): that window keeps its own size when the client is later resized with `refresh-client -C WxH`, while the other windows follow the client. Floats use this to fit their box.
- **`set-option -w` after `new-window -d` in the same run marks the session's current window, not the new one.** A float is therefore made and marked in one tmux run, with the new window targeted by a name of its own; tmux runs the whole line before any other client's command, so no structure read sees it unmarked.
- **An argument ending in `;` separates commands; one ending in `\;` keeps a `;`.** Every user-supplied argument (names, directories, a float's key or command) gets a `\` before a final `;`.
- **Names and start directories are format-expanded** (checked 2026-10-22): `new-session -s`, `rename-session`, `rename-window` and `-c` read `#S`, `#{…}`, `#(…)` as formats (`new-session -s 'v#S'` gives `v`; `rename-session 'r#S'` gives `r` and the old name), and `##` as a `#`. paddock doubles every `#` in them before the final-`;` escape.
- **Structure changes are notified:** `%window-add`, `%window-close`, `%layout-change` (split, pane exit, resize), `%window-renamed` (also from automatic-rename), `%session-renamed`, `%session-window-changed`, `%window-pane-changed`. Windows of other sessions arrive as `%unlinked-*` and are ignored.
- **The last pane closes its window, the last window its session**, and the session's control client then gets `%exit`.
- **`#{window_layout}` is JSON in 3.8**, while `%layout-change` still carries the old layout string. paddock parses neither: on any structure notification it re-reads `list-panes -s` (window id, name, `#{automatic-rename}`, pane id and position and size, active flags).
- **automatic-rename fires with `status off`.** `automatic-rename-format` is set so that a shell shows its directory name and anything else its command. `rename-window` turns automatic-rename off for that window, which tells paddock the user named it.
- **`select-layout -E` spreads the target pane's neighbours.** Running it on every pane of a nested layout made one window wider than its client (tmux bug), so `=` spreads the focused pane's split only.

## Seeding several panes on one client

On attach, every pane of the item is seeded in one control line (`display` plus three `capture-pane` per pane, see ADR 0005). tmux runs the line without reading pane output in between, so output before the last reply is in the captures and output after it is new. Panes that appear while attached (new window, split, or made through `tmux attach`) start empty, so they need no seed: their emulator is created when paddock first sees them, sized from tmux.

## Considered options

- **Keep one tmux session per terminal and group them in paddock**: no change to the control-mode client, but `tmux attach` would show a flat list, and paddock would keep owning a layout tree tmux cannot see.
- **One control client per pane**: a client follows its session, not a pane, so this would mean one session per pane again.

## Consequences

- Existing servers from v1 (one session per terminal) open as one item per terminal.
- paddock draws pane borders in the gaps tmux leaves between panes.
- New tmux options must also be applied to a server that is already running, since tmux reads its config only at start.
- A pane moved in from another session by hand (`join-pane`, `move-window`) starts with an empty screen in paddock until it redraws.
