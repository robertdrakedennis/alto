#!/bin/bash
# The scripted online run of the session lifecycle (see docs/client-status.md):
# log in to world 1, hop to world 2 in game, lose the connection to world 2 and
# reconnect. Starts a lobby and two worlds of the dev server on their own ports
# (BASE .. BASE+12), a relay in front of each world (so the run can cut a
# connection on the client's side only), and the release client with the
# scripted server commands. Every process is stopped by PID at the end; the
# client exits after MAX_SECONDS at the latest.
#
#   tools/client910/fixtures/session-hop/run.sh [OUT_DIR]
#
# Env: BASE (47594), MAX_SECONDS (150), CARGO_TARGET_DIR, SHOTS (comma list of
# logic cycles for PNGs in OUT_DIR). Launches the client once.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
BASE=${BASE:-47594}
MAX_SECONDS=${MAX_SECONDS:-150}
case "$BASE$MAX_SECONDS" in *[!0-9]*) echo "BASE and MAX_SECONDS must be numbers" >&2; exit 2;; esac
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
for offset in 0 1 2 11 12; do
  if lsof -iTCP:$((BASE + offset)) -sTCP:LISTEN -P -n >/dev/null 2>&1; then
    echo "port $((BASE + offset)) is in use" >&2; exit 2
  fi
done
(cd "$ROOT/tools/client910" && cargo build --release)
BIN=${CARGO_TARGET_DIR:-$ROOT/tools/target}/release/client910

# The relays: world 1 and 2 as the client and the lobby know them (BASE + id)
# forward to the worlds themselves (BASE + 10 + id).
python3 "$HERE/proxy.py" $((BASE + 1)) $((BASE + 11)) "$WORK/drop1" & PIDS="$PIDS $!"
python3 "$HERE/proxy.py" $((BASE + 2)) $((BASE + 12)) "$WORK/drop2" & PIDS="$PIDS $!"

cd "$ROOT/server"
export ALTO_WORLD_PORT_BASE=$BASE ALTO_LOBBY_PORT=$BASE ALTO_WORLD_IDS=1,2 ALTO_DEFAULT_WORLD=1 \
  ALTO_PLAYER_DATA_DIR=$WORK/players
LOADER="node"
$LOADER src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 & PIDS="$PIDS $!"
ALTO_WORLD_ID=1 ALTO_WORLD_PORT=$((BASE + 11)) $LOADER src/lostcity/world.ts > "$WORK/world1.log" 2>&1 & PIDS="$PIDS $!"
ALTO_WORLD_ID=2 ALTO_WORLD_PORT=$((BASE + 12)) $LOADER src/lostcity/world.ts > "$WORK/world2.log" 2>&1 & PIDS="$PIDS $!"
waited=0
until grep -q "listening on port $((BASE + 11))" "$WORK/world1.log" 2>/dev/null \
   && grep -q "listening on port $((BASE + 12))" "$WORK/world2.log" 2>/dev/null \
   && grep -q "Listening on port $BASE" "$WORK/lobby.log" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt 120 ] || { echo "the servers did not start" >&2; exit 1; }
done

[ ! -f "$ROOT/server/data/players/preferences.dat" ] || cp "$ROOT/server/data/players/preferences.dat" "$WORK/client/preferences.dat"
cd "$ROOT/tools/client910"
SHOT_ARGS=""
if [ -n "${SHOTS:-}" ]; then
  SHOT_ARGS="--screenshot $OUT/hop.png"
  export CLIENT910_SCREENSHOT_SERIES=$SHOTS
fi
CLIENT910_LOG=info CLIENT910_WINDOW_SIZE=1024,768 \
CLIENT910_PREFERENCES_FILE="$WORK/client/preferences.dat" \
CLIENT910_VARC_FILE="$WORK/client/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client/random.dat" \
"$BIN" --direct-login --lobby-port "$BASE" --world-port $((BASE + 1)) \
  --username hopper --password password --cache-dir "$WORK/cache" $SHOT_ARGS \
  --server-command "transfer 2" > "$OUT/client.log" 2>&1 & CLIENT=$!
PIDS="$PIDS $CLIENT"
waited=0
until grep -q "world login ok: world 2" "$OUT/client.log" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt "$MAX_SECONDS" ] || { echo "the hop to world 2 did not finish" >&2; break; }
done
sleep 4
touch "$WORK/drop2"
waited=0
until grep -q "reconnect resumed in place" "$OUT/client.log" 2>/dev/null; do
  sleep 1; waited=$((waited + 1))
  [ $waited -lt 60 ] || { echo "the reconnect did not resume" >&2; break; }
done
sleep 5
cp "$WORK/world1.log" "$WORK/world2.log" "$WORK/lobby.log" "$OUT/"
echo "run output in $OUT"
