# No paddock daemon, and one paddock instance at a time

> No daemon is superseded by [ADR 0007](0007-native-backend-with-a-server.md): paddock hosts its own terminals in a server, and this half held only for the `tmux` backend until it was removed in v0.2. One instance at a time, with takeover, stands.

paddock has no background process of its own. The hidden tmux server keeps terminals alive, and Claude Code hooks (`paddock hook <event>`) record events into `~/.local/state/paddock/` while the app is closed, so nothing of paddock's runs unless the user is looking at it. The cost is that the layout lives in the running app and a state file, so only one instance may run; a second launch offers to take over from the first.

## Considered options

- **A paddock server with many clients, as herdr does**: several windows at once, but one more long-lived process to start, upgrade and recover, for a personal tool that needs one window.

## Consequences

- The one instance holds a `flock` on `paddock.pid`, which names its pid. A `flock` belongs to the open file, and the file is close-on-exec, so a child shares the lock only in the gap between its fork and its `exec` (or exit); a child in that gap as paddock ends (one `std` forks rather than spawns, as the server's start with its `pre_exec` is) kept the lock a moment after paddock had gone, up to 700 ms on a loaded machine (ticket 58). So paddock lets go of the lock (`LOCK_UN`, which frees it whoever shares the file) as it ends; and, for an end that skips that (a crash, a kill), a launch that finds the lock held for a pid that no longer runs waits for the lock (up to 5 s, then asks as before) rather than offering to take over from a paddock that has ended. A POSIX record lock, which a child does not share, was not taken: on Linux it does not see a `flock`, so a paddock from before and one from after such a change could both run.
