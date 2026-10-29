# No paddock daemon, and one paddock instance at a time

> No daemon is superseded by [ADR 0007](0007-native-backend-with-a-server.md): paddock hosts its own terminals in a server, and this half stays true only for the `tmux` backend until it is removed (planned for v0.2). One instance at a time, with takeover, stands.

paddock has no background process of its own. The hidden tmux server keeps terminals alive, and Claude Code hooks (`paddock hook <event>`) record events into `~/.local/state/paddock/` while the app is closed, so nothing of paddock's runs unless the user is looking at it. The cost is that the layout lives in the running app and a state file, so only one instance may run; a second launch offers to take over from the first.

## Considered options

- **A paddock server with many clients, as herdr does**: several windows at once, but one more long-lived process to start, upgrade and recover, for a personal tool that needs one window.

## Consequences

- The one instance holds a `flock` on `paddock.pid`, which names its pid. A `flock` belongs to the open file, so a program paddock starts shares it from its fork until its `exec` or exit, and the lock can outlive paddock by a moment. A launch that finds the lock held for a pid that no longer runs therefore waits for the lock (up to 5 s) rather than offering to take over from a paddock that has ended (ticket 58). A POSIX record lock, which a child does not share, was not taken: on Linux it does not see a `flock`, so a paddock from before and one from after such a change could both run.
