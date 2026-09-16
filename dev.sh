#!/usr/bin/env bash
# Development loop: rebuild and restart Auriscope whenever a source file is
# saved. Debug profile, so a UI change is about one and a half seconds.
#
#   ./dev.sh                 # reopens whatever file you had last
#   ./dev.sh path/to.wav     # always opens this one
#
# Rust has no practical hot reload, so this restarts the process. Settings and
# the last file are persisted, so the app comes back where you left it: the
# same file loaded, same colour map, same zoom.
#
# Uses inotifywait when inotify-tools is installed, otherwise polls once a
# second, so it works with nothing installed at all.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

FILE="${1:-}"
WATCH=(src assets Cargo.toml build.rs)
APP_PID=""

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }

stop() {
  [ -n "$APP_PID" ] || return 0
  kill "$APP_PID" 2>/dev/null
  # Do not wait forever if it declines to go.
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.2
  done
  kill -KILL "$APP_PID" 2>/dev/null
  wait "$APP_PID" 2>/dev/null
  APP_PID=""
}

# Rebuild, then restart. A build failure leaves the running window alone, so
# you keep looking at the last version that worked while you fix it.
cycle() {
  say "── building ─────────────────────────────────────────"
  if ! cargo build; then
    say "build failed; leaving the running window as it was"
    return
  fi
  stop
  if [ -n "$FILE" ]; then
    ./target/debug/auriscope "$FILE" &
  else
    ./target/debug/auriscope &
  fi
  APP_PID=$!
  say "running (pid $APP_PID) — save a file to reload, Ctrl-C to stop"
}

cleanup() { stop; [ -n "${STAMP:-}" ] && rm -f "$STAMP"; }
trap cleanup EXIT
trap 'exit 0' INT TERM HUP

cycle
if command -v inotifywait >/dev/null 2>&1; then
  while inotifywait -qq -r -e close_write,move,create --include '.*\.(rs|toml)$' "${WATCH[@]}"; do
    cycle
  done
else
  # Portable fallback, needing nothing installed: poll for a source file newer
  # than the last cycle. One second of latency, no measurable cost.
  STAMP="$(mktemp)"
  while sleep 1; do
    changed="$(find "${WATCH[@]}" -type f \( -name '*.rs' -o -name '*.toml' \) \
      -newer "$STAMP" -print -quit 2>/dev/null)"
    if [ -n "$changed" ]; then
      touch "$STAMP"
      cycle
    fi
  done
fi
