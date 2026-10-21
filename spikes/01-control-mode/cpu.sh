#!/bin/sh
# CPU use of the spike TUI and the hidden tmux server, idle and under a flood.
# The TUI runs inside a second private tmux server (paddock-spike-outer) that
# stands in for the user's terminal app. Neither touches the default server.
# usage: ./cpu.sh [alacritty|vt100] [shell|claude]
set -eu
EMU=${1:-alacritty}
MODE=${2:-shell}
DIR=$(cd "$(dirname "$0")" && pwd)
BIN="$DIR/target/release/paddock-spike-01"
IN="tmux -L paddock-spike"
OUT="tmux -L paddock-spike-outer"
IDLE=${IDLE:-20}

cpu_secs() { # cumulative CPU seconds of a pid
  ps -o time= -p "$1" | awk -F'[:.]' '{ if (NF==3) print $1*60+$2+$3/100; else print $1*3600+$2*60+$3+$4/100 }'
}

$IN kill-session -t cpu 2>/dev/null || true
$OUT kill-server 2>/dev/null || true
if [ "$MODE" = claude ]; then CMD="claude --permission-mode plan"; else CMD="zsh -f"; fi
$OUT -f /dev/null new-session -d -s outer -x 120 -y 40 -c "$DIR/../.." \
  "$BIN tui --emu $EMU --session cpu --cmd '$CMD'"
sleep 3
[ "$MODE" = claude ] && sleep 10
TUI=$(pgrep -n -f "paddock-spike-01 tui --emu $EMU --session cpu")
SRV=$($IN display -p '#{pid}')

measure() { # label seconds
  a=$(cpu_secs "$TUI"); b=$(cpu_secs "$SRV"); t0=$(python3 -c "import time;print(time.time())")
  sleep "$2"
  a2=$(cpu_secs "$TUI"); b2=$(cpu_secs "$SRV"); t1=$(python3 -c "import time;print(time.time())")
  awk -v l="$1" -v a="$a" -v a2="$a2" -v b="$b" -v b2="$b2" -v d="$(echo "$t1 - $t0" | bc)" -v e="$EMU" -v m="$MODE" \
    'BEGIN { printf "INFO cpu %s %s %s: tui %.1f%%  tmux-server %.1f%% over %.1fs\n", e, m, l, (a2-a)*100/d, (b2-b)*100/d, d }'
}

measure idle "$IDLE"
if [ "$MODE" = shell ]; then
  a=$(cpu_secs "$TUI"); b=$(cpu_secs "$SRV"); t0=$(python3 -c "import time;print(time.time())")
  $IN send-keys -t cpu 'seq 1 10000000; echo FLOOD-DONE' Enter
  while ! $IN capture-pane -p -t cpu | grep -q '^FLOOD-DONE'; do sleep 0.2; done
  t1=$(python3 -c "import time;print(time.time())"); a2=$(cpu_secs "$TUI"); b2=$(cpu_secs "$SRV"); d=$(echo "$t1 - $t0" | bc); 
  awk -v a="$a" -v a2="$a2" -v b="$b" -v b2="$b2" -v d="$d" -v e="$EMU" \
    'BEGIN { printf "INFO cpu %s flood (seq 1 10000000, ~79 MB): tui %.1f%%  tmux-server %.1f%% over %.1fs (tui %.1fs cpu)\n", e, (a2-a)*100/d, (b2-b)*100/d, d, a2-a }'
  # Did the TUI keep up? Its status line shows the byte count; the grid should show FLOOD-DONE.
  sleep 1
  $OUT capture-pane -p -t outer | grep -E 'FLOOD-DONE|KiB in' | tail -2
fi
$OUT kill-server 2>/dev/null || true
$IN kill-session -t cpu 2>/dev/null || true
