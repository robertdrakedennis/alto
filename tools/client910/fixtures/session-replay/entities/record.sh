#!/bin/sh
# Regenerate the entity-presentation session-replay fixture
# (scenario_entities::recorded_entity_session_*):
#   tools/client910/fixtures/session-replay/entities/record.sh
# Starts the dev lobby/world with a scratch player directory and
# ALTO_TRACE_INFO=1, then runs the release client with CLIENT910_RECORD and the
# developer commands below (one per 100 logic cycles; the window must stay
# untouched, recording drops operating-system input). The client is stopped
# after CYCLES logic cycles (default 1100).
# Env: CYCLES, LOBBY_PORT (45660), WORLD_PORT (45661), MAX_SECONDS (200), CARGO_TARGET_DIR.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../../.." && pwd)
POST="$HERE/../postprocess.py"
LOBBY_PORT=${LOBBY_PORT:-45660}
WORLD_PORT=${WORLD_PORT:-45661}
MAX_SECONDS=${MAX_SECONDS:-200}
CYCLES=${CYCLES:-1100}
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
export ALTO_LOBBY_PORT=$LOBBY_PORT ALTO_WORLD_PORT=$WORLD_PORT ALTO_PLAYER_DATA_DIR=$WORK/players ALTO_TRACE_INFO=1 ALTO_DEV_NPC_WANDER=0 ALTO_NPC_SPAWNS=dev
mkdir -p "$WORK/players" "$WORK/client"
node src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & LOBBY=$!
node src/lostcity/world.ts > "$WORK/world.log" 2>&1 & WORLD=$!
until grep -q "listening on port $WORLD_PORT" "$WORK/world.log"; do sleep 1; done
[ ! -f "$ROOT/server/data/players/preferences.dat" ] || cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
cd "$ROOT/tools/client910"
env CLIENT910_RECORD="$WORK/raw.rtr" CLIENT910_WINDOW_SIZE=1024,768 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
"$BIN" --direct-login --lobby-port "$LOBBY_PORT" --world-port "$WORLD_PORT" \
  --username entities --password password --cache-dir "$WORK/cache" \
  --server-command "wear 1381" --server-command "spot 155 0" \
  --server-command "spot npc 1 155" --server-command "headicon 0 0 440" \
  --server-command "tint 8 4 40 100 200" --server-command "hit 7" \
  --server-command "hit npc 1 12" --server-command "hintarrow npc 1" \
  --server-command "npcheadicon 1 0 440 1" --server-command "npcadd 0 2 0" \
  > "$WORK/client.log" 2>&1 & CLIENT=$!
waited=0
until python3 "$POST" --done-cycle "$CYCLES" "$WORK/raw.rtr" || [ $waited -ge "$MAX_SECONDS" ]; do
  sleep 1; waited=$((waited + 1))
done
stop_pids $CLIENT
if [ -n "${KEEP_WORK:-}" ]; then cp -R "$WORK" "$KEEP_WORK"; fi
python3 "$POST" --until "$CYCLES" "$WORK/raw.rtr" "$WORK/world.log" "$HERE"
