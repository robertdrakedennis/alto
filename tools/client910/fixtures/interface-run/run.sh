#!/bin/bash
# The scripted online run of the interface model kinds (see docs/client-status.md):
# log in at the development spawn, mount a cache interface whose components carry
# fixed cache models (the default model kind), then mount a plain model interface
# and show on it the player's head with a chat emote (model kind 3 with the
# component animator) and a head built from three identity kits (model kind 7).
# Starts a lobby and a world of the dev server on their own ports (BASE, BASE + 1)
# and the release client with the scripted server commands; PNGs go to OUT_DIR.
# Every process is stopped by PID at the end; the client exits after its last
# screenshot.
#
#   tools/client910/fixtures/interface-run/run.sh [OUT_DIR]
#
# Env: BASE (47794), MAX_SECONDS (240), CARGO_TARGET_DIR, RENDERER (modern, the
# default, or faithful-gpu), SHOTS (comma list of logic cycles for PNGs), COMMANDS (a
# newline separated list of server commands, default below), EXTRA (further client
# arguments). Launches the client once.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
BASE=${BASE:-47794}
MAX_SECONDS=${MAX_SECONDS:-240}
RENDERER=${RENDERER:-modern}
case "$BASE$MAX_SECONDS" in *[!0-9]*) echo "BASE and MAX_SECONDS must be numbers" >&2; exit 2;; esac
case "$RENDERER" in faithful-gpu|classic|modern) ;; *) echo "RENDERER is modern, faithful-gpu or classic" >&2; exit 2;; esac
OUT=${1:-$(mktemp -d)}
WORK=$(mktemp -d)
mkdir -p "$OUT" "$WORK/players" "$WORK/client"
PIDS=""
cleanup() {
  for pid in $PIDS; do kill -TERM "$pid" 2>/dev/null || true; done
  sleep 2
  for pid in $PIDS; do kill -KILL "$pid" 2>/dev/null || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT
for offset in 0 1; do
  if lsof -iTCP:$((BASE + offset)) -sTCP:LISTEN -P -n >/dev/null 2>&1; then
    echo "port $((BASE + offset)) is in use" >&2; exit 2
  fi
done
(cd "$ROOT/tools/client910" && cargo build --release)
BIN=${CARGO_TARGET_DIR:-$ROOT/tools/target}/release/client910

cd "$ROOT/server"
export ALTO_LOBBY_PORT=$BASE ALTO_WORLD_PORT=$((BASE + 1)) ALTO_PLAYER_DATA_DIR=$WORK/players ALTO_DEV_NPC_WANDER=0 ALTO_NPC_SPAWNS=dev
LOADER="node"
$LOADER src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 </dev/null & PIDS="$PIDS $!"
$LOADER src/lostcity/world.ts > "$WORK/world.log" 2>&1 </dev/null & PIDS="$PIDS $!"
waited=0
until grep -q "listening on port $((BASE + 1))" "$WORK/world.log" 2>/dev/null \
   && grep -q "Listening on port $BASE" "$WORK/lobby.log" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt 120 ] || { echo "the servers did not start" >&2; exit 1; }
done

[ ! -f "$ROOT/server/data/players/preferences.dat" ] || cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
COMMANDS=${COMMANDS:-"ifopensub 1477 637 52 0
ifopensub 1477 637 1190 0
playerhead 1190 4 556
ifangle 1190 4 0 0 2000
kithead 1190 4 0 10 45"}
CMD_ARGS=()
while IFS= read -r line; do
  [ -z "$line" ] || CMD_ARGS+=(--server-command "$line")
done <<< "$COMMANDS"
# One developer command per 100 cycles (the first at about cycle 100): interface 52
# (a log and boats, cache models) is up from about 110, the plain model interface
# 1190 replaces it from about 210 (both in the central window slot 1477:637), the
# player's head with the chat emote shows from about 310, its view is set from about
# 410 (two shots while the emote plays), and the three-kit head replaces it from about 510.
export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-190,460,480,590}
cd "$ROOT/tools/client910"
# shellcheck disable=SC2086
CLIENT910_LOG=info CLIENT910_WINDOW_SIZE=1024,768 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
"$BIN" --direct-login --lobby-port "$BASE" --world-port $((BASE + 1)) \
  --username interface --password password --cache-dir "$WORK/cache" \
  --renderer "$RENDERER" --screenshot "$OUT/interface.png" ${CMD_ARGS[@]+"${CMD_ARGS[@]}"} ${EXTRA:-} \
  > "$OUT/client.log" 2>&1 </dev/null & CLIENT=$!
PIDS="$PIDS $CLIENT"
waited=0
while kill -0 "$CLIENT" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt "$MAX_SECONDS" ] || { echo "the client did not finish in $MAX_SECONDS s" >&2; break; }
done
cp "$WORK/world.log" "$WORK/lobby.log" "$OUT/" 2>/dev/null || true
echo "run output in $OUT"
