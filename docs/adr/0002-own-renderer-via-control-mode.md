# paddock draws every terminal itself, fed by tmux control mode

> The control-mode feed is superseded by [ADR 0007](0007-native-backend-with-a-server.md): the app is a client of paddock's own server, and control mode went with the tmux backend, removed in v0.2. paddock still draws every terminal itself.

paddock's frontend is its own: a sidebar, workspaces, tabs and splits, with scrollback, selection and search that behave the same in any terminal app. So paddock connects to its tmux server as a control-mode client, feeds each tmux pane's raw output into its own terminal-emulator instance, and draws the result with ratatui, instead of showing a real tmux client.

## Considered options

- **Show a real tmux client** (a nested client, or moving the agent's tmux pane into the main area): least code, but the session view would be tmux's, not paddock's.
- **Embed one tmux client per terminal in a pseudo-terminal**: simpler input handling and tidier output to emulate, but scrollback, selection and search would stay in tmux copy mode.

## Consequences

- paddock must encode keys, mouse and paste for each program itself and emulate every program's raw output. This is the largest and riskiest part of the build, so a spike comes first (Shift+Enter, Esc latency, bracketed paste, truecolor, wide characters, mouse wheel, history after reopening, CPU) and picks the emulator crate.
- paddock is written in Rust for its mature terminal-emulator crates.
