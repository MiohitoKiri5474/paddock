# Spike 01 results: Claude through control mode

Run on 2026-10-19: macOS, tmux 3.8, Rust 1.93, Claude Code 2.1.303 (plan mode, fullscreen UI).
Decision: **alacritty_terminal 0.26 plus its VS16 width fix** (the `yas-alacritty-terminal` fork for now). See [ADR 0005](../../docs/adr/0005-alacritty-terminal-emulator.md).

## How to rerun

```sh
cargo build --release
./target/release/paddock-spike-01 check render     # colours, widths, paste framing
./target/release/paddock-spike-01 check history    # 3000 lines, wheel, detach/reattach (also mid-stream)
./target/release/paddock-spike-01 check claude-esc # Claude: Shift+Enter, paste, emoji, Esc, wheel, reattach (submits one no-tools prompt)
./target/release/paddock-spike-01 check bench
./cpu.sh yas shell; ./cpu.sh yas claude            # also: alacritty, vt100
./e2e.sh yas                                       # through the real TUI, driven by a second private tmux
./target/release/paddock-spike-01 kill-server
```

The `check` runs feed `%output` into all three crates at once. Each grid is compared with tmux's own copy: `capture-pane -p` for text and `capture-pane -p -e` for colours. `e2e.sh` runs the TUI inside a second private tmux server (`-L paddock-spike-outer`, with `extended-keys always` and `csi-u`). That server plays the user's terminal. The checks send keys to it, then diff what the TUI drew against the hidden pane. No run uses the default tmux server.

## Acceptance criteria

| Criterion | Result | Evidence |
|---|---|---|
| Shift+Enter inserts a newline | pass | `CSI 13;2u`, `CSI 27;2;13~` and `ESC CR` each put `alpha` and `beta` on two prompt lines, both before and after reattach. A negative control (`-`) gives `alpha-beta`. In e2e, the outer `send-keys S-Enter` goes through crossterm and the encoder and also works. |
| Esc interrupts with no noticeable delay | pass | Time from sending Esc to "Interrupted" appearing in `%output`: `CSI 27 u` takes 13–33 ms, bare `ESC` takes 79 ms (Claude's own ESC timeout). In e2e, about 200 ms, mostly the cost of polling `capture-pane`. |
| Multi-line bracketed paste arrives as one | pass | 12 lines show as `[Pasted text #1 +12 lines]`, and a 4.7 KB paste over 19 `send-keys` commands shows as `[Pasted text #2 +100 lines]`. Nothing was submitted in either case. A raw probe (`cat -v`) sees one `^[[200~`, one `^[[201~`, and CR for each newline. |
| Truecolor | pass | 96-step RGB gradients, 256 colours and 16 colours: 0 differing cells, in all three crates. |
| Wide characters and emoji | pass with fix, gap remains | CJK, fullwidth, plain emoji, flags and combining marks match tmux in all crates. VS16 (`⚠️ ✔️ ❤️ 1️⃣`): upstream alacritty is 1 column short, so text typed into Claude's prompt lands misaligned. yas and vt100 match tmux. Skin-tone and ZWJ sequences: every crate is 2–4 columns wider than tmux (tmux and Claude agree). This left stale cells once in Claude's prompt, but not in a second run. |
| Mouse wheel scrolls history | pass | Shell (no mouse mode): 10 notches scroll 30 lines of our own scrollback, and the top row matches tmux history. Claude (modes 1003 and 1006 on): the wheel is forwarded as SGR 64/65 and Claude scrolls its transcript (checked in-process and in e2e). |
| History intact after detach/reattach | pass | Quiet pane: all 3004 lines identical to `capture-pane -S -`, colours kept. Reattach in the middle of `seq 1 3000000`: the last 40 000 lines identical, nothing lost or doubled. Claude's alternate screen is identical after reattach and after a resize redraw. |
| Idle and busy CPU | pass | Idle shell or idle Claude, 10–20 s: TUI 0.0%, tmux server 0.0%. Flood of `seq 1 10000000` (about 10 s): TUI 44% for alacritty and yas, 83% for vt100, tmux server about 97% for all. The TUI kept up. |
| Findings recorded as an ADR | done | docs/adr/0005 |

## Crate comparison

| | alacritty_terminal 0.26 | yas-alacritty-terminal 0.26.0-yas.1 | vt100 0.18 |
|---|---|---|---|
| Throughput (12 MB, no tmux) | 134–163 MB/s | 154 MB/s | 82 MB/s |
| TUI CPU in flood | 44% | 44% | 83% |
| Kitty keyboard flags | tracked | tracked | not tracked |
| modifyOtherKeys | not tracked | not tracked | not tracked |
| Synchronized output (2026) | buffered | buffered | ignored (frames can tear) |
| VS16 emoji width | wrong (1) | right (2) | right (2) |
| Skin-tone / ZWJ width | wrong | wrong | wrong |
| Scrollback API | direct `Line(-n)` | same | only through `set_scrollback` windows |

The spike tracks the kitty flag stack and modifyOtherKeys itself (`KeyModeScanner` in `src/emu.rs`), so the crates' gaps there do not matter.

## Findings that matter for the build

1. **`send-keys -H` is raw.** It writes UTF-8 and escape bytes unchanged (tmux treats them as literal keys), so paddock owns all key, mouse and paste encoding. Pastes are sent in 256-byte chunks, which stays one paste.
2. **tmux does not remember kitty flags.** After reattach, `#{pane_key_mode}` still says `Ext 2`, but kitty flags are gone (Claude pushed `CSI > 5 u`). Shift+Enter still works. Esc falls back to bare `ESC`, which costs about 80 ms instead of about 20 ms. A pane user option round-trips (`set -p @paddock-keys …`). See ADR 0005 for when the stored value can be trusted.
3. **The reattach seed must be one batch.** Send `display` and three `capture-pane` commands on one control line. tmux runs them back to back and queues `%output` in order with the replies. Output before the last reply is dropped; output after it is fed. An earlier version also drained queued output after the replies, which would have lost data on a busy pane.
4. **Claude 2.1.303 runs fullscreen here.** It uses the alternate screen, mouse modes 1000/1002/1003/1006, bracketed paste, focus events, synchronized output 2026, kitty flags 5 and modifyOtherKeys 2. paddock's own scrollback therefore covers shells; over Claude, the wheel goes to Claude.
5. **tmux answers the queries.** Claude asks for DA1, XTVERSION, kitty `?u`, DECRQM 2026 and OSC 7501. The emulator's replies are dropped (`VoidListener`).

## Human checks

Run in Ghostty (or any terminal):

```sh
cd spikes/01-control-mode && cargo build --release && cd ../..
spikes/01-control-mode/target/release/paddock-spike-01 tui --session try --cmd 'claude --permission-mode plan'
```

`C-a d` detaches. Run the same command again to reattach. Then check:

1. **Esc.** Ask for something long, such as "print 1 to 3000, no tools", then press Esc. It should feel as fast as plain Ghostty.
2. **Typing.** Type, hold a key, press Shift+Enter, arrows and Backspace. Echo should feel the same as plain Ghostty.
3. **Visual.** Look at colours, the borders, the spinner, `⏺` and `❯`. Type `⚠️ ✅ ❤️ 👍🏽 👨‍👩‍👧 漢字` into the prompt, then delete it, and look for misaligned or leftover cells. Repeat with `--emu alacritty` to see the VS16 bug.
4. **Wheel.** Scroll over Claude. Then start a shell session (`--session sh --cmd zsh`), run `seq 1 500`, and wheel up and down. The status line shows the scroll offset.
5. **Clean up.** Run `spikes/01-control-mode/target/release/paddock-spike-01 kill-server`.
