#!/bin/sh
# Shared by the inventory-flow recorders (bank/, shop/, trade/): starts the
# dev lobby/world on scratch ports with a scratch player directory, runs the
# release client(s) with scripted inputs, and trims the recording with
# postprocess.py. Source it; then call flow_start, flow_client, flow_finish.
# Env: LOBBY_PORT (45650), WORLD_PORT (45651), CARGO_TARGET_DIR, PREFERENCES
# (a client preferences.dat to start from; default server/data/players/preferences.dat).
# The ports are read when flow_start runs, so a recorder sets them before or
# after sourcing this file.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../../../.." && pwd)
POST="$HERE/../postprocess.py"
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
PIDS=
trap 'stop_pids $PIDS; rm -rf "$WORK"' EXIT

flow_start() {
  LOBBY_PORT=${LOBBY_PORT:-45650}
  WORLD_PORT=${WORLD_PORT:-45651}
  case " $LOBBY_PORT $WORLD_PORT " in
    *" 43594 "* | *" 43595 "*) echo "flow-lib.sh: 43594 and 43595 are the shared dev server's ports" >&2; exit 1 ;;
  esac
  (cd "$ROOT/tools/client910" && cargo build --release)
  BIN=${CARGO_TARGET_DIR:-$ROOT/tools/target}/release/client910
  mkdir -p "$WORK/players"
  cd "$ROOT/server"
  export ALTO_LOBBY_PORT=$LOBBY_PORT ALTO_WORLD_PORT=$WORLD_PORT ALTO_PLAYER_DATA_DIR=$WORK/players ALTO_TRACE_INFO=1 ALTO_DEV_NPC_WANDER=0 ALTO_NPC_SPAWNS=dev
  node src/lostcity/lobby.ts > "$WORK/lobby.log" 2>&1 < /dev/null & PIDS="$!"
  node src/lostcity/world.ts > "$WORK/world.log" 2>&1 < /dev/null & PIDS="$PIDS $!"
  until grep -qi "listening on port $WORLD_PORT" "$WORK/world.log" && grep -qi "listening on port $LOBBY_PORT" "$WORK/lobby.log"; do sleep 1; done
}

# flow_client USER [server command...]: one release client logged in as USER
# (password "password"); CLIENT910_* injectors come from the caller's
# environment. Runs in the background; $CLIENT_PID is its pid.
flow_client() {
  user=$1; shift
  n=$#; i=0
  while [ $i -lt $n ]; do c=$1; shift; set -- "$@" --server-command "$c"; i=$((i + 1)); done
  mkdir -p "$WORK/client-$user"
  cp "${PREFERENCES:-$ROOT/server/data/players/preferences.dat}" "$WORK/client-$user/preferences.dat" 2>/dev/null || true
  (cd "$ROOT/tools/client910" && \
    CLIENT910_WINDOW_SIZE=${CLIENT910_WINDOW_SIZE:-1024,768} CLIENT910_PREFERENCES_FILE="$WORK/client-$user/preferences.dat" \
    CLIENT910_VARC_FILE="$WORK/client-$user/client-vars.dat" CLIENT910_UID192_FILE="$WORK/client-$user/random.dat" \
    exec "$BIN" --direct-login --lobby-port "$LOBBY_PORT" --world-port "$WORLD_PORT" \
      --username "$user" --password password --cache-dir "$WORK/client-$user/cache" ${CLIENT_ARGS:-} "$@" \
      > "$WORK/client-$user.log" 2>&1) &
  CLIENT_PID=$!
  PIDS="$PIDS $CLIENT_PID"
}

# flow_finish CYCLE OUT_DIR: wait until the recording (raw.rtr) has run past
# CYCLE (at most MAX_SECONDS, default 150), stop the recorded client and servers
# (SIGTERM, SIGKILL after a grace period), keep the trace below CYCLE.
flow_finish() {
  cycle=$1; out=$2; waited=0
  until python3 "$POST" --done-cycle "$cycle" "$WORK/raw.rtr" || [ $waited -ge "${MAX_SECONDS:-150}" ]; do
    sleep 1; waited=$((waited + 1))
  done
  stop_pids $PIDS
  if [ -n "${KEEP_WORK:-}" ]; then cp -R "$WORK" "$KEEP_WORK"; fi
  python3 "$POST" --until "$cycle" "$WORK/raw.rtr" "$WORK/world.log" "$out"
}
