# The server saves the layout and rebuilds it after a restart

A reboot, a server crash or a restart for an upgrade ([ADR 0007](0007-native-backend-with-a-server.md)) used to lose every space. Now the server writes the layout to `~/.local/state/paddock/layout.json` whenever it changes, at most a few times a second. The file holds spaces in order with their names, roots and current tab, tabs with their names (and whether each name is automatic), splits and their sizes, zoom and focus, and each terminal's directory. Floats are left out: their keys make them again when wanted.

When a server starts and finds that file, it rebuilds the layout without asking. Every terminal gets a fresh shell in its saved directory, or in `$HOME` if that directory is gone, and paddock shows a one-line notice with how many spaces came back. A terminal that ran an agent comes back as a plain shell, and the dashboard treats that agent as ended.

## Considered options

- **Nothing after a restart, as with tmux**: simplest, but an upgrade restart would cost the whole layout.
- **Also resume agents (`claude --resume <id>`, and Gemini or agy where they can) and restore scrollback**: planned next, on top of this file.

## Consequences

- `layout.json` replaces the layout tmux held and the current space kept in `layout.toml`; only the sidebar width stays there.
- Programs other than shells are not restarted, so an editor or a server left running in a terminal has to be started again.
- Servers from before this file wrote none, so the first upgrade to a paddock that reads it restores nothing.
- Since ticket 48 the file also keeps the agent session last seen in each terminal. A Claude Code session whose conversation is not over (it did not quit, nor was it cleared) is resumed (`claude --resume <id>` typed into the new shell) rather than coming back as a plain shell; Gemini and Antigravity sessions still come back as plain shells.
