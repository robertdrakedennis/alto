#!/bin/sh
# Regenerate the headless session-replay fixture (app::session_replay):
#   tools/client910/fixtures/session-replay/record.sh
# Starts the dev lobby/world (ALTO_TRACE_INFO=1, a scratch player directory),
# runs the release client with CLIENT910_RECORD and the scripted injectors /
# server commands below, then trims the trace and extracts the server's own
# PLAYER_INFO / NPC_INFO positions (postprocess.py). The client window must
# stay untouched: recording drops operating-system input (session_record.rs).
# The client is stopped once the cutscene's map transaction completes (or
# after MAX_SECONDS). Env: LOBBY_PORT (39594), WORLD_PORT (39595),
# MAX_SECONDS (120), CARGO_TARGET_DIR.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
LOBBY_PORT=${LOBBY_PORT:-39594}
WORLD_PORT=${WORLD_PORT:-39595}
MAX_SECONDS=${MAX_SECONDS:-120}
WORK=$(mktemp -d)
trap 'kill $LOBBY $WORLD 2>/dev/null || true; rm -rf "$WORK"' EXIT
(cd "$ROOT/tools/client910" && cargo build --release)
BIN=${CARGO_TARGET_DIR:-$ROOT/tools/target}/release/client910
cd "$ROOT/server"
export ALTO_LOBBY_PORT=$LOBBY_PORT ALTO_WORLD_PORT=$WORLD_PORT ALTO_PLAYER_DATA_DIR=$WORK/players ALTO_TRACE_INFO=1
mkdir -p "$WORK/players" "$WORK/client"
node src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & LOBBY=$!
node src/lostcity/world.ts > "$WORK/world.log" 2>&1 & WORLD=$!
until grep -qi "listening on port $WORLD_PORT" "$WORK/world.log"; do sleep 1; done
cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
cd "$ROOT/tools/client910"
CLIENT910_RECORD="$WORK/raw.rtr" CLIENT910_WINDOW_SIZE=1024,768 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
CLIENT910_UI_CLICKS="l,930,125,60;l,900,100,280" \
CLIENT910_UI_OPERATIONS="250,96010251,-1,1" \
CLIENT910_TYPE_INPUT='450:\nhello replay\n' \
"$BIN" --direct-login --lobby-port "$LOBBY_PORT" --world-port "$WORLD_PORT" \
  --username replay --password password --cache-dir "$WORK/cache" \
  --server-command "varp 1001 -5" --server-command "varp 1002 123456" \
  --server-command "varbit 1000 1" --server-command "gamemessage hello from the replay" \
  --server-command "hintarrow tile 3224 3224" --server-command "song 2" \
  --server-command "notimeout" --server-command "ping 1234 5678" \
  --server-command "cutscene 2" > "$WORK/client.log" 2>&1 & CLIENT=$!
waited=0
until python3 "$HERE/postprocess.py" --done "$WORK/raw.rtr" || [ $waited -ge "$MAX_SECONDS" ]; do
  sleep 1; waited=$((waited + 1))
done
kill -9 $CLIENT 2>/dev/null || true
python3 "$HERE/postprocess.py" "$WORK/raw.rtr" "$WORK/world.log" "$HERE"
