# Server

paddock's server, upgrading it, restoring layout and scrollback, the files under the state directory, and uninstalling. Back to the [README](../README.md).

## Upgrade

Your terminals run on paddock's server, which keeps running after you leave paddock. After you upgrade, run `paddock server upgrade`: the running server hands every terminal, with its screen and scrollback, to the new paddock, and the programs in them (agents, editors, builds) keep running. An open paddock reconnects to the upgraded server by itself, on the same space, tab and split. A server from before this command (v0.1.0-rc.1 and rc.2, which speak protocol version 1) cannot hand over, and the command says so and how to restart it instead, as below. A new paddock whose server speaks another protocol version says so and offers to upgrade it the same way ("Upgrade it? Its programs keep running. [Y/n]"), then opens on it with every program still running. A server of protocol version 1 is known to be from before upgrades, so paddock offers no upgrade: it says the server cannot hand its terminals over and offers only the restart, yes by default ("Restart it? Its programs and agents end; its layout comes back with a fresh shell in each terminal, without scrollback or agent sessions, which it never saved. [Y/n]"). If a newer server cannot hand over after all (it refuses, saying why, or gives no answer), paddock says why and offers to restart it instead ("Restart it? Its programs end; its layout comes back. [y/N]"). After a restart the tab bar says "paddock server restarted: its programs ended", then how many spaces came back. If the upgrade fails after the server has switched to the new paddock, there is nothing left to restart: paddock says the programs have ended, and the layout comes back as below. Restarting ends the programs in the server's terminals, and its layout (spaces, tabs, splits and their directories, saved in `~/.local/state/paddock/layout.json`) comes back with a fresh shell in each terminal, under the scrollback it had (none after a server of protocol version 1, which saved no scrollback and no agent sessions; otherwise saved in `~/.local/state/paddock/scrollback/` as the server ends, up to 1 MiB of text per terminal, below a dim line saying when; searchable in copy mode like any scrollback). After a crash or a SIGKILL the server saves nothing, so each terminal shows the history saved at the last clean end, under that older date (or none). The first upgrade to a paddock that saves it restores none, as the older server wrote none. Say no to either and paddock stops; leave paddock and finish what runs first, then start it again. Floats and other programs (an editor, a server) are not restarted.

Agent sessions in those terminals: see [Resume after a restart](connect-agents.md#resume-after-a-restart).

**From paddock 0.0.x (tmux).** paddock runs on its own server, not on a hidden tmux server; the tmux backend was removed in v0.2. If your config still has `[backend] kind = "tmux"`, paddock refuses to start and says to delete that line. An old `tmux -L paddock` server is not migrated, and paddock no longer looks for one: its spaces and programs stay where they are until you close it. Finish what runs there first (`tmux -L paddock attach`, with the old paddock's spaces as tmux sessions), then:

```sh
tmux -L paddock kill-server         # ends every terminal of the old paddock
```

Hooks in paddock's terminals get `PADDOCK_PANE` and `PADDOCK_SOCKET`, by which `paddock hook` tells which terminal an event comes from; `TMUX` and `TMUX_PANE` are not read any more, so a script of yours that read `TMUX_PANE` should read `PADDOCK_PANE`.

## Files

- State: `~/.local/state/paddock/` (the server's `server.sock`, `server.lock`, `server.log` and `layout.json`, sidebar width, event logs, dismissed rows, `hook-errors.log`, `gh.log`)

## Uninstall

```sh
paddock hooks uninstall --agent claude --agent gemini --agent agy   # each agent you connected
# First close every space in paddock (sidebar `x`): the server ends with the last one.
tmux -L paddock kill-server         # an old paddock's tmux server (0.0.x), if one still runs
rm -rf ~/.local/state/paddock ~/.config/paddock
cargo uninstall paddock             # or brew uninstall paddock, or delete the binary you moved
```
