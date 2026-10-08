#!/bin/sh
# Regenerate the NPC combat session-replay fixture
# (scenario_npc_combat::recorded_npc_session_*):
#   tools/client910/fixtures/session-replay/npc-combat/record.sh
# Needs the generated tables (`npm --prefix server run data:generate`).
# Starts the dev lobby/world (the generated population, wandering) with a
# scratch player directory and ALTO_TRACE_INFO=1, then runs the release client
# with CLIENT910_RECORD (the window must stay untouched; recording drops
# operating-system input):
# - the player is seeded on the Lumbridge farm (3231,3293), where the generated
#   chickens wander: the replay harness cannot rebuild the scene mid-session,
#   so the session starts there instead of teleporting;
# - server commands, one per 100 cycles: notimeout, `setxp 0 40000` and
#   `setxp 2 40000` (Attack and Strength level 46, so a fist hits often and a
#   chicken dies within a few swings) and `npcadd 41 1 0` (a chicken beside the
#   player that stands still, so the click lands, with the generated chicken
#   profile: drops, animations);
# - a left click on that chicken at ATTACK_CYCLE (the default op of a chicken
#   is Attack): the fight, the kill, the death sequence and the drop that
#   appears under the body; then a left click on its tile at TAKE_CYCLE, long
#   after the latest kill and its death ticks (the default op of a ground
#   item is Take). The pixels were found with CLIENT910_SCREENSHOT_SERIES in a
#   1024x768 window (screenshots are 2048x1536, twice the click coordinates):
#   CHICKEN_CLICK (default 596,393) is the chicken's body, TAKE_CLICK (default
#   608,320) the middle of the drop, which lies on the crate the chicken stood
#   on: a ground item is picked by its drawn model, not by the tile under it.
#   The crate's tile cannot be stood on, so the player takes the item from the side
#   (engine/interaction/behaviours/obj.ts): the stack goes and the item reaches the backpack.
# The client is stopped after CYCLES logic cycles (default 2100). With SHOTS_DIR
# set it saves a screenshot at each SHOTS cycle (client-only; the recording is
# unchanged) to find the kill and the drop.
# Env: CYCLES, CHICKEN_CLICK, TAKE_CLICK, ATTACK_CYCLE, TAKE_CYCLE, SHOTS_DIR, SHOTS,
# LOBBY_PORT (45670), WORLD_PORT (45671), MAX_SECONDS (600), CARGO_TARGET_DIR.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../../.." && pwd)
POST="$HERE/../postprocess.py"
LOBBY_PORT=${LOBBY_PORT:-45670}
WORLD_PORT=${WORLD_PORT:-45671}
MAX_SECONDS=${MAX_SECONDS:-600}
CYCLES=${CYCLES:-2100}
CHICKEN_CLICK=${CHICKEN_CLICK-596,393}
TAKE_CLICK=${TAKE_CLICK-608,320}
HOVER_CYCLE=${HOVER_CYCLE:-690}
ATTACK_CYCLE=${ATTACK_CYCLE:-700}
TAKE_CYCLE=${TAKE_CYCLE:-1700}
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
export ALTO_LOBBY_PORT=$LOBBY_PORT ALTO_WORLD_PORT=$WORLD_PORT ALTO_PLAYER_DATA_DIR=$WORK/players ALTO_TRACE_INFO=1
mkdir -p "$WORK/players/accounts" "$WORK/client"
ACCOUNT=$(printf fighter | shasum -a 256 | cut -d' ' -f1)
printf '{"version":2,"accountKey":"fighter","x":3231,"z":3293,"level":0}' > "$WORK/players/accounts/$ACCOUNT.json"
node src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & LOBBY=$!
node src/lostcity/world.ts > "$WORK/world.log" 2>&1 & WORLD=$!
until grep -q "listening on port $WORLD_PORT" "$WORK/world.log"; do sleep 1; done
[ ! -f "$ROOT/server/data/players/preferences.dat" ] || cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
cd "$ROOT/tools/client910"
CLICKS="l,$CHICKEN_CLICK,$ATTACK_CYCLE;l,$TAKE_CLICK,$TAKE_CYCLE"
HOVER="$CHICKEN_CLICK,$HOVER_CYCLE,$((TAKE_CYCLE - 11));$TAKE_CLICK,$((TAKE_CYCLE - 10)),99999"
SHOT_ARGS=
if [ -n "${SHOTS_DIR:-}" ]; then
  mkdir -p "$SHOTS_DIR"
  export CLIENT910_SCREENSHOT_SERIES=${SHOTS:-706,716,730,750,780,830,880,1100,1760,1900}
  SHOT_ARGS="--screenshot $SHOTS_DIR/npc-combat.png"
fi
env CLIENT910_RECORD="$WORK/raw.rtr" CLIENT910_WINDOW_SIZE=1024,768 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
CLIENT910_UI_CLICKS="$CLICKS" CLIENT910_UI_HOVER="$HOVER" \
"$BIN" --direct-login --lobby-port "$LOBBY_PORT" --world-port "$WORLD_PORT" \
  --username fighter --password password --cache-dir "$WORK/cache" \
  --server-command "notimeout" --server-command "setxp 0 40000" \
  --server-command "setxp 2 40000" \
  --server-command "npcadd 41 1 0" $SHOT_ARGS \
  > "$WORK/client.log" 2>&1 & CLIENT=$!
waited=0
until python3 "$POST" --done-cycle "$CYCLES" "$WORK/raw.rtr" || [ $waited -ge "$MAX_SECONDS" ]; do
  sleep 1; waited=$((waited + 1))
done
stop_pids $CLIENT
if [ -n "${KEEP_WORK:-}" ]; then cp -R "$WORK" "$KEEP_WORK"; fi
python3 "$POST" --until "$CYCLES" "$WORK/raw.rtr" "$WORK/world.log" "$HERE"
