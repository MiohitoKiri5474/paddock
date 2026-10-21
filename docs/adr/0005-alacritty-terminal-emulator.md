# alacritty_terminal emulates each terminal, and paddock encodes every key itself

Spike 01 ran Claude Code on the hidden server through control mode. It tested alacritty_terminal 0.26, its `yas-alacritty-terminal` fork, and vt100 0.18 ([results](../../spikes/01-control-mode/RESULTS.md)). All three drew Claude's screen cell for cell like tmux, and every acceptance check passed except emoji widths.

paddock uses alacritty_terminal, with the fork's small VS16 width fix:

- It is twice as fast as vt100 and uses half the CPU under a flood.
- It tracks kitty keyboard flags.
- It buffers synchronized output, so Claude's frames never tear.

Without the VS16 fix, `⚠️` or `❤️` typed into Claude's prompt lands one column off.

`send-keys -H` writes bytes to the pane untouched, including UTF-8. paddock therefore encodes every key, paste and mouse event itself, from the modes the program asked for in its output.

## Considered options

- **vt100**: a smaller API, and correct VS16 widths, but it is half as fast and does not track kitty flags or synchronized output.
- **Upstream alacritty_terminal without the fix**: VS16 emoji, which are common in model output, misalign.
- **wezterm-term**: only third-party forks are on crates.io, so it was not tried.

## Consequences

- **The fork is a one-person crate.** paddock pins it, or carries the same patch on upstream alacritty_terminal through `[patch]`, until the fix is upstream.
- **Some emoji widths still differ.** For skin-tone and ZWJ sequences, every crate is wider than tmux and Claude, so a line can drift after such an emoji. Fixing this needs grapheme-cluster widths in the emulator. It is accepted for now.
- **tmux does not keep kitty flags.** After a reattach, tmux still reports modifyOtherKeys (`#{pane_key_mode}`), but Claude's kitty flags are gone. Shift+Enter still works, because Claude accepts `CSI 13;2u`, `CSI 27;2;13~` and `ESC CR`. Bare Esc, however, takes about 80 ms instead of about 20 ms.
- **paddock stores the key modes it saw.** It keeps them in a pane user option (`@paddock-keys`) together with the pane's foreground process group. A stored value is trusted only while that process group is still in the foreground. If Claude has exited, its flags were popped, and sending `CSI 99;5u` for Ctrl+C would break the shell. Otherwise paddock falls back to `#{pane_key_mode}`. This rule is not tested yet; the spike only showed that the option round-trips.
- **Reattach seeds the emulator in one batch.** `display` and three `capture-pane` commands (history, the saved normal screen, the visible screen) go on one control line. tmux runs them back to back and orders `%output` with their replies. Output before the last reply is already in the captures, and output after it is fed normally. Tested while a flood was streaming.
- **Claude runs full screen.** It uses the alternate screen and mouse mode 1003+1006. Over Claude, the wheel goes to Claude, which scrolls its own transcript. paddock's own scrollback is for shells, and selecting text over Claude needs Shift, as the design already says.
- **tmux answers terminal queries.** The emulator's replies (DA, XTVERSION, `CSI ? u`) are dropped.
