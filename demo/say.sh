#!/bin/sh
# Play the person in a demo recording started by demo/record.sh.
#
#   demo/say.sh [--hold S] "message"   type a message at a steady pace and send it
#   demo/say.sh [--hold S] --key KEY   press a key (Enter, Escape, ...)
#   demo/say.sh --screen               print the screen
#   demo/say.sh [--hold S] --end       stop: quit Pathfinder and end the recording
#
# Each action waits for Pathfinder to finish replying, then prints the
# screen. --hold S is how many seconds the video keeps the screen before
# this action (time to read the last reply; default 3); cut.py drops the
# rest of the time you spent deciding. DEMO_TYPE_DELAY sets the seconds per
# character (default 0.03; 0.018 suits long messages).
set -eu
ROOT=${DEMO_ROOT:-/tmp/pathfinder-demo}
ROOT=$(cd "$ROOT" && pwd -P)
T="tmux -L pathfinder-demo"
LOG="$ROOT/timeline.log"
now() { python3 -c 'import time; print(f"{time.time():.2f}")'; }
busy() { $T capture-pane -p -t demo | grep -Eq 'Working [0-9]+s|Esc stops'; }
screen() { $T capture-pane -p -t demo; }
settle() {
  # A reply starts within a moment of sending, or the step was instant.
  i=0; while ! busy && [ $i -lt 15 ]; do sleep 0.2; i=$((i+1)); done
  i=0; while busy && [ $i -lt 1500 ]; do sleep 0.4; i=$((i+1)); done
  sleep 1; screen
}

hold=3
[ "${1:-}" = "--hold" ] && { hold=$2; shift 2; }
case "${1:-}" in
  --screen) screen ;;
  --key)
    echo "$(now) key hold=$hold" >> "$LOG"
    $T send-keys -t demo "$2"
    settle ;;
  --end)
    echo "$(now) end hold=$hold" >> "$LOG"
    sleep 0.5
    $T kill-server 2>/dev/null || true
    echo "Ended. When VHS has written $ROOT/raw.mp4, run demo/cut.py." ;;
  "") echo "usage: demo/say.sh [--hold S] \"message\" | --key KEY | --screen | --end" >&2; exit 2 ;;
  *)
    msg=$1
    echo "$(now) type hold=$hold" >> "$LOG"
    # One character at a time, at a steady typing pace.
    printf '%s' "$msg" | DELAY=${DEMO_TYPE_DELAY:-0.03} python3 -c '
import os, subprocess, sys, time
text = sys.stdin.read()
d = float(os.environ["DELAY"])
for ch in text:
    subprocess.run(["tmux", "-L", "pathfinder-demo", "send-keys", "-t", "demo", "-l", "\\;" if ch == ";" else ch])
    time.sleep(d if ch != " " else d * 5 / 3)
'
    sleep 0.5
    echo "$(now) send" >> "$LOG"
    $T send-keys -t demo Enter
    settle ;;
esac
