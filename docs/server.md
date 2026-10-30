# Server

paddock's server, upgrading it, restoring layout and scrollback, the files under the state directory, and uninstalling. Back to the [README](../README.md).

## Upgrade

Your terminals run on paddock's server, which keeps running after you leave paddock. After you upgrade, run `paddock server upgrade`: the running server hands every terminal, with its screen and scrollback, to the new paddock, and the programs in them (agents, editors, builds) keep running. An open paddock reconnects to the upgraded server by itself, on the same space, tab and split. A server from before this command (v0.1.0-rc.1 and rc.2, which speak protocol version 1) cannot hand over, and the command says so and how to restart it instead, as below. A new paddock whose server speaks another protocol version says so and offers to upgrade it the same way ("Upgrade it? Its programs keep running. [Y/n]"), then opens on it with every program still running. A server of protocol version 1 is known to be from before upgrades, so paddock offers no upgrade: it says the server cannot hand its terminals over and offers only the restart, yes by default ("Restart it? Its programs and agents end; its layout comes back with a fresh shell in each terminal. [Y/n]"). If a newer server cannot hand over after all (it refuses, saying why, or gives no answer), paddock says why and offers to restart it instead ("Restart it? Its programs end; its layout comes back. [y/N]"). After a restart the tab bar says "paddock server restarted: its programs ended", then how many spaces came back. If the upgrade fails after the server has switched to the new paddock, there is nothing left to restart: paddock says the programs have ended, and the layout comes back as below. Restarting ends the programs in the server's terminals, and its layout (spaces, tabs, splits and their directories, saved in `~/.local/state/paddock/layout.json`) comes back with a fresh shell in each terminal, under the scrollback it had (saved in `~/.local/state/paddock/scrollback/` as the server ends, up to 1 MiB of text per terminal, below a dim line saying when; searchable in copy mode like any scrollback). After a crash or a SIGKILL the server saves nothing, so each terminal shows the history saved at the last clean end, under that older date (or none). The first upgrade to a paddock that saves it restores none, as the older server wrote none. Say no to either and paddock stops; leave paddock and finish what runs first, then start it again. Floats and other programs (an editor, a server) are not restarted.

Agent sessions in those terminals: see [Resume after a restart](connect-agents.md#resume-after-a-restart).

**From paddock 0.0.x (tmux).** paddock now runs on its own server by default, not on a hidden tmux server. The old `tmux -L paddock` server is not migrated: its spaces and programs stay where they are, and the new paddock starts empty. While that server runs, paddock says so in the tab bar, and prints it again however it ends (on leaving, or on an error at start), once per server: once printed, it remembers that server's pid and start time in `~/.local/state/paddock/old-tmux-server`, and a different server is said again. Finish what runs there first: `tmux -L paddock attach` attaches to that old server, with the old paddock's spaces as tmux sessions. Then close it:

```sh
tmux -L paddock kill-server         # ends every terminal of the old paddock
```

The new paddock's terminals live on its own server, so `tmux -L paddock attach` never shows them. Hooks in paddock's terminals get `PADDOCK_PANE` and `PADDOCK_SOCKET` instead of `TMUX` and `TMUX_PANE`, by which `paddock hook` tells which terminal an event comes from; a script of yours that read `TMUX_PANE` should read `PADDOCK_PANE` now. To keep the tmux backend until v0.2, set `[backend] kind = "tmux"` (see [Configure](config.md)).

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
