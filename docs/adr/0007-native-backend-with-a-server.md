# paddock hosts its own terminals in a server, replacing tmux

Supersedes [ADR 0001](0001-tmux-as-hidden-backend.md), [ADR 0003](0003-no-daemon-single-instance.md) and [ADR 0006](0006-sidebar-items-are-tmux-sessions.md), and the control-mode half of [ADR 0002](0002-own-renderer-via-control-mode.md). paddock still draws every terminal itself with alacritty_terminal ([ADR 0005](0005-alacritty-terminal-emulator.md)).

tmux gave paddock terminals that outlive the app. It also brought format-expanded names, a layout string paddock cannot parse, key modes lost on reattach, a 3.8 dependency for border dragging, and about 160 tmux queries in the tests. paddock now hosts its terminals itself. `paddock server` is a background process that owns every PTY, the layout (spaces, tabs, splits, floats, zoom), and an emulator per terminal. The app is a client of that server over a Unix socket in `~/.local/state/paddock/`.

- **Lifetime.** The first `paddock` starts the server, and the server exits when no terminal is left. Closing the app detaches as before.
- **One client at a time.** A second launch still offers to take over from the first.
- **Attach.** The server sends each terminal's screen, scrollback, cursor and key modes as a snapshot, then streams its raw output. The app keeps its own emulator, so rendering, selection, copy mode and search do not change. The snapshot replaces `capture-pane` seeding and the key-mode workaround in ADR 0005.
- **Clean environment.** Shells start with the same allowlisted environment the tmux server used, so variables from whichever terminal app launched paddock do not leak in.
- **Agent hooks.** Each terminal gets `PADDOCK_PANE` and `PADDOCK_SOCKET`, plus `AGY_CLI_OSC7501=on`. `paddock hook` reads them first and falls back to `TMUX` and `TMUX_PANE`. The event log is keyed by the server's instance id instead of the tmux server's pid.
- **Upgrades.** Client and server exchange a protocol version on connect. If an older server is running, paddock says so and offers to restart it. Restarting ends the running programs, and the layout comes back from [ADR 0008](0008-layout-saved-and-restored.md).

## Both backends during v0.1

`[backend] kind = "native" | "tmux"` picks the backend, and `native` is the default. Everything paddock does goes through one backend interface, which tmux implements first. The test suite reads state through `paddock debug state --json`, which both backends answer, and CI runs the whole suite on each. A running `tmux -L paddock` server from an older paddock is not migrated: paddock notes once that it is still running. tmux is removed in v0.2, unless a problem that only tmux avoids is still open.

## Considered options

- **Keep tmux and only restore after a reboot**: least work, but every tmux limit above stays.
- **Several clients on one server, as herdr does**: not needed for one window; the protocol leaves room for it later.
- **The server sends drawn frames**: a thinner client, but copy mode, search and selection would have to move into the server.
- **Hand PTYs to the new binary on upgrade (`exec` with inherited descriptors)**: programs would survive an upgrade, but it is much more work. Possible later.
- **Replace tmux in one step**: no fallback while the native backend is new.

## Consequences

- paddock is now a long-lived process host, which is what ADR 0003 avoided. A server crash ends every terminal, as a tmux crash did. ADR 0008 brings the layout back.
- paddock reads each terminal's directory and foreground command itself (macOS `libproc`), instead of tmux's `pane_current_path` and `pane_current_command`. Linux needs its own reader later.
- `tmux -L paddock attach` no longer shows paddock's terminals.
- The server answers terminal queries (DA, XTVERSION, `CSI ? u`) from its emulator, which tmux used to do.
