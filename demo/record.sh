#!/bin/sh
# Record a Pathfinder demo.
#
#   PATHFINDER_DEMO_CREDENTIALS=/path/to/a/pathfinder/home demo/record.sh [energy-bill|research]
#
# energy-bill (the default): someone works out their electricity bill in
# ~/Documents/Electricity bill. research: a scientist works through a public
# preprint in ~/Research/mixture-uncertainty, a git repository, so the
# agent's work is staged for review (the paper is downloaded from arXiv).
#
# Sets up a throwaway home, starts a shell in a private tmux server and
# starts VHS (demo/<scenario>.tape), which records that tmux session. You
# then play the person with demo/say.sh, reading each reply before you
# answer: the candidate you judge is right or flawed at random, so the
# lines can't all be scripted ahead. `demo/say.sh --end` stops the
# recording and demo/cut.py shortens the waits and writes the gif and mp4.
#
# PATHFINDER_DEMO_CREDENTIALS is a folder holding an auth.json (and
# optionally config.json and cache/models.json), e.g. a PATHFINDER_HOME
# where you've run /login. The files are copied into the throwaway home
# under $DEMO_ROOT (default /tmp/pathfinder-demo), never into the repo, and
# a refreshed sign-in is copied back so the source stays signed in.
set -eu

SCENARIO=${1:-energy-bill}
case "$SCENARIO" in
  energy-bill|research) ;;
  *) echo "usage: demo/record.sh [energy-bill|research]" >&2; exit 2 ;;
esac
REPO=$(cd "$(dirname "$0")/.." && pwd)
DEMO="$REPO/demo"
ROOT=${DEMO_ROOT:-/tmp/pathfinder-demo}
FAST=${PATHFINDER_DEMO_FAST:-chatgpt:gpt-6-luna}
MAIN=${PATHFINDER_DEMO_MAIN:-chatgpt:gpt-6.2-sol}
SOCK=pathfinder-demo

CREDS=${PATHFINDER_DEMO_CREDENTIALS:-}
if [ -z "$CREDS" ] || [ ! -f "$CREDS/auth.json" ]; then
  cat >&2 <<EOF
Set PATHFINDER_DEMO_CREDENTIALS to a folder with an auth.json to record with,
for example a PATHFINDER_HOME where you've run /login:

  PATHFINDER_DEMO_CREDENTIALS=~/.local/share/pathfinder-dev demo/record.sh

The files are copied into a throwaway home under $ROOT, not into the repo.
EOF
  exit 1
fi
for tool in vhs tmux pdftotext jq ffmpeg python3 git curl; do
  command -v $tool >/dev/null || { echo "$tool is not installed (pdftotext comes with poppler)" >&2; exit 1; }
done

echo "Building the release binary..."
(cd "$REPO/app" && cargo build --release --quiet)

now() { python3 -c 'import time; print(f"{time.time():.2f}")'; }

# A throwaway home, outside any git repository, so the screen shows
# ~/Documents/... (or ~/Research/...).
tmux -L $SOCK kill-server 2>/dev/null || true
rm -rf "$ROOT"
mkdir -p "$ROOT"
ROOT=$(cd "$ROOT" && pwd -P) # the real path (/tmp is a link on macOS)
HOME_DIR="$ROOT/home"
PF_HOME="$ROOT/pathfinder-home"
case "$SCENARIO" in
  energy-bill)
    # Not a git repository: the agent's work is a plain edit in the folder.
    FOLDER="$HOME_DIR/Documents/Electricity bill"
    mkdir -p "$FOLDER"
    cp "$DEMO/energy-bill/bill-october-2026.pdf" "$DEMO/energy-bill/tariff-sheet.pdf" "$FOLDER/" ;;
  research)
    # A git repository holding the public preprint (Darijani et al. 2026,
    # arXiv:2603.20365), fetched from arXiv here and never kept in this repo.
    # In a repository a handoff runs in a staged copy and its review may be
    # checked.
    FOLDER="$HOME_DIR/Research/mixture-uncertainty"
    mkdir -p "$FOLDER"
    echo "Downloading arXiv:2603.20365..."
    curl -fsSL -A "pathfinder-demo" -o "$FOLDER/darijani2026-gmm-uncertainty.pdf" https://arxiv.org/pdf/2603.20365v1
    (cd "$FOLDER" && git init -q -b main \
      && git add . \
      && git -c user.name=Researcher -c user.email=researcher@example.invalid -c commit.gpgsign=false commit -q -m "Add the preprint") ;;
esac
mkdir -p "$PF_HOME" "$ROOT/bin"

(umask 077
 cp "$CREDS/auth.json" "$PF_HOME/auth.json"
 if [ -f "$CREDS/config.json" ]; then cp "$CREDS/config.json" "$PF_HOME/config.json"; else echo '{}' > "$PF_HOME/config.json"; fi
 # The model list, if the source has one, so the chosen models show from the start.
 if [ -f "$CREDS/cache/models.json" ]; then mkdir -p "$PF_HOME/cache"; cp "$CREDS/cache/models.json" "$PF_HOME/cache/"; fi
 # The demo's models, whatever the source config picked.
 jq --arg fast "$FAST" --arg main "$MAIN" '.system1 = $fast | .system2 = $main | del(.db_path)' \
   "$PF_HOME/config.json" > "$PF_HOME/config.json.tmp"
 mv "$PF_HOME/config.json.tmp" "$PF_HOME/config.json")

ln -sf "$REPO/app/target/release/pathfinder" "$ROOT/bin/pathfinder"

echo "PS1='\\w \$ '" > "$ROOT/bashrc"
cat > "$ROOT/tmux.conf" <<'EOF'
set -g status off
set -g escape-time 0
set -g window-size latest
set -g default-terminal "tmux-256color"
set -as terminal-overrides ",xterm-256color:Tc"
EOF

if [ -n "${DEMO_SETUP_ONLY:-}" ]; then
  echo "Set up $ROOT (DEMO_SETUP_ONLY): not recording."
  exit 0
fi

# The shell the viewer sees: the throwaway home, this build first on PATH,
# no telemetry and no update check.
env -i \
  HOME="$HOME_DIR" \
  PATH="$ROOT/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  TERM=xterm-256color COLORTERM=truecolor LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 \
  PATHFINDER_HOME="$PF_HOME" \
  PATHFINDER_TELEMETRY=off \
  PATHFINDER_NO_UPDATE_CHECK=1 \
  tmux -L $SOCK -f "$ROOT/tmux.conf" -u new-session -d -s demo -x 112 -y 34 -c "$FOLDER" \
    "bash --noprofile --rcfile '$ROOT/bashrc' -i"

: > "$ROOT/timeline.log"
# A sign-in's refresh token is replaced each time it is used, so a refresh
# in the throwaway copy would leave the source signed out. Copy a refreshed
# auth.json back to the source as soon as it changes, and once more at the
# end, so both stay on the same sign-in.
sync_auth() {
  if [ -f "$PF_HOME/auth.json" ] && ! cmp -s "$PF_HOME/auth.json" "$CREDS/auth.json"; then
    (umask 077; cp "$PF_HOME/auth.json" "$CREDS/auth.json.demo-tmp" && mv "$CREDS/auth.json.demo-tmp" "$CREDS/auth.json")
  fi
}
# Logs when each reply starts and ends, for cut.py.
(
  prev=idle
  while tmux -L $SOCK has-session -t demo 2>/dev/null; do
    if tmux -L $SOCK capture-pane -p -t demo 2>/dev/null | grep -Eq 'Working [0-9]+s|Esc stops'; then cur=busy; else cur=idle; fi
    if [ "$cur" != "$prev" ]; then echo "$(now) $cur" >> "$ROOT/timeline.log"; prev=$cur; fi
    sync_auth
    sleep 0.2
  done
  sync_auth
) > /dev/null 2>&1 &

cd "$ROOT"
nohup vhs "$DEMO/$SCENARIO.tape" > "$ROOT/vhs.log" 2>&1 &

# Wait for VHS to attach.
i=0
until [ "$(tmux -L $SOCK list-clients 2>/dev/null | wc -l)" -gt 0 ]; do
  sleep 0.1; i=$((i+1))
  [ $i -gt 900 ] && { echo "VHS didn't attach; see $ROOT/vhs.log" >&2; exit 1; }
done
echo "$(now) attached" >> "$ROOT/timeline.log"
echo "Recording into $ROOT. Drive it with demo/say.sh; demo/say.sh --end finishes."
