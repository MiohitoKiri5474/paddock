#!/bin/sh
# End-to-end through the real TUI: a second private tmux server
# (paddock-spike-outer) plays the user's terminal app and types into the TUI;
# the TUI forwards to Claude on the hidden server (paddock-spike).
# Claude runs in plan mode; only a trivial no-tools prompt is ever submitted.
# usage: ./e2e.sh [alacritty|vt100]
set -u
EMU=${1:-alacritty}
DIR=$(cd "$(dirname "$0")" && pwd)
BIN="$DIR/target/release/paddock-spike-01"
IN="tmux -L paddock-spike"
OUT="tmux -L paddock-spike-outer"
S=e2e

pass() { echo "PASS e2e/$EMU $1"; }
fail() { echo "FAIL e2e/$EMU $1"; }
inner() { $IN capture-pane -p -t $S; }
prompt() { inner | awk '/^────/{n++; next} n==1' | sed 's/[[:space:]]*$//'; }
clear_prompt() { for i in 1 2 3 4; do $OUT send-keys -t outer C-u BSpace; done; sleep 0.4; }
now() { python3 -c 'import time;print(time.time())'; }

$IN kill-session -t $S 2>/dev/null
$OUT kill-server 2>/dev/null
printf 'set -s escape-time 0\nset -s extended-keys always\nset -s extended-keys-format csi-u\nset -g remain-on-exit on\n' > "$DIR/target/outer.conf"
$OUT -f "$DIR/target/outer.conf" new-session -d -s outer -x 110 -y 41 -c "$DIR/../.." \
  "$BIN tui --emu $EMU --session $S --cmd 'claude --permission-mode plan'"
i=0; until inner 2>/dev/null | grep -q '^❯'; do sleep 0.5; i=$((i+1)); [ $i -gt 80 ] && break; done
sleep 2

# 1. What the TUI draws equals what tmux holds (first 40 rows; row 41 is our status line).
$OUT capture-pane -p -t outer | head -40 | sed 's/[[:space:]]*$//' > "$DIR/target/outer.txt"
inner | head -40 | sed 's/[[:space:]]*$//' > "$DIR/target/inner.txt"
if diff -q "$DIR/target/outer.txt" "$DIR/target/inner.txt" >/dev/null; then pass "drawn screen == tmux screen"
else fail "drawn screen differs: $(diff "$DIR/target/outer.txt" "$DIR/target/inner.txt" | head -4 | tr '\n' ' ')"; fi
$OUT -f /dev/null capture-pane -p -t outer | tail -1

# 2. Shift+Enter typed into the outer terminal.
clear_prompt
$OUT send-keys -t outer -l alpha; $OUT send-keys -t outer S-Enter; $OUT send-keys -t outer -l beta; sleep 0.6
P=$(prompt)
if [ "$(printf '%s\n' "$P" | grep -c 'alpha\|beta')" = 2 ]; then pass "Shift+Enter newline"; else fail "Shift+Enter: $P"; fi

# 3. Bracketed paste from the outer terminal.
clear_prompt
printf 'p1\np2\np3\np4\n' | $OUT load-buffer -
$OUT paste-buffer -p -t outer; sleep 0.8
P=$(prompt)
case "$P" in *"Pasted text"*|*p1*p2*) pass "paste as one: $(echo $P)";; *) fail "paste: $P";; esac
clear_prompt

# 4. Esc interrupts.
$OUT send-keys -t outer -l 'Print the integers from 1 to 3000, one per line, as plain text. Do not use any tools.'
sleep 0.3; $OUT send-keys -t outer Enter
i=0; until inner | grep -qx '  *40'; do sleep 0.1; i=$((i+1)); [ $i -gt 400 ] && break; done
sleep 0.5; t0=$(now); $OUT send-keys -t outer Escape
i=0; until inner | grep -q 'Interrupted'; do sleep 0.01; i=$((i+1)); [ $i -gt 500 ] && break; done
t1=$(now)
ms=$(echo "($t1 - $t0) * 1000 / 1" | bc)
if inner | grep -q Interrupted; then pass "Esc interrupts (${ms} ms incl. capture-pane polling)"; else fail "Esc did not interrupt"; fi
sleep 1

# 5. Wheel over Claude (mouse mode on): forwarded, Claude scrolls its transcript.
B=$(inner | md5)
for i in 1 2 3 4 5; do $OUT send-keys -t outer -H 1b 5b 3c 36 34 3b 31 30 3b 31 30 4d; done; sleep 0.6
A=$(inner | md5)
[ "$A" != "$B" ] && pass "wheel forwarded to Claude scrolls it" || fail "wheel: screen unchanged"
for i in 1 2 3 4 5; do $OUT send-keys -t outer -H 1b 5b 3c 36 35 3b 31 30 3b 31 30 4d; done; sleep 0.4

# 6. Detach (C-a d) and reattach a fresh TUI: same screen.
$OUT send-keys -t outer C-a d; sleep 0.5
$OUT respawn-pane -k -t outer "$BIN tui --emu $EMU --session $S"; sleep 2
$OUT capture-pane -p -t outer | head -40 | sed 's/[[:space:]]*$//' > "$DIR/target/outer.txt"
inner | head -40 | sed 's/[[:space:]]*$//' > "$DIR/target/inner.txt"
if diff -q "$DIR/target/outer.txt" "$DIR/target/inner.txt" >/dev/null; then pass "screen after detach/reattach"
else fail "after reattach differs: $(diff "$DIR/target/outer.txt" "$DIR/target/inner.txt" | head -4 | tr '\n' ' ')"; fi
$OUT capture-pane -p -t outer | tail -1
clear_prompt
$OUT send-keys -t outer -l alpha; $OUT send-keys -t outer S-Enter; $OUT send-keys -t outer -l beta; sleep 0.6
P=$(prompt)
if [ "$(printf '%s\n' "$P" | grep -c 'alpha\|beta')" = 2 ]; then pass "Shift+Enter after reattach"; else fail "Shift+Enter after reattach: $P"; fi
clear_prompt

$OUT kill-server 2>/dev/null
$IN kill-session -t $S 2>/dev/null
