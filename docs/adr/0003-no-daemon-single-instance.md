# No paddock daemon, and one paddock instance at a time

> No daemon is superseded by [ADR 0007](0007-native-backend-with-a-server.md): paddock hosts its own terminals in a server, and this half stays true only for the `tmux` backend until v0.2. One instance at a time, with takeover, stands.

paddock has no background process of its own. The hidden tmux server keeps terminals alive, and Claude Code hooks (`paddock hook <event>`) record events into `~/.local/state/paddock/` while the app is closed, so nothing of paddock's runs unless the user is looking at it. The cost is that the layout lives in the running app and a state file, so only one instance may run; a second launch offers to take over from the first.

## Considered options

- **A paddock server with many clients, as herdr does**: several windows at once, but one more long-lived process to start, upgrade and recover, for a personal tool that needs one window.
