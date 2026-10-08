#!/bin/sh
# Regenerate the woodcutting session-replay fixture
# (app::scenario_tests::recorded_woodcutting_session_*):
#   tools/client910/fixtures/session-replay/woodcutting/record.sh
# Starts the dev lobby/world with a scratch player directory,
# ALTO_TRACE_INFO=1 and the TEST-ONLY ALTO_WC_ALWAYS=1 (every woodcutting
# success roll hits, World.loadContent), then runs the release client with
# CLIENT910_RECORD and these scripted inputs (the window must stay untouched;
# recording drops operating-system input):
# - server commands (one per 100 cycles): notimeout, `invset 0 1351 1`
#   (bronze hatchet in backpack slot 0), `setxp 8 58` (the first log levels
#   Woodcutting up), `ifopensub 1477 115 1466 1` (the Skills tab 1466, which
#   this dev server never opens: its own window 0 slot 1477:361 (enum 7716 ->
#   struct 21293 param 3505) sits in the hidden layer 1477:359 of the dev
#   layout, where the client runs no hooks, so it goes into the visible
#   All Chat window slot 1477:115 instead and its stat rows are looped);
# - a left click on the minimap at (955,108) at cycle 250 (MOVE_MINIMAPCLICK
#   from the spawn (3222,3222) east of the castle wall, below the tree), so
#   the default camera shows tree 38760 at (3228,3228) once it settles (the
#   camera keys are frame-time based in the client and
#   would not replay);
# - the mouse on the tree's trunk (TREE_CLICK, default 447,97: the middle of
#   the trunk's pick capsule in the replayed camera after the walk settles,
#   found with the replay's headless pick frame) from HOVER_CYCLE (700) and a
#   left click there at CLICK_CYCLE (750): the "Chop down" OPLOC1. An empty
#   TREE_CLICK records the walk only (to re-derive the point).
# The client is stopped RESPAWN_TAIL cycles after the tree's respawn
# (postprocess.py --respawn) or after MAX_SECONDS.
# Env: TREE_CLICK (x,y), HOVER_CYCLE, CLICK_CYCLE, LOBBY_PORT (45640), WORLD_PORT (45641), MAX_SECONDS (150), CARGO_TARGET_DIR.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../../.." && pwd)
POST="$HERE/../postprocess.py"
LOBBY_PORT=${LOBBY_PORT:-45640}
WORLD_PORT=${WORLD_PORT:-45641}
MAX_SECONDS=${MAX_SECONDS:-150}
TREE_CLICK=${TREE_CLICK-447,97}
HOVER_CYCLE=${HOVER_CYCLE:-700}
CLICK_CYCLE=${CLICK_CYCLE:-750}
WORK=$(mktemp -d)
# stop_pids PID...: SIGTERM each recorded PID, SIGKILL only those still alive after a grace period (10 s).
stop_pids() {
  for pid in "$@"; do kill -TERM "$pid" 2>/dev/null || true; done
  grace=0
  while [ $grace -lt 10 ]; do
    alive=
    for pid in "$@"; do
      state=$(ps -o stat= -p "$pid" 2>/dev/null | cut -c1)
      [ -z "$state" ] || [ "$state" = Z ] || alive=1
    done
    [ -n "$alive" ] || return 0
    sleep 1; grace=$((grace + 1))
  done
  for pid in "$@"; do kill -KILL "$pid" 2>/dev/null || true; done
}
trap 'stop_pids $LOBBY $WORLD; rm -rf "$WORK"' EXIT
(cd "$ROOT/tools/client910" && cargo build --release)
BIN=${CARGO_TARGET_DIR:-$ROOT/tools/target}/release/client910
cd "$ROOT/server"
export ALTO_LOBBY_PORT=$LOBBY_PORT ALTO_WORLD_PORT=$WORLD_PORT ALTO_PLAYER_DATA_DIR=$WORK/players ALTO_TRACE_INFO=1 ALTO_WC_ALWAYS=1 ALTO_NPC_SPAWNS=dev
mkdir -p "$WORK/players" "$WORK/client"
node src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & LOBBY=$!
node src/lostcity/world.ts > "$WORK/world.log" 2>&1 & WORLD=$!
until grep -qi "listening on port $WORLD_PORT" "$WORK/world.log"; do sleep 1; done
cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
cd "$ROOT/tools/client910"
CLICKS="l,955,108,250"
HOVER=
if [ -n "$TREE_CLICK" ]; then
  CLICKS="$CLICKS;l,$TREE_CLICK,$CLICK_CYCLE"
  HOVER="$TREE_CLICK,$HOVER_CYCLE,99999"
fi
env CLIENT910_RECORD="$WORK/raw.rtr" CLIENT910_WINDOW_SIZE=1024,768 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
CLIENT910_UI_CLICKS="$CLICKS" ${HOVER:+CLIENT910_UI_HOVER=$HOVER} \
"$BIN" --direct-login --lobby-port "$LOBBY_PORT" --world-port "$WORLD_PORT" \
  --username woodcut --password password --cache-dir "$WORK/cache" \
  --server-command "notimeout" --server-command "invset 0 1351 1" \
  --server-command "setxp 8 58" --server-command "ifopensub 1477 115 1466 1" \
  > "$WORK/client.log" 2>&1 & CLIENT=$!
waited=0
until python3 "$POST" --done-respawn "$WORK/raw.rtr" || [ $waited -ge "$MAX_SECONDS" ]; do
  sleep 1; waited=$((waited + 1))
done
stop_pids $CLIENT
if [ -n "${KEEP_WORK:-}" ]; then cp -R "$WORK" "$KEEP_WORK"; fi
python3 "$POST" --respawn "$WORK/raw.rtr" "$WORK/world.log" "$HERE"
