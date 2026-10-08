#!/bin/bash
# The scripted online run of the entity presentation (see docs/client-status.md):
# log in at the development spawn, put on a staff whose worn model emits
# particles, play a spot animation with particles, hint an NPC, show overhead
# icons, take a hit and fight a goblin's hitsplats, and hover an NPC with an
# item selected for use (the targeted-operation cursor). Starts a lobby and a
# world of the dev server on their own ports (BASE, BASE + 1) and the release
# client with the scripted server commands; PNGs go to OUT_DIR. Every process
# is stopped by PID at the end; the client exits after its last screenshot.
#
#   tools/client910/fixtures/entity-run/run.sh [OUT_DIR]
#
# Env: BASE (47694), MAX_SECONDS (240), CARGO_TARGET_DIR, RENDERER (modern, the
# default, or faithful-gpu), SHOTS (comma list of logic cycles for PNGs), COMMANDS (a newline
# separated list of server commands, default below), CLIENT910_UI_CLICKS and
# CLIENT910_UI_HOVER (the scripted mouse), EXTRA (further client arguments).
# Launches the client once.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
BASE=${BASE:-47694}
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
$LOADER src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & PIDS="$PIDS $!"
$LOADER src/lostcity/world.ts > "$WORK/world.log" 2>&1 & PIDS="$PIDS $!"
waited=0
until grep -q "listening on port $((BASE + 1))" "$WORK/world.log" 2>/dev/null \
   && grep -q "Listening on port $BASE" "$WORK/lobby.log" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt 120 ] || { echo "the servers did not start" >&2; exit 1; }
done

[ ! -f "$ROOT/server/data/players/preferences.dat" ] || cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
COMMANDS=${COMMANDS:-"wear 1381
spot 155 0
hintarrow npc 1
headicon 0 0 440
hit 7
hit npc 1 12
invset 0 385 1
npcheadicon 1 0 440 1"}
CMD_ARGS=()
while IFS= read -r line; do
  [ -z "$line" ] || CMD_ARGS+=(--server-command "$line")
done <<< "$COMMANDS"
# One developer command per 100 cycles (the first at about cycle 100): the staff
# is worn from 106, the spot animation plays around 230, the hint arrow shows
# from 330, the head icon from 430, the hits around 520 and 620, the NPC's icon
# from 830. Then the shark in the backpack is right-clicked (850), "Use" is
# chosen (880), the pointer rests on empty ground (881) and then on the NPC
# (900): the targeted-use cursor over each (the `[cursor]` lines of client.log).
export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-230,330,430,520,620,830,870,960}
export CLIENT910_UI_CLICKS=${CLIENT910_UI_CLICKS-"r,842,408,850;l,820,452,880"}
export CLIENT910_UI_HOVER=${CLIENT910_UI_HOVER-"500,300,881,899;298,204,900,99999"}
cd "$ROOT/tools/client910"
# shellcheck disable=SC2086
CLIENT910_LOG=info CLIENT910_WINDOW_SIZE=1024,768 CLIENT910_CURSOR_TRACE=1 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
"$BIN" --direct-login --lobby-port "$BASE" --world-port $((BASE + 1)) \
  --username entity --password password --cache-dir "$WORK/cache" \
  --renderer "$RENDERER" --screenshot "$OUT/entity.png" ${CMD_ARGS[@]+"${CMD_ARGS[@]}"} ${EXTRA:-} \
  > "$OUT/client.log" 2>&1 & CLIENT=$!
PIDS="$PIDS $CLIENT"
waited=0
while kill -0 "$CLIENT" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt "$MAX_SECONDS" ] || { echo "the client did not finish in $MAX_SECONDS s" >&2; break; }
done
cp "$WORK/world.log" "$WORK/lobby.log" "$OUT/" 2>/dev/null || true
echo "run output in $OUT"
