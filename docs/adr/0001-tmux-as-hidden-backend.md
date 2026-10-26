# tmux hosts every terminal, hidden behind paddock

> tmux as the host is superseded by [ADR 0007](0007-native-backend-with-a-server.md): paddock hosts its own terminals in a server, and tmux stays only as the `tmux` backend until it is removed (planned for v0.2). One tmux session per terminal was already superseded by [ADR 0006](0006-sidebar-items-are-tmux-sessions.md). The clean environment stands.

paddock runs each terminal in its own session on a private tmux server (`tmux -L paddock`) that the user never sees, so terminals survive closing paddock or the terminal app without paddock hosting processes itself. The server uses paddock's own minimal config and starts with a clean environment, so neither the user's tmux setup nor variables from whichever terminal app launched it (for example `SUPACODE_*`) leak into paddock's shells.

## Considered options

- **Own PTY server, as herdr does**: full control, but paddock would have to be the long-running process host, which conflicts with having no daemon (ADR 0003).
- **Claude Code's `--bg` supervisor**: hosts only Claude, not shells, and stops idle unattached sessions after about an hour.
- **The user's default tmux server**: the user's config, prefix and plugins would leak into paddock, and tmux-resurrect would also restore paddock's sessions.

## Consequences

- A reboot or a crashed tmux server ends every terminal. Restoring them with `claude --resume` is planned for after v1.
